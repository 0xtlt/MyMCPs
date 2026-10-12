//! npm MCPs through the upstream, with a shell script standing in for the
//! `node` binary: listing, calling, testing and updating one, and what an
//! administrator reads when it does not start.
#![cfg(unix)]

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mymcps_core::TestCore;
use mymcps_core::models::{Mcp, McpStatus, McpTransport};
use mymcps_core::secrets::{EnvironmentInput, merge_environment};
use mymcps_node::{HostEnvironment, NodeRunner, NodeRuntime};
use mymcps_upstream::Upstream;
use serde_json::json;
use support::*;

/// Given npm's `install` it records its arguments and writes the files of
/// the package where npm would; given anything else it is a minimal MCP
/// stdio server.
const FAKE_NODE: &str = r#"#!/bin/sh
if [ "$2" = install ]; then
  for argument in "$@"; do printf '%s\n' "$argument"; done > install.argv
  prefix=
  while [ $# -gt 1 ]; do
    if [ "$1" = --prefix ]; then prefix=$2; fi
    shift
  done
  package=${1%@*}
  mkdir -p "$prefix/node_modules/$package"
  echo '' > "$prefix/node_modules/$package/server.js"
  printf '{"name":"%s","version":"%s","bin":"server.js"}\n' "$package" "${FAKE_VERSION:-1.0.0}" \
    > "$prefix/node_modules/$package/package.json"
  exit 0
fi
if [ "$FAKE_BEHAVIOUR" = refuse-key ]; then
  printf 'error: cannot start, the key %s was refused\n' "$API_KEY" >&2
  exit 3
fi
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      version=$(printf '%s\n' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"fake-node","version":"1.0.0"}}}\n' "$id" "$version" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"snapshot","title":"Snapshot","description":"Describes the process it runs in","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}},{"name":"bare","inputSchema":{"type":"object","properties":{"q":{"type":"string"}}}}]}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      printf '%s\n' "$line" > call.json
      case "$line" in
        *'"name":"failing"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"it failed"}],"isError":true}}\n' "$id" ;;
        *)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"called"}]}}\n' "$id" ;;
      esac ;;
  esac
done
"#;

struct Server {
    core: TestCore,
    upstream: Arc<Upstream>,
    _directory: tempfile::TempDir,
}

impl Server {
    async fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let fake = bin.join("node");
        // Written by a child process, never by this one: a test on another
        // thread that starts a command while a script is open for writing
        // here would hand that descriptor to its child for a moment, and
        // Linux refuses to run a file anyone has open for writing (ETXTBSY).
        let mut writer = std::process::Command::new("/bin/sh")
            .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
            .arg(&fake)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        std::io::Write::write_all(&mut writer.stdin.take().unwrap(), FAKE_NODE.as_bytes()).unwrap();
        assert!(writer.wait().unwrap().success());
        std::fs::create_dir_all(root.join("app")).unwrap();

        let core = TestCore::with_config(|config| {
            config.data_dir = config.data_dir.canonicalize().unwrap();
        })
        .await;
        let host = HostEnvironment {
            path: Some("/usr/bin:/bin".to_owned()),
            // The Node.js of this machine, if it has one, is never found by accident.
            known_node_locations: Vec::new(),
            current_dir: root.join("app"),
        };
        let node = NodeRunner::with_runtime(
            core.core.clone(),
            NodeRuntime::with_host(None, host)
                .binary_path(fake)
                .npm_cli(root.join("npm-cli.js"))
                // What the kernel enforces is tested with the real launcher.
                .unsandboxed(),
        );
        let upstream = Upstream::builder(core.core.clone(), Default::default())
            .node(node)
            .build();
        Self {
            core,
            upstream,
            _directory: directory,
        }
    }

    async fn npm_mcp(&self, environment: &[(&str, &str)]) -> Mcp {
        let entries: Vec<EnvironmentInput> = environment
            .iter()
            .map(|(name, value)| EnvironmentInput {
                name: (*name).to_owned(),
                value: Some((*value).to_owned()),
            })
            .collect();
        let npm_env = merge_environment(&self.core.encryption, None, &entries);
        create_mcp(&self.core, |mcp| {
            mcp.name = "Fake MCP".into();
            mcp.transport = McpTransport::Npm;
            mcp.npm_package = Some("@example/fake-mcp".into());
            mcp.npm_env = npm_env;
            mcp.status = McpStatus::Draft;
        })
        .await
    }

    fn sandbox(&self, mcp: &Mcp) -> PathBuf {
        self.upstream.node().sandbox_root_for(mcp.id)
    }

    /// The directory of the install in use for `mcp`.
    fn install(&self, mcp: &Mcp) -> PathBuf {
        let packages = self.upstream.node().packages_root_for(mcp.id);
        let current: serde_json::Value =
            serde_json::from_slice(&std::fs::read(packages.join("current.json")).unwrap()).unwrap();
        packages.join(current["install"].as_str().unwrap())
    }
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn lists_and_calls_the_tools_of_an_npm_mcp() {
    let server = Server::new().await;
    let mut mcp = server.npm_mcp(&[]).await;

    let tools = server.upstream.probe(&mut mcp).await.unwrap();
    assert_eq!(
        serde_json::to_value(&tools).unwrap(),
        json!([
            {
                "name": "snapshot",
                "description": "Describes the process it runs in",
                "inputSchema": { "type": "object" },
            },
            {
                "name": "bare",
                "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } },
            },
        ])
    );

    let arguments = json!({ "q": "hello" }).as_object().cloned();
    let result = server
        .upstream
        .call_tool(&mut mcp, "snapshot", arguments)
        .await
        .unwrap();
    assert_eq!(
        result,
        json!({ "content": [{ "type": "text", "text": "called" }] })
    );
    let call: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(server.sandbox(&mcp).join("call.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        call["params"],
        json!({ "name": "snapshot", "arguments": { "q": "hello" } })
    );

    // A call without arguments is made with an empty object, and a tool
    // that fails answers with a result.
    let failed = server
        .upstream
        .call_tool(&mut mcp, "failing", None)
        .await
        .unwrap();
    assert_eq!(failed["isError"], true);
    let call: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(server.sandbox(&mcp).join("call.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        call["params"],
        json!({ "name": "failing", "arguments": {} })
    );
}

#[tokio::test]
async fn updates_a_latest_tracking_npm_mcp_by_installing_it_again_then_testing_it() {
    let server = Server::new().await;
    let mut mcp = server.npm_mcp(&[("FAKE_VERSION", "1.0.0")]).await;
    server.upstream.probe(&mut mcp).await.unwrap();
    let before = server.install(&mcp);
    assert_eq!(
        server
            .upstream
            .installed_npm_package_version(&mcp)
            .as_deref(),
        Some("1.0.0")
    );

    // The registry has a newer version by now.
    mcp.npm_env = merge_environment(
        &server.core.encryption,
        None,
        &[EnvironmentInput {
            name: "FAKE_VERSION".to_owned(),
            value: Some("1.1.0".to_owned()),
        }],
    );
    server
        .upstream
        .update_mcp_to_latest(&mut mcp)
        .await
        .unwrap();

    let after = server.install(&mcp);
    assert_ne!(before, after);
    assert_eq!(
        lines(&after.join("install.argv")).last().unwrap(),
        "@example/fake-mcp@latest"
    );
    assert_eq!(
        server
            .upstream
            .installed_npm_package_version(&mcp)
            .as_deref(),
        Some("1.1.0")
    );
    assert_eq!(mcp.status, McpStatus::Ready);
    let saved = find_mcp(&server.core, mcp.id).await;
    assert_eq!(saved.status, McpStatus::Ready);
    assert_eq!(saved.last_error, None);
    assert_eq!(saved.npm_version, None);

    let result = server.upstream.update_latest_tracking_mcps().await.unwrap();
    assert_eq!((result.updated, result.skipped), (1, 0));
    assert!(result.failed.is_empty());

    // Deleting the MCP's files takes its installs with its sandbox.
    server.upstream.remove_mcp_sandbox(mcp.id).await.unwrap();
    assert!(!server.sandbox(&mcp).exists());
    assert_eq!(server.upstream.installed_npm_package_version(&mcp), None);
}

// From `tests/unit/security.spec.ts`: redacts long npm environment secrets
// before truncating startup errors.
#[tokio::test]
async fn saves_why_an_npm_mcp_did_not_start_without_the_secrets_it_echoed() {
    let server = Server::new().await;
    let secret = format!("opaque-{}-tail", "x".repeat(400));
    let mut mcp = server
        .npm_mcp(&[("FAKE_BEHAVIOUR", "refuse-key"), ("API_KEY", &secret)])
        .await;

    server
        .upstream
        .test_and_update_status(&mut mcp)
        .await
        .unwrap();

    assert_eq!(mcp.status, McpStatus::Error);
    assert!(!mcp.oauth_required);
    let last_error = mcp.last_error.clone().unwrap();
    assert!(
        last_error.starts_with("Failed to start npm MCP \"@example/fake-mcp\"."),
        "{last_error}"
    );
    assert!(last_error.contains("[REDACTED]"), "{last_error}");
    assert!(!last_error.contains(&secret));
    assert!(!last_error.contains(&secret[..300]));
    assert_eq!(
        find_mcp(&server.core, mcp.id).await.last_error,
        mcp.last_error
    );
}
