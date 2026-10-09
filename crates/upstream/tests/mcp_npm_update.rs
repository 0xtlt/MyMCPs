//! `tests/unit/mcp_npm_update.spec.ts`: updating the npm MCPs that track
//! `latest`, with the two steps of an update replaced as the TypeScript tests
//! replaced `mcpNpmUpdateRuntime`.

mod support;

use std::sync::{Arc, Mutex};

use futures::future::join;
use mymcps_core::models::{Mcp, McpStatus, McpTransport};
use mymcps_core::secrets::{EnvironmentInput, merge_environment};
use mymcps_core::{Core, TestCore};
use mymcps_upstream::{McpNpmUpdateFailure, NpmUpdateRuntime, Upstream, UpstreamError};
use support::*;

/// Reloads nothing and finds every MCP ready, recording what it was asked.
struct Runtime {
    core: Arc<Core>,
    reloaded: Arc<Mutex<Vec<i64>>>,
    /// The reload of this package fails, quoting the environment of the MCP.
    failing_package: Option<&'static str>,
}

#[async_trait::async_trait]
impl NpmUpdateRuntime for Runtime {
    async fn reload(&self, mcp: &Mcp) -> Result<(), UpstreamError> {
        // Let a call made at the same moment find this run under way.
        tokio::task::yield_now().await;
        self.reloaded.lock().unwrap().push(mcp.id);
        if mcp.npm_package.as_deref() == self.failing_package {
            let environment = mcp.npm_environment(&self.core.encryption).unwrap();
            return Err(mymcps_deno::DenoError::CacheReload {
                package: mcp.npm_package.clone().unwrap(),
                detail: format!("error: registry refused the token {}", environment[0].1),
            }
            .into());
        }
        Ok(())
    }

    async fn probe(&self, mcp: &mut Mcp) -> Result<(), UpstreamError> {
        mcp.status = McpStatus::Ready;
        mcp.last_error = None;
        mcp.save(&*self.core.db).await?;
        Ok(())
    }
}

fn update_runtime(
    core: &TestCore,
    failing_package: Option<&'static str>,
) -> (Arc<Upstream>, Arc<Mutex<Vec<i64>>>) {
    let reloaded: Arc<Mutex<Vec<i64>>> = Arc::default();
    let upstream = Upstream::builder(core.core.clone(), Default::default())
        .npm_update_runtime(Runtime {
            core: core.core.clone(),
            reloaded: reloaded.clone(),
            failing_package,
        })
        .build();
    (upstream, reloaded)
}

async fn npm_mcp(core: &TestCore, name: &str, package: &str, version: Option<&str>) -> Mcp {
    create_mcp(core, |mcp| {
        mcp.name = name.to_owned();
        mcp.transport = McpTransport::Npm;
        mcp.npm_package = Some(package.to_owned());
        mcp.npm_version = version.map(str::to_owned);
        mcp.status = McpStatus::Draft;
    })
    .await
}

#[tokio::test]
async fn reloads_a_latest_tracking_npm_mcp_without_rewriting_npm_version() {
    let core = TestCore::new().await;
    let (upstream, reloaded) = update_runtime(&core, None);
    let mut mcp = npm_mcp(&core, "Latest", "@example/latest-mcp", Some("latest")).await;

    upstream.update_mcp_to_latest(&mut mcp).await.unwrap();
    let saved = find_mcp(&core, mcp.id).await;

    assert_eq!(*reloaded.lock().unwrap(), [mcp.id]);
    assert_eq!(saved.npm_version.as_deref(), Some("latest"));
    assert_eq!(saved.status, McpStatus::Ready);
    assert_eq!(mcp.status, McpStatus::Ready);
}

#[tokio::test]
async fn refuses_http_mcps_and_pinned_npm_versions_without_calling_reload() {
    let core = TestCore::new().await;
    let (upstream, reloaded) = update_runtime(&core, None);
    let mut http_mcp = create_mcp(&core, |_| {}).await;
    let mut pinned = npm_mcp(&core, "Pinned", "@example/pinned-mcp", Some("1.14.4")).await;
    let mut unnamed = npm_mcp(&core, "Unnamed", "  ", None).await;

    let refusal = upstream
        .update_mcp_to_latest(&mut http_mcp)
        .await
        .unwrap_err();
    assert!(refusal.is_npm_update_error());
    assert_eq!(refusal.to_string(), "Only Deno npm MCPs can be updated");
    let refusal = upstream
        .update_mcp_to_latest(&mut pinned)
        .await
        .unwrap_err();
    assert!(refusal.is_npm_update_error());
    assert_eq!(refusal.to_string(), "Pinned npm versions are not updated");
    let refusal = upstream
        .update_mcp_to_latest(&mut unnamed)
        .await
        .unwrap_err();
    assert!(refusal.is_npm_update_error());
    assert_eq!(refusal.to_string(), "npm MCP is missing a package name");

    let saved = find_mcp(&core, pinned.id).await;
    assert_eq!(saved.npm_version.as_deref(), Some("1.14.4"));
    assert!(reloaded.lock().unwrap().is_empty());
}

#[tokio::test]
async fn batch_updates_only_latest_tracking_npm_mcps() {
    let core = TestCore::new().await;
    let (upstream, reloaded) = update_runtime(&core, None);
    let latest = npm_mcp(&core, "Latest MCP", "@example/latest-mcp", None).await;
    let pinned = npm_mcp(&core, "Pinned MCP", "@example/pinned-mcp", Some("2.0.0")).await;
    create_mcp(&core, |mcp| mcp.name = "HTTP MCP".into()).await;

    let result = upstream.update_latest_tracking_mcps().await.unwrap();

    assert_eq!(*reloaded.lock().unwrap(), [latest.id]);
    assert_eq!(result.updated, 1);
    assert_eq!(result.skipped, 1);
    assert!(result.failed.is_empty());

    assert_eq!(
        find_mcp(&core, pinned.id).await.npm_version.as_deref(),
        Some("2.0.0")
    );
    let latest = find_mcp(&core, latest.id).await;
    assert_eq!(latest.npm_version, None);
    assert_eq!(latest.status, McpStatus::Ready);
}

#[tokio::test]
async fn a_batch_goes_on_past_an_mcp_that_fails_and_reports_it_without_its_secrets() {
    let core = TestCore::new().await;
    let (upstream, reloaded) = update_runtime(&core, Some("@example/broken-mcp"));
    let environment = merge_environment(
        &core.encryption,
        None,
        &[EnvironmentInput {
            name: "REGISTRY_TOKEN".into(),
            value: Some("npm_secret-registry-token".into()),
        }],
    );
    let broken = create_mcp(&core, |mcp| {
        mcp.name = "Broken MCP".into();
        mcp.transport = McpTransport::Npm;
        mcp.npm_package = Some("@example/broken-mcp".into());
        mcp.npm_version = Some("Latest".into());
        mcp.npm_env = environment;
    })
    .await;
    let unnamed = npm_mcp(&core, "Unnamed MCP", "", None).await;
    let healthy = npm_mcp(&core, "Healthy MCP", "@example/healthy-mcp", Some(" ")).await;

    let result = upstream.update_latest_tracking_mcps().await.unwrap();

    assert_eq!(*reloaded.lock().unwrap(), [broken.id, healthy.id]);
    assert_eq!((result.updated, result.skipped), (1, 0));
    assert_eq!(
        result.failed,
        [
            McpNpmUpdateFailure {
                id: broken.id,
                slug: "broken-mcp".into(),
                error: "Failed to reload Deno cache for \"@example/broken-mcp\". error: registry refused the token [REDACTED]".into(),
            },
            McpNpmUpdateFailure {
                id: unnamed.id,
                slug: "unnamed-mcp".into(),
                error: "npm MCP is missing a package name".into(),
            },
        ]
    );
}

#[tokio::test]
async fn calls_made_while_a_batch_runs_share_that_run() {
    let core = TestCore::new().await;
    let (upstream, reloaded) = update_runtime(&core, None);
    let latest = npm_mcp(&core, "Latest MCP", "@example/latest-mcp", None).await;

    let (first, second) = join(
        upstream.update_latest_tracking_mcps(),
        upstream.update_latest_tracking_mcps(),
    )
    .await;
    assert_eq!(first.unwrap(), second.unwrap());
    assert_eq!(*reloaded.lock().unwrap(), [latest.id]);

    // A run that ended is not the answer to a later call.
    upstream.update_latest_tracking_mcps().await.unwrap();
    assert_eq!(*reloaded.lock().unwrap(), [latest.id, latest.id]);
}
