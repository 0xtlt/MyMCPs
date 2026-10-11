//! One way to list, call and test an MCP, whatever it runs on.

use std::collections::HashMap;

use mymcps_core::models::{Mcp, McpAuthType, McpStatus, McpTransport};
use mymcps_core::redaction::sanitize_mcp_diagnostic;
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
        let listed = self.list_tools(mcp).await;
        let mut counts = self
            .known_tool_counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &listed {
            Ok(tools) => counts.insert(mcp.id, tools.len()),
            // What it listed before says nothing about it now.
            Err(_) => counts.remove(&mcp.id),
        };
        drop(counts);
        listed
    }

    async fn list_tools(&self, mcp: &mut Mcp) -> Result<Vec<UpstreamTool>, UpstreamError> {
        match mcp.transport {
            McpTransport::Http => self.list_http_tools(mcp).await,
            McpTransport::Npm => Ok(self
                .deno
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
        self.known_tool_counts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
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
    pub async fn call_tool(
        &self,
        mcp: &mut Mcp,
        tool_name: &str,
        arguments: Option<Map<String, Value>>,
    ) -> Result<Value, UpstreamError> {
        match mcp.transport {
            McpTransport::Builtin => Ok(self.call_builtin_tool(mcp, tool_name, arguments).await?),
            McpTransport::Http => {
                let connected = self.connect_http_upstream(mcp).await?;
                let result = connected
                    .client
                    .call_tool(tool_name, Some(arguments.unwrap_or_default()))
                    .await;
                connected.close().await;
                Ok(result?)
            }
            McpTransport::Npm => Ok(self
                .deno
                .call_tool(mcp, tool_name, arguments.unwrap_or_default())
                .await?),
        }
    }

    /// Listing built-in tools is local, so health comes from one authenticated
    /// provider request instead.
    async fn test_builtin_and_update_status(&self, mcp: &mut Mcp) -> Result<(), UpstreamError> {
        let had_access_token = self
            .core
            .decrypt_secret(mcp.oauth_access_token.as_deref())
            .is_some();
        match self.verify_builtin(mcp).await {
            Ok(()) => {
                mcp.status = McpStatus::Ready;
                mcp.last_error = None;
                mcp.oauth_required = false;
            }
            Err(error) => {
                // Connect only repairs an OAuth sign-in. A rejected password is an error
                // to fix in the form.
                let authorization_required = error.is_authorization_error()
                    && self
                        .builtin_mcp(mcp.builtin_key.as_deref())
                        .is_some_and(|definition| definition.oauth().is_some());
                if authorization_required && !had_access_token {
                    mcp.status = McpStatus::Draft;
                    mcp.last_error = Some("OAuth authorization required".to_owned());
                } else {
                    mcp.status = McpStatus::Error;
                    mcp.last_error = Some(self.diagnostic(&error, mcp));
                }
                mcp.oauth_required = authorization_required;
            }
        }
        mcp.save(&*self.core.db).await?;
        Ok(())
    }

    /// Test the connection to an MCP and save what was found: its status,
    /// the reason when it cannot be used, and whether it waits for an OAuth
    /// authorization.
    ///
    /// Fails only when the row cannot be saved.
    pub async fn test_and_update_status(&self, mcp: &mut Mcp) -> Result<(), UpstreamError> {
        if mcp.transport == McpTransport::Builtin {
            return self.test_builtin_and_update_status(mcp).await;
        }

        match self.probe(mcp).await {
            Ok(_) => {
                mcp.status = McpStatus::Ready;
                mcp.last_error = None;
                mcp.oauth_required = false;
            }
            Err(error) => {
                let authorization_required = error.is_unauthorized();

                if mcp.transport == McpTransport::Http
                    && mcp.auth_type == McpAuthType::Auto
                    && authorization_required
                {
                    let has_oauth_access_token = self
                        .core
                        .decrypt_secret(mcp.oauth_access_token.as_deref())
                        .is_some();
                    mcp.status = if has_oauth_access_token {
                        McpStatus::Error
                    } else {
                        McpStatus::Draft
                    };
                    mcp.last_error = Some(if !has_oauth_access_token {
                        "OAuth authorization required".to_owned()
                    } else if let UpstreamError::Unauthorized(details) = &error {
                        self.diagnostic(&format!("OAuth token rejected. {details}"), mcp)
                    } else {
                        "OAuth token was rejected by the MCP server (HTTP 401). Re-authorize this MCP."
                            .to_owned()
                    });
                    mcp.oauth_required = true;
                } else {
                    mcp.status = McpStatus::Error;
                    mcp.last_error = Some(self.diagnostic(&error, mcp));
                    mcp.oauth_required = false;
                }
            }
        }
        mcp.save(&*self.core.db).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
