//! One way to list, call and test an MCP, whatever it runs on.

use std::collections::HashMap;

use mymcps_core::Timestamp;
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

/// What an agent is told when a call or a tool search failed because the
/// MCP waits for its OAuth sign-in in MyMCPs. `None` when it failed for
/// another reason.
pub fn authorization_needed_message(mcp: &Mcp, error: &UpstreamError) -> Option<String> {
    if mcp.transport != McpTransport::Http
        || mcp.auth_type != McpAuthType::Auto
        || !error.needs_authorization()
    {
        return None;
    }
    let has_access_token = mcp
        .oauth_access_token
        .as_deref()
        .is_some_and(|token| !token.is_empty());
    let slug = &mcp.slug;
    Some(if has_access_token {
        format!(
            "MCP \"{slug}\" needs to be re-authorized in MyMCPs: its OAuth sign-in has expired or was revoked. Re-authorize it on the MCPs page of MyMCPs, then try again."
        )
    } else {
        format!(
            "MCP \"{slug}\" needs to be connected in MyMCPs: it signs in with OAuth and no account is connected yet. Connect it on the MCPs page of MyMCPs, then try again."
        )
    })
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
    /// An HTTP MCP whose OAuth sign-in no longer works is marked as waiting
    /// to be authorized again.
    ///
    /// `mcp` is read again when its OAuth access token had to be renewed, or
    /// when the mark changed.
    pub async fn probe(&self, mcp: &mut Mcp) -> Result<Vec<UpstreamTool>, UpstreamError> {
        let listed = self.count_tools(mcp).await;
        if let Err(error) = &listed {
            self.note_sign_in(mcp, Some(error)).await;
        }
        listed
    }

    /// The tools an MCP has right now, with their count remembered for
    /// [`Upstream::known_tool_counts`]. Unlike [`Upstream::probe`], it saves
    /// nothing of the sign-in.
    async fn count_tools(&self, mcp: &mut Mcp) -> Result<Vec<UpstreamTool>, UpstreamError> {
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
                let result = self.call_http_tool(mcp, tool_name, arguments).await;
                self.note_sign_in(mcp, result.as_ref().err()).await;
                result
            }
            McpTransport::Npm => Ok(self
                .node
                .call_tool(mcp, tool_name, arguments.unwrap_or_default())
                .await?),
        }
    }

    async fn call_http_tool(
        &self,
        mcp: &mut Mcp,
        tool_name: &str,
        arguments: Option<Map<String, Value>>,
    ) -> Result<Value, UpstreamError> {
        let connected = self.connect_http_upstream(mcp).await?;
        let result = connected
            .client
            .call_tool(tool_name, Some(arguments.unwrap_or_default()))
            .await;
        connected.close().await;
        Ok(result?)
    }

    /// Why the OAuth sign-in of an HTTP MCP no longer works, when that is what
    /// `error` says: what `last_error` holds while the MCP waits to be
    /// authorized again. `None` for another failure, or an MCP without an
    /// OAuth access token.
    fn lapsed_sign_in(&self, mcp: &Mcp, error: &UpstreamError) -> Option<String> {
        if mcp.transport != McpTransport::Http
            || mcp.auth_type != McpAuthType::Auto
            || !error.needs_authorization()
            || self
                .core
                .decrypt_secret(mcp.oauth_access_token.as_deref())
                .is_none()
        {
            return None;
        }
        Some(match error {
            UpstreamError::OAuthSignInExpired(_) => error.to_string(),
            UpstreamError::Unauthorized(details) => {
                self.diagnostic(&format!("OAuth token rejected. {details}"), mcp)
            }
            _ => match error.refused_renewal() {
                Some(refused) if refused.message().is_empty() => {
                    self.diagnostic(&"OAuth refresh token rejected (invalid_grant).", mcp)
                }
                Some(refused) => self.diagnostic(
                    &format!("OAuth refresh token rejected (invalid_grant): {refused}"),
                    mcp,
                ),
                None => {
                    "OAuth token was rejected by the MCP server (HTTP 401). Re-authorize this MCP."
                        .to_owned()
                }
            },
        })
    }

    /// Save what a request to an HTTP MCP showed of its OAuth sign-in, so the
    /// pages show it without a test: a lapsed sign-in marks the MCP for
    /// re-authorization, and a tool call that went through clears that mark.
    ///
    /// Only the sign-in that was used is changed: the update is conditioned on
    /// the saved access token, so a row authorized again in the meantime keeps
    /// what its new authorization saved. A failure to save is logged, never
    /// returned: the request goes on.
    async fn note_sign_in(&self, mcp: &mut Mcp, failure: Option<&UpstreamError>) {
        if mcp.transport != McpTransport::Http || mcp.auth_type != McpAuthType::Auto {
            return;
        }
        let (status, oauth_required, last_error) = match failure {
            Some(error) => match self.lapsed_sign_in(mcp, error) {
                Some(reason) => (McpStatus::Error, true, Some(reason)),
                None => return,
            },
            None => {
                let marked = mcp.status == McpStatus::Error
                    && mcp.oauth_required
                    && mcp
                        .oauth_access_token
                        .as_deref()
                        .is_some_and(|token| !token.is_empty());
                if !marked {
                    return;
                }
                (McpStatus::Ready, false, None)
            }
        };
        if mcp.status == status
            && mcp.oauth_required == oauth_required
            && mcp.last_error == last_error
        {
            return;
        }

        let updated = sqlx::query(
            "update `mcps` set `status` = ?, `oauth_required` = ?, `last_error` = ?, `updated_at` = ? \
             where `id` = ? and `oauth_access_token` = ?",
        )
        .bind(status)
        .bind(oauth_required)
        .bind(&last_error)
        .bind(Timestamp::now())
        .bind(mcp.id)
        .bind(mcp.oauth_access_token.as_deref())
        .execute(&*self.core.db)
        .await;
        let saved = match updated {
            Ok(done) => done.rows_affected() == 1,
            Err(error) => {
                tracing::warn!(
                    %error,
                    mcp_id = mcp.id,
                    slug = %mcp.slug,
                    "Could not save the state of the OAuth sign-in of an MCP"
                );
                return;
            }
        };
        if saved && let Err(error) = self.reload(mcp).await {
            tracing::warn!(
                error = %error,
                mcp_id = mcp.id,
                slug = %mcp.slug,
                "Could not read an MCP again after saving the state of its OAuth sign-in"
            );
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

        match self.count_tools(mcp).await {
            Ok(_) => {
                mcp.status = McpStatus::Ready;
                mcp.last_error = None;
                mcp.oauth_required = false;
            }
            Err(error) => {
                let waits_for_oauth = mcp.transport == McpTransport::Http
                    && mcp.auth_type == McpAuthType::Auto
                    && error.needs_authorization();

                if let Some(reason) = self.lapsed_sign_in(mcp, &error) {
                    mcp.status = McpStatus::Error;
                    mcp.last_error = Some(reason);
                    mcp.oauth_required = true;
                } else if waits_for_oauth {
                    mcp.status = McpStatus::Draft;
                    mcp.last_error = Some("OAuth authorization required".to_owned());
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
