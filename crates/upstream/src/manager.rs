//! One way to list, call and test an MCP, whatever it runs on.

use std::collections::HashMap;

use mymcps_core::models::{Mcp, McpAuthType, McpTransport, ProbeOutcome};
use mymcps_core::redaction::sanitize_mcp_diagnostic;
use mymcps_mcp::client::ClientError;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::Upstream;
use crate::approvals::with_approval_notes;
use crate::error::UpstreamError;
use crate::http_client::UpstreamTool;
use crate::validators::NAMESPACED_TOOL_VALIDATOR;

/// A tool of the eager gateway: an upstream tool with the MCP it belongs to.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NamespacedTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: Value,
    pub mcp_id: i64,
    pub mcp_slug: String,
    pub namespaced_name: String,
}

/// The two halves of a tool name of the eager gateway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamespacedToolName {
    pub slug: String,
    pub tool_name: String,
}

pub fn namespace_tool(slug: &str, tool_name: &str) -> String {
    format!("{slug}__{tool_name}")
}

/// Split `<slug>__<tool>` at its first separator. `None` for a name that has
/// no separator, or no slug before it.
pub fn parse_namespaced_tool(namespaced: &str) -> Option<NamespacedToolName> {
    NAMESPACED_TOOL_VALIDATOR
        .validate_as(&Value::from(namespaced))
        .ok()
}

impl Upstream {
    fn diagnostic(&self, error: &dyn std::fmt::Display, mcp: &Mcp) -> String {
        sanitize_mcp_diagnostic(&self.core.encryption, &error.to_string(), mcp)
    }

    /// The tools an MCP has right now. Connects to it, unless it is built in.
    ///
    /// `mcp` is read again when its OAuth access token had to be renewed.
    pub async fn probe(&self, mcp: &mut Mcp) -> Result<Vec<UpstreamTool>, UpstreamError> {
        let turn = self.tool_counts().start();
        let listed = self.list_tools(mcp).await;
        // What it listed before says nothing about it now when it fails.
        let count = listed.as_ref().ok().map(Vec::len);
        self.tool_counts().finish(mcp.id, turn, count);
        listed
    }

    fn tool_counts(&self) -> std::sync::MutexGuard<'_, ToolCounts> {
        self.known_tool_counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Forget what an MCP that was deleted listed.
    pub fn forget_tool_count(&self, mcp_id: i64) {
        self.tool_counts().counts.remove(&mcp_id);
    }

    async fn list_tools(&self, mcp: &mut Mcp) -> Result<Vec<UpstreamTool>, UpstreamError> {
        match mcp.transport {
            McpTransport::Http => self.list_http_tools(mcp).await,
            McpTransport::Npm => Ok(self
                .node
                .list_tools(mcp)
                .await?
                .iter()
                .map(UpstreamTool::from)
                .collect()),
            McpTransport::Builtin => Ok(self.list_builtin_tools(mcp)?),
        }
    }

    /// How many tools each MCP had the last time it was asked for them since
    /// the server started, by MCP id. Nothing is stored: an MCP that was not
    /// asked yet, or that could not answer the last time, is absent.
    pub fn known_tool_counts(&self) -> HashMap<i64, usize> {
        self.tool_counts().known()
    }

    /// Every tool of these MCPs, under its name in the eager gateway. An MCP
    /// that cannot be listed is left out.
    pub async fn list_namespaced_tools(&self, mcps: &mut [Mcp]) -> Vec<NamespacedTool> {
        let mut tools = Vec::new();

        for mcp in mcps {
            match self.probe(mcp).await {
                Ok(listed) => {
                    for tool in with_approval_notes(&self.builtins, mcp, listed) {
                        tools.push(NamespacedTool {
                            namespaced_name: namespace_tool(&mcp.slug, &tool.name),
                            name: tool.name,
                            description: tool.description,
                            input_schema: tool.input_schema,
                            mcp_id: mcp.id,
                            mcp_slug: mcp.slug.clone(),
                        });
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        error = %self.diagnostic(&error, mcp),
                        mcp_id = mcp.id,
                        slug = %mcp.slug,
                        "Skipping unhealthy upstream while listing gateway tools"
                    );
                }
            }
        }

        tools
    }

    /// Call a tool of an MCP and return its result as the MCP gave it: a
    /// result with `isError` is a result, not an error.
    ///
    /// An HTTP or npm MCP has [`Mcp::tool_timeout`] to answer the call, after
    /// it connected or started. Without an answer by then the call fails with
    /// [`UpstreamError::ToolTimeout`].
    pub async fn call_tool(
        &self,
        mcp: &mut Mcp,
        tool_name: &str,
        arguments: Option<Map<String, Value>>,
    ) -> Result<Value, UpstreamError> {
        let timeout = mcp.tool_timeout();
        let timed_out = |transport| UpstreamError::ToolTimeout { transport, timeout };
        match mcp.transport {
            McpTransport::Builtin => Ok(self.call_builtin_tool(mcp, tool_name, arguments).await?),
            McpTransport::Http => {
                let connected = self.connect_http_upstream(mcp).await?;
                let result = connected
                    .client
                    .call_tool_within(tool_name, Some(arguments.unwrap_or_default()), timeout)
                    .await;
                connected.close().await;
                result.map_err(|error| match &error {
                    ClientError::Mcp(answer) if answer.is_timeout_after(timeout) => {
                        timed_out(McpTransport::Http)
                    }
                    _ => error.into(),
                })
            }
            McpTransport::Npm => self
                .node
                .call_tool(mcp, tool_name, arguments.unwrap_or_default())
                .await
                .map_err(|error| match error.mcp_error() {
                    Some(answer) if answer.is_timeout_after(timeout) => {
                        timed_out(McpTransport::Npm)
                    }
                    _ => error.into(),
                }),
        }
    }

    /// Listing built-in tools is local, so health comes from one authenticated
    /// provider request instead.
    async fn probe_builtin(&self, mcp: &mut Mcp, held_token: bool) -> ProbeOutcome {
        match self.verify_builtin(mcp).await {
            Ok(()) => ProbeOutcome::Reached,
            // Connect only repairs an OAuth sign-in. A rejected password is
            // an error to fix in the form.
            Err(error)
                if error.is_authorization_error()
                    && self
                        .builtin_mcp(mcp.builtin_key.as_deref())
                        .is_some_and(|definition| definition.oauth().is_some()) =>
            {
                ProbeOutcome::SignInRefused {
                    held_token,
                    reason: self.diagnostic(&error, mcp),
                }
            }
            Err(error) => ProbeOutcome::Failed(self.diagnostic(&error, mcp)),
        }
    }

    /// What connecting to an HTTP or npm MCP found. `None`: a renewal of its
    /// sign-in was dropped because the MCP changed meanwhile, which says
    /// nothing of the MCP as it is now.
    async fn probe_connection(&self, mcp: &mut Mcp, held_token: bool) -> Option<ProbeOutcome> {
        Some(match self.probe(mcp).await {
            Err(error) if error.is_superseded() => return None,
            Ok(_) => ProbeOutcome::Reached,
            // Only an MCP that signs in by itself can be given a sign-in by
            // Connect. A static credential that is refused is an error to
            // fix in the form.
            Err(error)
                if mcp.transport == McpTransport::Http
                    && mcp.auth_type == McpAuthType::Auto
                    && error.is_unauthorized() =>
            {
                let reason = if let UpstreamError::Unauthorized(details) = &error {
                    self.diagnostic(&format!("OAuth token rejected. {details}"), mcp)
                } else {
                    "OAuth token was rejected by the MCP server (HTTP 401). Re-authorize this MCP."
                        .to_owned()
                };
                ProbeOutcome::SignInRefused { held_token, reason }
            }
            Err(error) => ProbeOutcome::Failed(self.diagnostic(&error, mcp)),
        })
    }

    /// Test the connection to an MCP and save what was found
    /// ([`Mcp::mark_probed`]): its status, the reason when it cannot be used,
    /// and whether it waits for an OAuth authorization.
    ///
    /// The result is saved only if the MCP is still what the test started
    /// from ([`Mcp::save_probe_result`]): one that was edited, signed in
    /// again or signed out while the test ran is read again instead, and
    /// keeps what that change left. A test of a stale copy is dropped the
    /// same way.
    ///
    /// Fails only when the row cannot be saved.
    pub async fn test_and_update_status(&self, mcp: &mut Mcp) -> Result<(), UpstreamError> {
        let basis = mcp.probe_basis();
        let held_token = self
            .core
            .decrypt_secret(mcp.oauth_access_token.as_deref())
            .is_some();
        let outcome = if mcp.transport == McpTransport::Builtin {
            self.probe_builtin(mcp, held_token).await
        } else {
            match self.probe_connection(mcp, held_token).await {
                Some(outcome) => outcome,
                None => return Ok(()),
            }
        };
        mcp.mark_probed(outcome);
        let basis = basis.after_renewal(mcp);
        if !mcp.save_probe_result(&*self.core.db, &basis).await? {
            tracing::debug!(
                mcp_id = mcp.id,
                "Dropped the result of a test of an MCP that changed while it ran"
            );
            // Gone meanwhile: nothing is left to report on.
            let _ = self.reload(mcp).await;
        }
        Ok(())
    }
}

/// How many tools each MCP listed the last time it was asked, by MCP id,
/// with the turn of the listing that said so: a listing that started before
/// another, and finished after it, does not replace what it found.
#[derive(Debug, Default)]
pub(crate) struct ToolCounts {
    turns: u64,
    counts: HashMap<i64, (u64, Option<usize>)>,
}

impl ToolCounts {
    fn start(&mut self) -> u64 {
        self.turns += 1;
        self.turns
    }

    fn finish(&mut self, mcp_id: i64, turn: u64, count: Option<usize>) {
        match self.counts.get(&mcp_id) {
            Some((latest, _)) if *latest > turn => {}
            _ => {
                self.counts.insert(mcp_id, (turn, count));
            }
        }
    }

    fn known(&self) -> HashMap<i64, usize> {
        self.counts
            .iter()
            .filter_map(|(id, (_, count))| count.map(|count| (*id, count)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_that_started_earlier_does_not_replace_a_later_one() {
        let mut counts = ToolCounts::default();
        let slow = counts.start();
        let fast = counts.start();
        counts.finish(1, fast, Some(3));
        counts.finish(1, slow, Some(9));
        assert_eq!(counts.known().get(&1), Some(&3));
        // A later failure says nothing is known any more.
        let failed = counts.start();
        counts.finish(1, failed, None);
        assert_eq!(counts.known().get(&1), None);
        counts.finish(2, counts.turns, Some(1));
        counts.counts.remove(&2);
        assert!(counts.known().is_empty());
    }

    #[test]
    fn splits_a_name_at_its_first_separator() {
        let long = "x".repeat(4000);
        for (name, slug, tool_name) in [
            ("weather__get_forecast", "weather", "get_forecast"),
            ("weather__get__forecast", "weather", "get__forecast"),
            ("weather___get_forecast", "weather", "_get_forecast"),
            ("_weather__get_forecast", "_weather", "get_forecast"),
            ("weather__", "weather", ""),
            (" __tool", " ", "tool"),
            ("Not A Slug__tool", "Not A Slug", "tool"),
            ("multi\nline__tool\nname", "multi\nline", "tool\nname"),
            (format!("{long}__tool").as_str(), long.as_str(), "tool"),
        ] {
            assert_eq!(
                parse_namespaced_tool(name),
                Some(NamespacedToolName {
                    slug: slug.to_owned(),
                    tool_name: tool_name.to_owned(),
                }),
                "{name:?}"
            );
            assert_eq!(
                NAMESPACED_TOOL_VALIDATOR
                    .validate(&Value::from(name))
                    .unwrap(),
                serde_json::json!({ "slug": slug, "toolName": tool_name })
            );
            assert_eq!(namespace_tool(slug, tool_name), name);
        }
    }

    #[test]
    fn refuses_a_name_without_a_separator_or_without_a_slug_before_it() {
        for name in [
            "",
            " ",
            "_",
            "__",
            "___",
            "__tool",
            "___tool",
            "__weather__tool",
            "weather",
            "weather_tool",
            "weather_",
        ] {
            assert_eq!(parse_namespaced_tool(name), None, "{name:?}");
        }
        // Values that stand where a string is expected without being one.
        use serde_json::json;
        for name in [
            json!(null),
            json!(0),
            json!(1),
            json!(true),
            json!(["issues"]),
            json!([]),
            json!({}),
            json!({ "slug": "issues" }),
        ] {
            assert!(NAMESPACED_TOOL_VALIDATOR.validate(&name).is_err(), "{name}");
        }
        assert!(NAMESPACED_TOOL_VALIDATOR.validate(None::<&Value>).is_err());
    }
}
