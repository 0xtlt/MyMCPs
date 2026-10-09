//! Agent-facing MCP gateway: aggregate tools from allowed upstreams behind
//! one URL.
//!
//! [`McpGateway::handle`] is everything `/mcp` does with a request once its
//! body is parsed: it authenticates the Bearer access token, holds the token
//! to its request allowance, reads the tool mode the request asks for, and
//! answers as a stateless MCP server whose tools are those of the MCPs the
//! token may use.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use http::header::{CONTENT_TYPE, RETRY_AFTER, WWW_AUTHENTICATE};
use http::{HeaderMap, HeaderValue, Method, Response, StatusCode};
use mymcps_core::models::{
    AccessToken, CallErrorCategory, CallOutcome, GatewayToolMode, InstanceSetting, Mcp,
};
use mymcps_core::redaction::sanitize_mcp_diagnostic;
use mymcps_mcp::Implementation;
use mymcps_mcp::server::{
    HandlerError, RequestBody, ResponseBody, ServerOptions, ToolHandler, handle_request,
};
use mymcps_upstream::approvals::with_approval_notes;
use mymcps_upstream::{Upstream, parse_namespaced_tool};
use mymcps_vine::js::json_stringify;
use serde_json::{Map, Value, json};

use crate::Gateway;
use crate::approvals::{ApprovalGate, ApprovalService, GatedCall};
use crate::auto_update::McpAutoUpdateScheduler;
use crate::bearer::{BearerAccess, BearerError, BearerRejection};
use crate::call_log::{McpCallLogInput, McpCallLogService};
use crate::lazy_tools::{
    LAZY_GATEWAY_TOOLS, lazy_gateway_instructions, mcp_catalog, parse_call_tool_input,
    parse_gateway_tool_mode, parse_tool_search_input, search_upstream_tools,
};

/// The header a request names its tool mode with.
pub const TOOL_MODE_HEADER: &str = "x-mymcps-tool-mode";

/// One request to `/mcp`, as the web server hands it over.
#[derive(Debug)]
pub struct McpRequest<'a> {
    pub method: &'a Method,
    pub headers: &'a HeaderMap,
    /// The body as the pages read theirs: parsed, every string trimmed and
    /// an empty one read as null, under the parameters of the query string.
    /// An empty object for a request without a body.
    pub body: Value,
    /// The address the request came from, for the call log.
    pub caller_ip: Option<String>,
}

/// The gateway with the MCPs behind it: what answers `/mcp`, the approvals
/// its calls wait for, and the job that keeps npm MCPs up to date.
#[derive(Debug)]
pub struct McpGateway {
    pub gateway: Arc<Gateway>,
    pub upstream: Arc<Upstream>,
    pub approvals: ApprovalService,
    pub auto_update: McpAutoUpdateScheduler,
}

impl McpGateway {
    pub fn new(gateway: Arc<Gateway>, upstream: Arc<Upstream>) -> Self {
        Self {
            approvals: ApprovalService::new(upstream.clone()),
            auto_update: McpAutoUpdateScheduler::new(upstream.clone()),
            gateway,
            upstream,
        }
    }

    /// Answer a request to `/mcp`: authenticate its Bearer access token and
    /// hold it to its request allowance, then
    /// [`handle_authenticated`](Self::handle_authenticated).
    ///
    /// Fails only when the database does.
    pub async fn handle(
        &self,
        request: McpRequest<'_>,
    ) -> Result<Response<ResponseBody>, sqlx::Error> {
        let authorization = first_header_value(request.headers, "authorization");
        match self
            .gateway
            .authenticate_bearer(authorization.as_deref())
            .await
        {
            Ok(access) => self.handle_authenticated(access, request).await,
            Err(BearerError::Rejected(rejection)) => Ok(self.rejection(&rejection)),
            Err(BearerError::Database(error)) => Err(error),
        }
    }

    /// What `/mcp` answers a request whose access token is known.
    pub async fn handle_authenticated(
        &self,
        access: BearerAccess,
        request: McpRequest<'_>,
    ) -> Result<Response<ResponseBody>, sqlx::Error> {
        let settings = InstanceSetting::current(&*self.gateway.core.db).await?;
        let Some(tool_mode) = parse_gateway_tool_mode(
            header_value(request.headers, TOOL_MODE_HEADER).as_deref(),
            settings.gateway_tool_mode,
        ) else {
            return Ok(json_response(
                StatusCode::BAD_REQUEST,
                &json!({
                    "error": "invalid_tool_mode",
                    "message": "X-MyMCPs-Tool-Mode must be either eager or lazy",
                }),
            ));
        };

        let mut options = ServerOptions::new(Implementation::new("mymcps", mymcps_core::VERSION));
        if tool_mode == GatewayToolMode::Lazy {
            options.instructions = Some(lazy_gateway_instructions(&access.allowed_mcps));
        }
        let session = Arc::new(Session {
            upstream: self.upstream.clone(),
            approvals: self.approvals.clone(),
            call_log: self.gateway.call_log.clone(),
            access_token: access.access_token,
            mcps: access.allowed_mcps,
            caller_ip: request.caller_ip,
            tool_mode,
        });

        self.prune_expired();

        Ok(handle_request(
            request.method,
            request.headers,
            RequestBody::Parsed(request.body),
            &options,
            session,
        )
        .await)
    }

    /// Requests are what keeps the tables of the gateway from growing: each
    /// one lets go of what expired, without waiting for it.
    fn prune_expired(&self) {
        let call_log = self.gateway.call_log.clone();
        let approvals = self.approvals.clone();
        tokio::spawn(async move {
            call_log.prune_expired(false).await;
            approvals.prune_expired(false).await;
        });
    }

    fn rejection(&self, rejection: &BearerRejection) -> Response<ResponseBody> {
        let status = StatusCode::from_u16(rejection.status()).unwrap_or(StatusCode::UNAUTHORIZED);
        let mut response = json_response(status, &rejection.body());
        let headers = response.headers_mut();
        if let Some(challenge) = rejection
            .www_authenticate(&self.gateway.core.config)
            .and_then(|challenge| HeaderValue::from_str(&challenge).ok())
        {
            headers.insert(WWW_AUTHENTICATE, challenge);
        }
        if let Some(retry_after) = rejection.retry_after() {
            headers.insert(RETRY_AFTER, HeaderValue::from(retry_after));
        }
        response
    }
}

/// A header as Node hands it to the app when it may be sent more than once:
/// the values joined with `, `, bytes read as Latin-1.
fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let mut values = headers.get_all(name).iter().map(latin1);
    let first = values.next()?;
    Some(values.fold(first, |joined, value| joined + ", " + &value))
}

/// A header of which Node keeps the first value only, as it does of
/// `Authorization`.
fn first_header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers.get(name).map(latin1)
}

fn latin1(value: &HeaderValue) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| char::from(*byte))
        .collect()
}

fn json_response(status: StatusCode, body: &Value) -> Response<ResponseBody> {
    let mut response = Response::new(ResponseBody::Full(Bytes::from(json_stringify(body))));
    *response.status_mut() = status;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json; charset=utf-8"),
    );
    response
}

/// The result of a call the gateway refuses or fails itself: one sentence
/// for the agent.
fn error_result(text: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
}

/// A result an agent reads as text and a program as data.
fn structured_result(content: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": json_stringify(&content) }],
        "structuredContent": content,
    })
}

/// The MCP server one request talks to: the access token it came with, and
/// the MCPs that token may use.
struct Session {
    upstream: Arc<Upstream>,
    approvals: ApprovalService,
    call_log: McpCallLogService,
    access_token: AccessToken,
    mcps: Vec<Mcp>,
    caller_ip: Option<String>,
    tool_mode: GatewayToolMode,
}

/// A tool call the token may make, on its way to its MCP.
struct AllowedCall<'a> {
    mcp: &'a Mcp,
    requested_tool_name: String,
    tool_name: String,
    args: Option<Map<String, Value>>,
    started_at: Instant,
}

impl Session {
    fn mcp(&self, slug: &str) -> Option<&Mcp> {
        self.mcps.iter().rev().find(|mcp| mcp.slug == slug)
    }

    /// Queue the record of a call: what is not given is the same for every
    /// call of this request.
    fn record(&self, started_at: Instant, input: McpCallLogInput) {
        self.call_log.record(McpCallLogInput {
            access_token: self.access_token.clone(),
            caller_ip: self.caller_ip.clone(),
            duration: started_at.elapsed(),
            ..input
        });
    }

    fn record_error(
        &self,
        started_at: Instant,
        category: CallErrorCategory,
        summary: &str,
        input: McpCallLogInput,
    ) {
        self.record(
            started_at,
            McpCallLogInput {
                outcome: CallOutcome::Error,
                error_category: Some(category),
                error_summary: Some(summary.to_owned()),
                ..input
            },
        );
    }

    async fn call_lazy_tool(
        &self,
        requested_tool_name: String,
        args: Option<Map<String, Value>>,
        started_at: Instant,
    ) -> Value {
        match requested_tool_name.as_str() {
            "list_mcps" => structured_result(json!({ "mcps": mcp_catalog(&self.mcps) })),
            "tool_search" => self.search_tools(args).await,
            "call_tool" => {
                let args = args.map(Value::Object);
                let input = match parse_call_tool_input(args.as_ref()) {
                    Ok(input) => input,
                    Err(message) => {
                        self.record_error(
                            started_at,
                            CallErrorCategory::InvalidTool,
                            &message,
                            McpCallLogInput {
                                requested_tool_name,
                                tool_name: None,
                                args: args.clone(),
                                ..Default::default()
                            },
                        );
                        return error_result(&message);
                    }
                };

                let target_tool_name = format!("{}__{}", input.mcp, input.tool);
                let Some(mcp) = self.mcp(&input.mcp) else {
                    self.record_error(
                        started_at,
                        CallErrorCategory::DisallowedMcp,
                        "MCP not allowed for this token",
                        McpCallLogInput {
                            mcp_slug: Some(input.mcp),
                            requested_tool_name: target_tool_name,
                            tool_name: Some(input.tool),
                            args: input.arguments.cloned().map(Value::Object),
                            ..Default::default()
                        },
                    );
                    return error_result("MCP not allowed for this token");
                };

                self.call_and_record(AllowedCall {
                    mcp,
                    requested_tool_name: target_tool_name,
                    tool_name: input.tool,
                    args: input.arguments.cloned(),
                    started_at,
                })
                .await
            }
            _ => error_result("Invalid lazy gateway tool name"),
        }
    }

    async fn search_tools(&self, args: Option<Map<String, Value>>) -> Value {
        let args = args.map(Value::Object);
        let input = match parse_tool_search_input(args.as_ref()) {
            Ok(input) => input,
            Err(message) => return error_result(&message),
        };
        let Some(mcp) = self.mcp(&input.mcp) else {
            return error_result(&format!(
                "MCP \"{}\" is not available to this access token",
                input.mcp
            ));
        };

        let mut mcp = mcp.clone();
        match self.upstream.probe(&mut mcp).await {
            Ok(tools) => {
                let tools = with_approval_notes(self.upstream.builtins(), &mcp, tools);
                let matches = search_upstream_tools(&tools, &input.query, input.limit);
                structured_result(json!({
                    "mcp": mcp_catalog([&mcp]).into_iter().next(),
                    "query": input.query,
                    "tools": matches,
                }))
            }
            Err(error) => {
                tracing::warn!(
                    error = %sanitize_mcp_diagnostic(
                        &self.upstream.core().encryption,
                        &error.to_string(),
                        &mcp,
                    ),
                    mcp_id = mcp.id,
                    slug = %mcp.slug,
                    "Lazy gateway tool search failed"
                );
                error_result(&format!("Unable to search tools for MCP \"{}\"", mcp.slug))
            }
        }
    }

    async fn call_namespaced_tool(
        &self,
        requested_tool_name: String,
        args: Option<Map<String, Value>>,
        started_at: Instant,
    ) -> Value {
        let Some(parsed) = parse_namespaced_tool(&requested_tool_name) else {
            self.record_error(
                started_at,
                CallErrorCategory::InvalidTool,
                "Invalid tool name",
                McpCallLogInput {
                    requested_tool_name,
                    tool_name: None,
                    args: args.map(Value::Object),
                    ..Default::default()
                },
            );
            return error_result("Invalid tool name");
        };

        let Some(mcp) = self.mcp(&parsed.slug) else {
            self.record_error(
                started_at,
                CallErrorCategory::DisallowedMcp,
                "MCP not allowed for this token",
                McpCallLogInput {
                    mcp_slug: Some(parsed.slug),
                    requested_tool_name,
                    tool_name: Some(parsed.tool_name),
                    args: args.map(Value::Object),
                    ..Default::default()
                },
            );
            return error_result("MCP not allowed for this token");
        };

        self.call_and_record(AllowedCall {
            mcp,
            requested_tool_name,
            tool_name: parsed.tool_name,
            args,
            started_at,
        })
        .await
    }

    async fn call_and_record(&self, call: AllowedCall<'_>) -> Value {
        let AllowedCall {
            mcp,
            requested_tool_name,
            tool_name,
            args,
            started_at,
        } = call;
        // The row is this call's own: asking the MCP may read it again.
        let mut mcp = mcp.clone();
        let attempt = self.attempt(&mut mcp, &tool_name, args.as_ref()).await;

        let record = McpCallLogInput {
            mcp: Some(mcp.clone()),
            requested_tool_name,
            tool_name: Some(tool_name.clone()),
            args: args.map(Value::Object),
            ..Default::default()
        };
        match attempt {
            Ok(Attempt::Held(held)) => {
                self.record_error(
                    started_at,
                    held.category,
                    held.reason,
                    McpCallLogInput {
                        response: Some(held.result.clone()),
                        ..record
                    },
                );
                held.result
            }
            Ok(Attempt::Ran(result)) => {
                let is_error = result.get("isError") == Some(&Value::Bool(true));
                let record = McpCallLogInput {
                    response: Some(result.clone()),
                    ..record
                };
                if is_error {
                    self.record_error(
                        started_at,
                        CallErrorCategory::ToolError,
                        "Upstream tool returned an error",
                        record,
                    );
                } else {
                    self.record(
                        started_at,
                        McpCallLogInput {
                            outcome: CallOutcome::Success,
                            ..record
                        },
                    );
                }
                result
            }
            Err(error) => {
                tracing::warn!(
                    error = %sanitize_mcp_diagnostic(
                        &self.upstream.core().encryption,
                        &error,
                        &mcp,
                    ),
                    mcp_id = mcp.id,
                    tool = ?tool_name,
                    "Upstream tool call failed"
                );
                self.record_error(
                    started_at,
                    CallErrorCategory::UpstreamException,
                    &error,
                    record,
                );
                error_result("Upstream tool call failed")
            }
        }
    }

    /// Run a call, unless it has to wait for a person. Fails with the
    /// message of what went wrong, which is redacted before it goes anywhere.
    async fn attempt(
        &self,
        mcp: &mut Mcp,
        tool_name: &str,
        args: Option<&Map<String, Value>>,
    ) -> Result<Attempt, String> {
        // A tool set to ask is held here until a person approved this very call.
        let gate = self
            .approvals
            .gate(GatedCall {
                access_token: &self.access_token,
                mcp,
                tool_name,
                args,
            })
            .await
            .map_err(|error| error.to_string())?;
        if let ApprovalGate::Held(held) = gate {
            return Ok(Attempt::Held(held));
        }

        self.upstream
            .call_tool(mcp, tool_name, args.cloned())
            .await
            .map(Attempt::Ran)
            .map_err(|error| error.to_string())
    }
}

enum Attempt {
    Held(crate::approvals::HeldCall),
    Ran(Value),
}

#[async_trait]
impl ToolHandler for Session {
    // Listing connects to every allowed upstream, so it is left to the one
    // request that asks for the list. A call finds its upstream by slug.
    async fn list_tools(&self) -> Result<Value, HandlerError> {
        if self.tool_mode == GatewayToolMode::Lazy {
            return Ok(json!({ "tools": &*LAZY_GATEWAY_TOOLS }));
        }

        let mut mcps = self.mcps.clone();
        let tools: Vec<Value> = self
            .upstream
            .list_namespaced_tools(&mut mcps)
            .await
            .into_iter()
            .map(|tool| {
                json!({
                    "name": tool.namespaced_name,
                    "description": tool
                        .description
                        .unwrap_or_else(|| format!("Tool {} from {}", tool.name, tool.mcp_slug)),
                    "inputSchema": tool.input_schema,
                })
            })
            .collect();
        Ok(json!({ "tools": tools }))
    }

    async fn call_tool(
        &self,
        name: String,
        arguments: Option<Map<String, Value>>,
    ) -> Result<Value, HandlerError> {
        let started_at = Instant::now();
        Ok(match self.tool_mode {
            GatewayToolMode::Lazy => self.call_lazy_tool(name, arguments, started_at).await,
            GatewayToolMode::Eager => self.call_namespaced_tool(name, arguments, started_at).await,
        })
    }
}
