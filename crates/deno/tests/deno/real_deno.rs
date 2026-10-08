//! The runner with the real Deno: a package from an npm registry on
//! 127.0.0.1 is cached, started in its sandbox and asked what it can reach.
//! The Node app had no such test; it is what shows that the argument list
//! still means, to Deno, what the runner intends.
//!
//! Each test skips, with a message, on a machine without Deno.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use base64::Engine;
use mymcps_deno::{DenoRunner, DenoRuntime, HostEnvironment};
use serde_json::{Value, json};
use sha2::{Digest, Sha512};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::support::{Sandbox, announce, display, inherited_from_the_server};

/// What Deno itself adds to the environment a package sees. It also puts a
/// directory of its cache, with a `node` that is Deno, first on the `PATH`.
const DENO_VARIABLES: [&str; 3] = [
    "npm_config_user_agent",
    "DENO_NODE_SHIM_ACTIVE",
    "__CF_USER_TEXT_ENCODING",
];

const PACKAGE_JSON: &str =
    r#"{"name":"@example/fake-mcp","version":"1.0.0","bin":{"fake-mcp":"./server.js"}}"#;

/// A minimal MCP stdio server, as a Node package. Its only tool tries what a
/// package must and must not be able to do.
const SERVER_JS: &str = r#"#!/usr/bin/env node
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
    rewriteItself: attempt(() => fs.writeFileSync(__filename, 'rewritten')),
    run: attempt(() => require('node:child_process').execFileSync('/bin/echo', ['ran']).toString()),
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

/// The Deno of this machine: `DENO_PATH`, the `PATH`, the usual locations.
fn real_deno(test: &str) -> Option<PathBuf> {
    let configured = std::env::var("DENO_PATH").ok();
    let located = HostEnvironment::from_process().locate_deno_binary(configured.as_deref());
    if located.is_err() {
        announce(
            test,
            "skipped, Deno is not installed (looked at DENO_PATH, the PATH and the usual locations)",
        );
    }
    located.ok()
}

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
struct Registry {
    port: u16,
    requests: Arc<Mutex<Vec<String>>>,
    serving: tokio::task::JoinHandle<()>,
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.serving.abort();
    }
}

impl Registry {
    async fn start() -> Self {
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

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    /// The request lines so far, without the HTTP version.
    fn requests(&self) -> Vec<String> {
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

fn runner_with_real_deno(sandbox: &Sandbox, deno: PathBuf) -> DenoRunner {
    DenoRunner::with_runtime(
        sandbox.core.core.clone(),
        DenoRuntime::with_host(None, sandbox.host()).binary_path(deno),
    )
}

/// The variables of an npm MCP that fetches from `registry` and from nowhere
/// else: whatever Deno might ask of another host is sent to the registry as
/// well, which refuses it.
fn registry_variables(registry: &Registry) -> Vec<(&'static str, String)> {
    let proxy = format!("http://127.0.0.1:{}", registry.port);
    vec![
        ("NPM_CONFIG_REGISTRY", registry.url()),
        ("HTTPS_PROXY", proxy.clone()),
        ("HTTP_PROXY", proxy),
        ("NO_PROXY", "127.0.0.1".to_owned()),
    ]
}

fn as_refs<'a>(variables: &'a [(&'static str, String)]) -> Vec<(&'a str, &'a str)> {
    variables
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect()
}

#[tokio::test]
async fn runs_a_package_that_reads_its_cache_and_writes_only_its_sandbox() {
    let Some(deno) = real_deno("runs_a_package_that_reads_its_cache_and_writes_only_its_sandbox")
    else {
        return;
    };
    let sandbox = Sandbox::new().await;
    let registry = Registry::start().await;
    let runner = runner_with_real_deno(&sandbox, deno);

    let mut variables = registry_variables(&registry);
    variables.extend([
        ("API_KEY", "kept-secret".to_owned()),
        // Saved before these names were refused.
        ("DENO_DIR", display(&sandbox.path().join("member-cache"))),
        ("HOME", "/member/home".to_owned()),
        ("NODE_OPTIONS", "--require=/member/evil.js".to_owned()),
    ]);
    let mut mcp = sandbox.npm_mcp(&as_refs(&variables));
    mcp.set_npm_args_list(&["--flag".to_owned()]);
    let sandbox_dir = runner.sandbox_root_for(mcp.id);
    let deno_dir = sandbox.deno_dir();

    // What a package must not reach: the database, the `.env` of the
    // application, the sandbox of another MCP.
    let database = sandbox.core.config.database_path();
    assert!(database.exists());
    let dotenv = sandbox.host().current_dir.join(".env");
    std::fs::write(&dotenv, "APP_KEY=not-for-packages\n").unwrap();
    let neighbour = runner.sandbox_root_for(8);
    std::fs::create_dir_all(&neighbour).unwrap();
    std::fs::write(
        neighbour.join("token.json"),
        "{\"token\":\"of another MCP\"}",
    )
    .unwrap();

    // What an earlier run of the package may have left in its sandbox must
    // not turn the sandbox into a Node project for Deno.
    std::fs::create_dir_all(&sandbox_dir).unwrap();
    let left_behind = r#"{"name":"left-by-the-package","dependencies":{"left-pad":"1.3.0"}}"#;
    std::fs::write(sandbox_dir.join("package.json"), left_behind).unwrap();

    // Update MCP: the package is downloaded into the one cache directory.
    runner.reload_npm_package_cache(&mcp).await.unwrap();
    assert_eq!(
        registry.requests(),
        [
            "GET /@example%2ffake-mcp",
            "GET /@example/fake-mcp/-/fake-mcp-1.0.0.tgz"
        ]
    );
    let cached_registries: Vec<PathBuf> = std::fs::read_dir(deno_dir.join("npm"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    let [cached_registry] = cached_registries.as_slice() else {
        panic!("one registry in the cache, not {cached_registries:?}");
    };
    let cached_package = cached_registry.join("@example").join("fake-mcp");
    assert!(cached_package.join("1.0.0").join("server.js").is_file());

    let tools = runner.list_tools(&mcp).await.unwrap();
    assert_eq!(
        serde_json::to_value(&tools).unwrap(),
        json!([{
            "name": "probe",
            "description": "Tries the limits of the sandbox",
            "inputSchema": { "type": "object" }
        }])
    );

    let arguments = json!({
        "read": {
            "database": database,
            "dotenv": dotenv,
            "neighbour": neighbour.join("token.json"),
            "ownState": "state.json",
        },
        "list": {
            "application": sandbox.host().current_dir,
            "dataDirectory": sandbox.core.config.data_dir,
            "sandboxes": sandbox_dir.parent().unwrap(),
            "ownSandbox": sandbox_dir,
            "cache": deno_dir,
        },
        "write": {
            "ownState": "state.json",
            "inTmpdir": sandbox_dir.join("scratch.tmp"),
            "application": sandbox.host().current_dir.join("written-by-package"),
            "neighbour": neighbour.join("token.json"),
            "cache": deno_dir.join("written-by-package"),
        },
    });
    let result = runner
        .call_tool(&mcp, "probe", arguments.as_object().unwrap().clone())
        .await
        .unwrap();
    let probe: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();

    // The package runs in its sandbox, which is also its home and its temporary directory.
    assert_eq!(probe["argv"], json!(["--flag"]));
    let canonical = |value: &Value| {
        PathBuf::from(value.as_str().unwrap())
            .canonicalize()
            .unwrap()
    };
    assert_eq!(
        canonical(&probe["cwd"]),
        sandbox_dir.canonicalize().unwrap()
    );
    assert_eq!(probe["homedir"], json!(sandbox_dir));
    assert_eq!(probe["tmpdir"], json!(sandbox_dir));

    // Its environment, whole: the variables of the MCP that are not reserved,
    // what the gateway sets, and the four names every stdio server inherits.
    let node_shim = format!("{}:", deno_dir.join("node_compat_bin").display());
    let environment: BTreeMap<String, String> = probe["env"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(name, _)| !DENO_VARIABLES.contains(&name.as_str()))
        .map(|(name, value)| {
            let value = value.as_str().unwrap();
            let value = match name.as_str() {
                "PATH" => value.strip_prefix(&node_shim).unwrap_or(value),
                _ => value,
            };
            (name.clone(), value.to_owned())
        })
        .collect();
    let mut expected = inherited_from_the_server();
    expected.extend(
        registry_variables(&registry)
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value)),
    );
    expected.extend(
        [
            ("API_KEY", "kept-secret".to_owned()),
            ("PATH", sandbox.server_path()),
            ("HOME", display(&sandbox_dir)),
            ("TMPDIR", display(&sandbox_dir)),
            ("DENO_DIR", display(&deno_dir)),
            ("NO_COLOR", "1".to_owned()),
        ]
        .map(|(name, value)| (name.to_owned(), value)),
    );
    assert_eq!(environment, expected);

    // It reads the files it was installed with, and reads and writes its sandbox.
    assert_eq!(
        probe["asset"],
        json!({ "ok": true, "value": "packaged asset" })
    );
    let written = json!({ "ok": true, "value": "written by the package" });
    assert_eq!(probe["write"]["ownState"], written);
    assert_eq!(probe["write"]["inTmpdir"], written);
    assert_eq!(probe["read"]["ownState"], written);
    assert_eq!(probe["list"]["ownSandbox"]["ok"], json!(true));
    assert_eq!(probe["list"]["cache"]["ok"], json!(true));
    assert_eq!(
        std::fs::read_to_string(sandbox_dir.join("state.json")).unwrap(),
        "written by the package"
    );

    // Everything else is refused by Deno.
    let refused = |attempt: &Value, access: &str| {
        assert_eq!(attempt["ok"], json!(false), "{attempt}");
        let error = attempt["error"].as_str().unwrap();
        assert!(
            error.contains(&format!("Requires {access} access")),
            "{error}"
        );
    };
    refused(&probe["read"]["database"], "read");
    refused(&probe["read"]["dotenv"], "read");
    refused(&probe["read"]["neighbour"], "read");
    refused(&probe["list"]["application"], "read");
    refused(&probe["list"]["dataDirectory"], "read");
    refused(&probe["list"]["sandboxes"], "read");
    refused(&probe["write"]["application"], "write");
    refused(&probe["write"]["neighbour"], "write");
    refused(&probe["write"]["cache"], "write");
    refused(&probe["rewriteItself"], "write");
    assert_eq!(probe["run"]["ok"], json!(false), "{}", probe["run"]);
    assert!(
        !sandbox
            .host()
            .current_dir
            .join("written-by-package")
            .exists()
    );
    assert!(!deno_dir.join("written-by-package").exists());
    assert_eq!(
        std::fs::read_to_string(neighbour.join("token.json")).unwrap(),
        "{\"token\":\"of another MCP\"}"
    );
    assert_eq!(
        std::fs::read_to_string(cached_package.join("1.0.0").join("server.js")).unwrap(),
        SERVER_JS
    );

    // Deno kept its cache where it was told to, and left no project files.
    for unwanted in [
        ".cache",
        "Library",
        "node_modules",
        "deno.lock",
        "deno.json",
    ] {
        assert!(!sandbox_dir.join(unwanted).exists(), "{unwanted}");
    }
    assert_eq!(
        std::fs::read_to_string(sandbox_dir.join("package.json")).unwrap(),
        left_behind
    );

    // The cache Deno wrote is the one the version shown in the UI is read
    // from. That reader looks under the public registry, so stand it in.
    std::fs::rename(
        cached_registry,
        deno_dir.join("npm").join("registry.npmjs.org"),
    )
    .unwrap();
    let cached = |version| runner.cached_npm_package_version("@example/fake-mcp", version);
    assert_eq!(cached(None).as_deref(), Some("1.0.0"));
    assert_eq!(cached(Some("latest")).as_deref(), Some("1.0.0"));
    assert_eq!(cached(Some("1.0.0")).as_deref(), Some("1.0.0"));
    assert_eq!(cached(Some("2.0.0")), None);
}

#[tokio::test]
async fn explains_a_package_that_deno_could_not_start() {
    let Some(deno) = real_deno("explains_a_package_that_deno_could_not_start") else {
        return;
    };
    let sandbox = Sandbox::new().await;
    let registry = Registry::start().await;
    let runner = runner_with_real_deno(&sandbox, deno);
    let mut mcp = sandbox.npm_mcp(&as_refs(&registry_variables(&registry)));
    mcp.npm_package = Some("@example/missing-mcp".to_owned());

    let failure = runner.list_tools(&mcp).await.unwrap_err();

    assert!(failure.is_startup_failure());
    let message = failure.to_string();
    assert!(
        message.starts_with(
            "Failed to start Deno npm MCP \"@example/missing-mcp\". MCP error -32000: Connection closed. Output: error: "
        ),
        "{message}"
    );
    assert!(message.contains("@example/missing-mcp"), "{message}");
    assert!(!message.contains("Is Deno installed?"));
    // No colours, thanks to NO_COLOR, and on one line.
    assert!(
        !message.contains('\u{1B}') && !message.contains('\n'),
        "{message:?}"
    );

    let failure = runner.reload_npm_package_cache(&mcp).await.unwrap_err();
    let message = failure.to_string();
    assert!(
        message.starts_with("Failed to reload Deno cache for \"@example/missing-mcp\". error: "),
        "{message}"
    );
    assert!(
        registry
            .requests()
            .iter()
            .any(|request| request.eq_ignore_ascii_case("GET /@example%2fmissing-mcp")),
        "{:?}",
        registry.requests()
    );
}
