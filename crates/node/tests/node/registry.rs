//! An npm registry on 127.0.0.1 with one package, a minimal MCP server whose
//! only tool tries what a package must and must not be able to do. Shared by
//! the tests that run the real Node.js: here without a sandbox, and in
//! `crates/cli` inside the real one.

use std::io::Write;
use std::sync::{Arc, Mutex};

use base64::Engine;
use serde_json::json;
use sha2::{Digest, Sha512};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A package that would leave a trace if npm ran its scripts.
const PACKAGE_JSON: &str = r#"{"name":"@example/fake-mcp","version":"1.0.0","bin":{"fake-mcp":"./server.js"},"scripts":{"preinstall":"node -e \"require('fs').writeFileSync('script-ran','')\"","postinstall":"node -e \"require('fs').writeFileSync('script-ran','')\""}}"#;

/// A minimal MCP stdio server, as a Node package. Its only tool tries what a
/// package must and must not be able to do.
pub const SERVER_JS: &str = r#"#!/usr/bin/env node
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

function attempt(action) {
  try {
    return { ok: true, value: action() }
  } catch (error) {
    return { ok: false, error: error.name + ': ' + String(error.message).split('\n')[0] }
  }
}
function each(paths, action) {
  return Object.fromEntries(Object.entries(paths || {}).map(([name, target]) => [name, attempt(() => action(target))]))
}
function probe(args) {
  return {
    argv: process.argv.slice(2),
    cwd: process.cwd(),
    env: { ...process.env },
    homedir: os.homedir(),
    tmpdir: os.tmpdir(),
    asset: attempt(() => fs.readFileSync(path.join(__dirname, 'asset.txt'), 'utf8')),
    write: each(args.write, (target) => {
      fs.writeFileSync(target, 'written by the package')
      return fs.readFileSync(target, 'utf8')
    }),
    read: each(args.read, (target) => fs.readFileSync(target, 'utf8')),
    list: each(args.list, (target) => fs.readdirSync(target)),
    chmod: each(args.chmod, (target) => fs.chmodSync(target, 0o600)),
    truncate: each(args.truncate, (target) => fs.truncateSync(target, 0)),
    rewriteItself: attempt(() => fs.writeFileSync(__filename, 'rewritten')),
    run: attempt(() => require('node:child_process').execFileSync('/bin/echo', ['ran']).toString()),
    signalServer: attempt(() => process.kill(process.ppid, 0)),
    signalItself: attempt(() => process.kill(process.pid, 0)),
    interfaces: attempt(() => Object.keys(os.networkInterfaces()).length > 0),
    allocate: attempt(() => Buffer.alloc(args.allocate || 1).length),
    threads: attempt(() => new (require('node:worker_threads').Worker)('1', { eval: true }).threadId > 0),
  }
}
function send(message) {
  process.stdout.write(JSON.stringify(message) + '\n')
}
let buffer = ''
process.stdin.on('data', (chunk) => {
  buffer += chunk
  for (let end = buffer.indexOf('\n'); end >= 0; end = buffer.indexOf('\n')) {
    const line = buffer.slice(0, end).trim()
    buffer = buffer.slice(end + 1)
    if (!line) continue
    const message = JSON.parse(line)
    if (message.method === 'initialize') {
      send({ jsonrpc: '2.0', id: message.id, result: {
        protocolVersion: message.params.protocolVersion,
        capabilities: { tools: {} },
        serverInfo: { name: 'fake-mcp', version: '1.0.0' },
      } })
    } else if (message.method === 'tools/list') {
      send({ jsonrpc: '2.0', id: message.id, result: { tools: [
        { name: 'probe', description: 'Tries the limits of the sandbox', inputSchema: { type: 'object' } },
      ] } })
    } else if (message.method === 'tools/call') {
      send({ jsonrpc: '2.0', id: message.id, result: { content: [
        { type: 'text', text: JSON.stringify(probe(message.params.arguments || {})) },
      ] } })
    }
  }
})
process.stdin.on('end', () => process.exit(0))
"#;

/// The package as `npm publish` uploads it: a gzipped tar of `package/`.
fn package_tarball() -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut archive = tar::Builder::new(encoder);
    for (name, content, mode) in [
        ("package.json", PACKAGE_JSON, 0o644),
        ("server.js", SERVER_JS, 0o755),
        ("asset.txt", "packaged asset", 0o644),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(content.len() as u64);
        header.set_mode(mode);
        archive
            .append_data(&mut header, format!("package/{name}"), content.as_bytes())
            .unwrap();
    }
    let mut encoder = archive.into_inner().unwrap();
    encoder.flush().unwrap();
    encoder.finish().unwrap()
}

/// An npm registry on 127.0.0.1 that has one package, `@example/fake-mcp`.
pub struct Registry {
    pub port: u16,
    requests: Arc<Mutex<Vec<String>>>,
    serving: tokio::task::JoinHandle<()>,
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

impl Registry {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let requests: Arc<Mutex<Vec<String>>> = Arc::default();

        let tarball = Arc::new(package_tarball());
        let integrity = format!(
            "sha512-{}",
            base64::engine::general_purpose::STANDARD.encode(Sha512::digest(tarball.as_slice()))
        );
        let document = Arc::new(
            json!({
                "name": "@example/fake-mcp",
                "dist-tags": { "latest": "1.0.0" },
                "versions": {
                    "1.0.0": {
                        "name": "@example/fake-mcp",
                        "version": "1.0.0",
                        "bin": { "fake-mcp": "./server.js" },
                        "hasInstallScript": true,
                        "dist": {
                            "tarball": format!("http://127.0.0.1:{port}/@example/fake-mcp/-/fake-mcp-1.0.0.tgz"),
                            "integrity": integrity,
                        },
                    },
                },
            })
            .to_string()
            .into_bytes(),
        );

        let seen = requests.clone();
        let serving = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(answer(
                    stream,
                    seen.clone(),
                    document.clone(),
                    tarball.clone(),
                ));
            }
        });
        Self {
            port,
            requests,
            serving,
        }
    }

    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    /// The request lines so far, without the HTTP version.
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

async fn answer(
    mut stream: TcpStream,
    seen: Arc<Mutex<Vec<String>>>,
    document: Arc<Vec<u8>>,
    tarball: Arc<Vec<u8>>,
) {
    let mut head = Vec::new();
    let mut chunk = [0_u8; 4096];
    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
        match stream.read(&mut chunk).await {
            Ok(read) if read > 0 && head.len() < 64 * 1024 => {
                head.extend_from_slice(&chunk[..read])
            }
            _ => return,
        }
    }
    let head = String::from_utf8_lossy(&head);
    let mut request_line = head.lines().next().unwrap_or_default().split(' ');
    let method = request_line.next().unwrap_or_default();
    let target = request_line.next().unwrap_or_default();
    seen.lock().unwrap().push(format!("{method} {target}"));

    let (status, content_type, body): (&str, &str, &[u8]) =
        match (method, target.to_ascii_lowercase().as_str()) {
            ("GET", "/@example%2ffake-mcp") => ("200 OK", "application/json", &document),
            ("GET", "/@example/fake-mcp/-/fake-mcp-1.0.0.tgz") => {
                ("200 OK", "application/octet-stream", &tarball)
            }
            // Anything else, a request to be forwarded to another host included.
            _ => (
                "404 Not Found",
                "application/json",
                b"{\"error\":\"Not found\"}",
            ),
        };
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.write_all(body).await;
    let _ = stream.shutdown().await;
}

/// The variables of an npm MCP that fetches from `registry` and from nowhere
/// else: whatever npm might ask of another host is sent to the registry as
/// well, which refuses it.
pub fn registry_variables(registry: &Registry) -> Vec<(&'static str, String)> {
    let proxy = format!("http://127.0.0.1:{}", registry.port);
    vec![
        ("NPM_CONFIG_REGISTRY", registry.url()),
        ("HTTPS_PROXY", proxy.clone()),
        ("HTTP_PROXY", proxy),
        ("NO_PROXY", "127.0.0.1".to_owned()),
    ]
}

/// The Node.js of this machine that npm MCPs can run with: the binary
/// itself, where a version manager puts a program of its own on the `PATH`,
/// and one that has npm beside it.
pub fn real_node() -> Option<std::path::PathBuf> {
    let configured = std::env::var("NODE_BINARY").ok();
    let found = mymcps_node::HostEnvironment::from_process()
        .locate_node_binary(configured.as_deref())
        .ok()?;
    let answer = std::process::Command::new(found)
        .args(["-p", "process.execPath"])
        .output()
        .ok()?;
    let node = std::path::PathBuf::from(String::from_utf8(answer.stdout).ok()?.trim());
    mymcps_node::locate_npm_cli(&node).is_ok().then_some(node)
}
