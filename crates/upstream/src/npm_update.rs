//! Updating the npm MCPs that follow the latest version of their package.

use std::sync::PoisonError;

use async_trait::async_trait;
use mymcps_core::models::{Mcp, McpTransport};
use mymcps_core::redaction::sanitize_mcp_diagnostic;
use mymcps_vine::js;
use serde::Serialize;

use crate::Upstream;
use crate::error::UpstreamError;
use crate::shared::{SharedOutcome, outcome_channel};

/// An MCP that cannot be updated. The message is shown as it is.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct McpNpmUpdateError(String);

fn refusal(message: &str) -> UpstreamError {
    UpstreamError::NpmUpdate(McpNpmUpdateError(message.to_owned()))
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct McpNpmUpdateRunResult {
    pub updated: u32,
    pub skipped: u32,
    pub failed: Vec<McpNpmUpdateFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct McpNpmUpdateFailure {
    pub id: i64,
    pub slug: String,
    pub error: String,
}

/// The two steps of an update, for a test to replace them
/// ([`crate::UpstreamBuilder::npm_update_runtime`]). The server reloads the
/// Deno cache of the package, then tests the MCP and saves its status.
#[async_trait]
pub trait NpmUpdateRuntime: Send + Sync + 'static {
    async fn reload(&self, mcp: &Mcp) -> Result<(), UpstreamError>;
    async fn probe(&self, mcp: &mut Mcp) -> Result<(), UpstreamError>;
}

pub(crate) type PendingNpmUpdates = SharedOutcome<McpNpmUpdateRunResult>;

pub fn is_latest_npm_version(npm_version: Option<&str>) -> bool {
    let version = js::trim(npm_version.unwrap_or_default());
    version.is_empty() || version.to_lowercase() == "latest"
}

pub fn is_tracking_latest(transport: McpTransport, npm_version: Option<&str>) -> bool {
    transport == McpTransport::Npm && is_latest_npm_version(npm_version)
}

impl Upstream {
    async fn reload_npm_package(&self, mcp: &Mcp) -> Result<(), UpstreamError> {
        match &self.npm_update_runtime {
            Some(runtime) => runtime.reload(mcp).await,
            None => Ok(self.deno.reload_npm_package_cache(mcp).await?),
        }
    }

    async fn probe_updated(&self, mcp: &mut Mcp) -> Result<(), UpstreamError> {
        match &self.npm_update_runtime {
            Some(runtime) => runtime.probe(mcp).await,
            None => self.test_and_update_status(mcp).await,
        }
    }

    /// Reload the Deno cache of an npm MCP that tracks `latest`, then test it
    /// and save its status. The version saved for the MCP is not changed.
    pub async fn update_mcp_to_latest(&self, mcp: &mut Mcp) -> Result<(), UpstreamError> {
        if mcp.transport != McpTransport::Npm {
            return Err(refusal("Only Deno npm MCPs can be updated"));
        }
        if !is_tracking_latest(mcp.transport, mcp.npm_version.as_deref()) {
            return Err(refusal("Pinned npm versions are not updated"));
        }
        if js::trim(mcp.npm_package.as_deref().unwrap_or_default()).is_empty() {
            return Err(refusal("npm MCP is missing a package name"));
        }

        self.reload_npm_package(mcp).await?;
        self.probe_updated(mcp).await
    }

    async fn run_latest_tracking_updates(&self) -> Result<McpNpmUpdateRunResult, UpstreamError> {
        let mcps: Vec<Mcp> =
            sqlx::query_as("select * from `mcps` where `transport` = ? order by `id` asc")
                .bind(McpTransport::Npm)
                .fetch_all(&*self.core.db)
                .await?;
        let mut result = McpNpmUpdateRunResult::default();

        for mut mcp in mcps {
            if !is_tracking_latest(mcp.transport, mcp.npm_version.as_deref()) {
                result.skipped += 1;
                continue;
            }

            match self.update_mcp_to_latest(&mut mcp).await {
                Ok(()) => result.updated += 1,
                Err(error) => {
                    let message = match &error {
                        UpstreamError::NpmUpdate(refusal) => refusal.to_string(),
                        other => {
                            sanitize_mcp_diagnostic(&self.core.encryption, &other.to_string(), &mcp)
                        }
                    };
                    tracing::warn!(
                        mcp_id = mcp.id,
                        slug = %mcp.slug,
                        error = %message,
                        "Failed to update latest-tracking npm MCP"
                    );
                    result.failed.push(McpNpmUpdateFailure {
                        id: mcp.id,
                        slug: mcp.slug.clone(),
                        error: message,
                    });
                }
            }
        }

        Ok(result)
    }

    /// Reload Deno caches for npm MCPs that already track `latest`. Pinned versions are skipped.
    ///
    /// One run at a time: a call made while a run is under way waits for
    /// that run and returns its result. The run goes on to its end even if
    /// every caller stops waiting.
    pub async fn update_latest_tracking_mcps(
        &self,
    ) -> Result<McpNpmUpdateRunResult, UpstreamError> {
        /// Lets the next call start a run, however this one ended.
        struct Release(Upstream);

        impl Drop for Release {
            fn drop(&mut self) {
                *self
                    .0
                    .pending_npm_updates
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = None;
            }
        }

        let (outcome, run) = {
            let mut in_flight = self
                .pending_npm_updates
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            match &*in_flight {
                Some(run) => (None, run.clone()),
                None => {
                    let (outcome, run) = outcome_channel("The update of npm MCPs was interrupted");
                    *in_flight = Some(run.clone());
                    (Some(outcome), run)
                }
            }
        };
        if let Some(outcome) = outcome {
            let release = Release(self.clone());
            tokio::spawn(async move {
                let result = release.0.run_latest_tracking_updates().await;
                drop(release);
                // Nobody may be waiting any more.
                let _ = outcome.send(result);
            });
        }
        run.await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn treats_empty_null_and_latest_as_tracking_latest() {
        assert!(is_latest_npm_version(None));
        assert!(is_latest_npm_version(Some("")));
        assert!(is_latest_npm_version(Some("  ")));
        assert!(is_latest_npm_version(Some("latest")));
        assert!(is_latest_npm_version(Some("Latest")));
        assert!(is_latest_npm_version(Some(" LATEST\n")));
        assert!(!is_latest_npm_version(Some("1.2.3")));
        assert!(!is_latest_npm_version(Some("1.14.4")));
        assert!(!is_latest_npm_version(Some("latest-1")));
    }

    #[test]
    fn only_npm_mcps_without_a_pin_track_latest() {
        assert!(is_tracking_latest(McpTransport::Npm, None));
        assert!(is_tracking_latest(McpTransport::Npm, Some("latest")));
        assert!(!is_tracking_latest(McpTransport::Npm, Some("1.2.3")));
        assert!(!is_tracking_latest(McpTransport::Http, None));
        assert!(!is_tracking_latest(McpTransport::Http, Some("latest")));
        assert!(!is_tracking_latest(McpTransport::Builtin, None));
    }
}
