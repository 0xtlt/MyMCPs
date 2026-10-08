//! npm MCPs through the upstream, with a shell script standing in for the
//! `deno` binary: listing, calling, testing and updating one, and what an
//! administrator reads when it does not start.
#![cfg(unix)]

mod support;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mymcps_core::TestCore;
use mymcps_core::models::{Mcp, McpStatus, McpTransport};
use mymcps_core::secrets::{EnvironmentInput, merge_environment};
use mymcps_deno::{DenoRunner, DenoRuntime, HostEnvironment};
use mymcps_upstream::Upstream;
use serde_json::json;
use support::*;

/// As `deno run` it is a minimal MCP stdio server; as `deno cache` it records
/// its arguments in its working directory and exits.
const FAKE_DENO: &str = r#"#!/bin/sh
if [ "$1" = cache ]; then
  for argument in "$@"; do printf '%s\n' "$argument"; done > reload.argv
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
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"fake-deno","version":"1.0.0"}}}\n' "$id" "$version" ;;
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
        let fake = bin.join("deno");
        std::fs::write(&fake, FAKE_DENO).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir_all(root.join("app")).unwrap();

        let core = TestCore::with_config(|config| {
            config.data_dir = config.data_dir.canonicalize().unwrap();
        })
        .await;
        let host = HostEnvironment {
            path: Some("/usr/bin:/bin".to_owned()),
            deno_dir: Some(root.join("deno-dir").display().to_string()),
            xdg_cache_home: None,
            local_app_data: None,
            home_dir: root.join("home"),
            // The Deno of this machine, if it has one, is never found by accident.
            known_deno_locations: Vec::new(),
            current_dir: root.join("app"),
        };
        let deno = DenoRunner::with_runtime(
            core.core.clone(),
            DenoRuntime::with_host(None, host).binary_path(fake),
        );
        let upstream = Upstream::builder(core.core.clone(), Default::default())
            .deno(deno)
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
        self.upstream.deno().sandbox_root_for(mcp.id)
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
async fn updates_a_latest_tracking_npm_mcp_by_reloading_its_cache_then_testing_it() {
    let server = Server::new().await;
    let mut mcp = server.npm_mcp(&[]).await;

    server
        .upstream
        .update_mcp_to_latest(&mut mcp)
        .await
        .unwrap();

    assert_eq!(
        lines(&server.sandbox(&mcp).join("reload.argv")),
        [
            "cache",
            "--reload",
            "--quiet",
            "--node-modules-dir=none",
            "--no-lock",
            "npm:@example/fake-mcp@latest"
        ]
    );
    assert_eq!(mcp.status, McpStatus::Ready);
    let saved = find_mcp(&server.core, mcp.id).await;
    assert_eq!(saved.status, McpStatus::Ready);
    assert_eq!(saved.last_error, None);
    assert_eq!(saved.npm_version, None);

    let result = server.upstream.update_latest_tracking_mcps().await.unwrap();
    assert_eq!((result.updated, result.skipped), (1, 0));
    assert!(result.failed.is_empty());
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
        last_error.starts_with("Failed to start Deno npm MCP \"@example/fake-mcp\"."),
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
