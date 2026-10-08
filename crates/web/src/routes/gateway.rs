//! Agent-facing MCP gateway: bearer access tokens, no session auth.
//! (`mcp_bearer_middleware.ts` and the HTTP side of `gateway_controller.ts`)
//!
//! The gateway crate does the work: it authenticates the Bearer access
//! token, holds it to its request allowance, reads the tool mode and answers
//! as an MCP server. What is left here is what the Node app did before its
//! controller ran: reading the body the way every route read its own.

use std::convert::Infallible;

use axum::Router;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use bytes::Bytes;
use futures::{FutureExt, Stream, StreamExt, stream};
use http::Method;
use mymcps_gateway::McpRequest;
use serde_json::{Map, Value};

use crate::client_ip::client_ip;
use crate::error::AppError;
use crate::input::{media_type, parse_body, parse_query, read_body};
use crate::routes::FeatureRoutes;
use crate::state::AppState;

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        // `get` also answers HEAD, as the router of the Node app did.
        machine: Router::new().route("/mcp", get(handle).post(handle)),
        ..Default::default()
    }
}

/// The body of an answer of the MCP server. One that is whole already (a
/// refusal, the answer to `initialize`) is sent with its length; an event
/// stream whose answers are still being worked out is sent as they come.
fn body_of<S>(mut pieces: S) -> Body
where
    S: Stream<Item = Result<Bytes, Infallible>> + Send + Unpin + 'static,
{
    let first = match pieces.next().now_or_never() {
        Some(Some(Ok(first))) => first,
        Some(_) => return Body::empty(),
        None => return Body::from_stream(pieces),
    };
    match pieces.next().now_or_never() {
        Some(None) => Body::from(first),
        Some(Some(second)) => Body::from_stream(stream::iter([Ok(first), second]).chain(pieces)),
        None => Body::from_stream(stream::iter([Ok(first)]).chain(pieces)),
    }
}

/// `GET` and `POST /mcp`
pub async fn handle(State(state): State<AppState>, request: Request) -> Result<Response, AppError> {
    let (parts, body) = request.into_parts();

    // The body is read before the access token is: a body that is too
    // large, or is not JSON, is refused whoever sends it.
    let mut input = if parts.method == Method::POST {
        let bytes = match read_body(body).await {
            Ok(bytes) => bytes,
            Err(refusal) => return Ok(refusal.into_response()),
        };
        match parse_body(&media_type(&parts.headers), &bytes) {
            Ok(parsed) => parsed,
            Err(status) => {
                return Ok((status, "The request body is not valid JSON").into_response());
            }
        }
    } else {
        Map::new()
    };
    // The query string over the body, as `request.all()` gave it to the MCP
    // server. A message is refused for any member it should not have, so
    // nothing is left out here.
    for (key, value) in parse_query(parts.uri.query().unwrap_or("")) {
        input.insert(key, value);
    }

    let response = state
        .mcp_gateway
        .handle(McpRequest {
            method: &parts.method,
            headers: &parts.headers,
            body: Value::Object(input),
            caller_ip: Some(client_ip(&state.core, &parts)),
        })
        .await?;

    let (parts, body) = response.into_parts();
    Ok(Response::from_parts(parts, body_of(body)))
}
