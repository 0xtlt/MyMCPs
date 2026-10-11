//! The functional specs of the Node app whose cases go through `/mcp`, at
//! the level of [`McpGateway::handle`](mymcps_gateway::McpGateway::handle):
//! one module for each spec, with the MCPs behind the gateway answered by
//! the test, as the TypeScript tests replaced `fetch`.

#[path = "../support/mod.rs"]
mod support;

mod gateway_auth;
mod gateway_lazy;
mod hardening_gateway_mcp;
mod mcp_call_logging;
mod tool_approvals;
mod vine_gateway_input;
