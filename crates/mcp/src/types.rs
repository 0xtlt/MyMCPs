//! Protocol constants and message types of `@modelcontextprotocol/sdk` 1.32.
//!
//! Tool definitions and tool results stay JSON values: the gateway forwards
//! them, and a closed struct would lose what a later protocol version adds.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::schemas::JSON_RPC_MESSAGE;
use crate::zod::ValidationError;

/// The protocol version a client of this SDK version asks for.
pub const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";

/// The version assumed for a request that carries no `MCP-Protocol-Version` header.
pub const DEFAULT_NEGOTIATED_PROTOCOL_VERSION: &str = "2025-03-26";

pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 5] = [
    LATEST_PROTOCOL_VERSION,
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
    "2024-10-07",
];

pub const JSONRPC_VERSION: &str = "2.0";

/// How long a request waits for its answer before it is cancelled.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_millis(60_000);

/// JSON-RPC and MCP error codes (`ErrorCode` in the SDK).
pub mod error_code {
    pub const CONNECTION_CLOSED: i64 = -32000;
    pub const REQUEST_TIMEOUT: i64 = -32001;
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    pub const URL_ELICITATION_REQUIRED: i64 = -32042;
}

/// The name and version an MCP client or server announces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Implementation {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl Implementation {
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
            title: None,
        }
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub(crate) fn to_value(&self) -> Value {
        let mut value = Map::new();
        value.insert("name".to_owned(), json!(self.name));
        value.insert("version".to_owned(), json!(self.version));
        if let Some(title) = &self.title {
            value.insert("title".to_owned(), json!(title));
        }
        Value::Object(value)
    }
}

/// A tool as a server lists it: `name`, `description`, `inputSchema` and
/// whatever else the server sent that the protocol knows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Tool(Map<String, Value>);

static NULL: Value = Value::Null;

impl Tool {
    /// Empty only for a value that did not come out of [`crate::client::Client::list_tools`].
    pub fn name(&self) -> &str {
        self.0
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    pub fn description(&self) -> Option<&str> {
        self.0.get("description").and_then(Value::as_str)
    }

    pub fn input_schema(&self) -> &Value {
        self.0.get("inputSchema").unwrap_or(&NULL)
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.get(key)
    }

    pub fn as_object(&self) -> &Map<String, Value> {
        &self.0
    }

    pub fn into_object(self) -> Map<String, Value> {
        self.0
    }
}

impl From<Map<String, Value>> for Tool {
    fn from(fields: Map<String, Value>) -> Self {
        Self(fields)
    }
}

impl From<Tool> for Value {
    fn from(tool: Tool) -> Self {
        Value::Object(tool.0)
    }
}

/// The `error` member of a JSON-RPC error response.
#[derive(Debug, Clone, PartialEq)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

/// An error answered by the other side, or raised by the protocol layer
/// (timeout, closed connection). It reads as the SDK's `McpError.message`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("MCP error {code}: {message}")]
pub struct McpError {
    pub code: i64,
    /// The message without the `MCP error <code>: ` prefix.
    pub message: String,
    pub data: Option<Value>,
}

impl McpError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub(crate) fn connection_closed() -> Self {
        Self::new(error_code::CONNECTION_CLOSED, "Connection closed")
    }

    pub(crate) fn request_timeout(timeout: Duration) -> Self {
        Self {
            code: error_code::REQUEST_TIMEOUT,
            message: "Request timed out".to_owned(),
            data: Some(json!({ "timeout": timeout.as_millis() as u64 })),
        }
    }
}

impl From<JsonRpcError> for McpError {
    fn from(error: JsonRpcError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            data: error.data,
        }
    }
}

/// One JSON-RPC 2.0 message.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonRpcMessage {
    Request {
        id: Value,
        method: String,
        params: Option<Value>,
    },
    Notification {
        method: String,
        params: Option<Value>,
    },
    Result {
        id: Value,
        result: Value,
    },
    Error {
        id: Option<Value>,
        error: JsonRpcError,
    },
}

impl JsonRpcMessage {
    /// Validate a value as the SDK's `JSONRPCMessageSchema` does. The message
    /// holds what that schema keeps.
    pub fn parse(value: &Value) -> Result<Self, ValidationError> {
        let parsed = JSON_RPC_MESSAGE.parse_value(value)?;
        let take = |key: &str| parsed.get(key).cloned();
        let text = |key: &str| take(key).and_then(|value| value.as_str().map(str::to_owned));

        // The four shapes are strict objects with different required keys,
        // so the keys of an accepted message say which one it is.
        if let Some(method) = text("method") {
            let params = take("params");
            return Ok(match take("id") {
                Some(id) => Self::Request { id, method, params },
                None => Self::Notification { method, params },
            });
        }
        if let Some(result) = take("result") {
            return Ok(Self::Result {
                id: take("id").unwrap_or(Value::Null),
                result,
            });
        }
        let error = take("error").unwrap_or(Value::Null);
        Ok(Self::Error {
            id: take("id"),
            error: JsonRpcError {
                code: error
                    .get("code")
                    .and_then(|code| {
                        code.as_i64()
                            .or_else(|| code.as_f64().map(|code| code as i64))
                    })
                    .unwrap_or_default(),
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                data: error.get("data").cloned(),
            },
        })
    }

    /// The message with its keys in the order the SDK writes them.
    pub fn to_value(&self) -> Value {
        let mut message = Map::new();
        match self {
            Self::Request { id, method, params } => {
                message.insert("method".to_owned(), json!(method));
                if let Some(params) = params {
                    message.insert("params".to_owned(), params.clone());
                }
                message.insert("jsonrpc".to_owned(), json!(JSONRPC_VERSION));
                message.insert("id".to_owned(), id.clone());
            }
            // The one message the SDK spells out by hand, `jsonrpc` first.
            Self::Notification { method, params } if method == "notifications/cancelled" => {
                message.insert("jsonrpc".to_owned(), json!(JSONRPC_VERSION));
                message.insert("method".to_owned(), json!(method));
                if let Some(params) = params {
                    message.insert("params".to_owned(), params.clone());
                }
            }
            Self::Notification { method, params } => {
                message.insert("method".to_owned(), json!(method));
                if let Some(params) = params {
                    message.insert("params".to_owned(), params.clone());
                }
                message.insert("jsonrpc".to_owned(), json!(JSONRPC_VERSION));
            }
            Self::Result { id, result } => {
                message.insert("result".to_owned(), result.clone());
                message.insert("jsonrpc".to_owned(), json!(JSONRPC_VERSION));
                message.insert("id".to_owned(), id.clone());
            }
            Self::Error { id, error } => {
                message.insert("jsonrpc".to_owned(), json!(JSONRPC_VERSION));
                if let Some(id) = id {
                    message.insert("id".to_owned(), id.clone());
                }
                let mut body = Map::new();
                body.insert("code".to_owned(), json!(error.code));
                body.insert("message".to_owned(), json!(error.message));
                if let Some(data) = &error.data {
                    body.insert("data".to_owned(), data.clone());
                }
                message.insert("error".to_owned(), Value::Object(body));
            }
        }
        Value::Object(message)
    }

    /// The message as one line of JSON, written as `JSON.stringify` does.
    pub fn to_json(&self) -> String {
        crate::json::to_string(&self.to_value())
    }

    pub fn is_request(&self) -> bool {
        matches!(self, Self::Request { .. })
    }

    pub(crate) fn is_initialized_notification(&self) -> bool {
        matches!(self, Self::Notification { method, .. } if method == "notifications/initialized")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_messages_in_the_key_order_of_the_sdk() {
        let request = JsonRpcMessage::Request {
            id: json!(0),
            method: "initialize".to_owned(),
            params: Some(json!({ "protocolVersion": LATEST_PROTOCOL_VERSION })),
        };
        assert_eq!(
            request.to_json(),
            r#"{"method":"initialize","params":{"protocolVersion":"2025-11-25"},"jsonrpc":"2.0","id":0}"#
        );
        let notification = JsonRpcMessage::Notification {
            method: "notifications/initialized".to_owned(),
            params: None,
        };
        assert_eq!(
            notification.to_json(),
            r#"{"method":"notifications/initialized","jsonrpc":"2.0"}"#
        );
        let result = JsonRpcMessage::Result {
            id: json!("a"),
            result: json!({}),
        };
        assert_eq!(
            result.to_json(),
            r#"{"result":{},"jsonrpc":"2.0","id":"a"}"#
        );
        let error = JsonRpcMessage::Error {
            id: Some(json!(3)),
            error: JsonRpcError {
                code: -32601,
                message: "Method not found".to_owned(),
                data: None,
            },
        };
        assert_eq!(
            error.to_json(),
            r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32601,"message":"Method not found"}}"#
        );
    }

    #[test]
    fn tells_the_four_kinds_of_message_apart() {
        let parse = |text: &str| JsonRpcMessage::parse(&crate::json::parse(text).unwrap());
        assert!(matches!(
            parse(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#),
            Ok(JsonRpcMessage::Request { .. })
        ));
        assert!(matches!(
            parse(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            Ok(JsonRpcMessage::Notification { .. })
        ));
        assert!(matches!(
            parse(r#"{"jsonrpc":"2.0","id":"x","result":{"tools":[]}}"#),
            Ok(JsonRpcMessage::Result { .. })
        ));
        let error = parse(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"Bad","data":{"a":1}}}"#,
        );
        assert_eq!(
            error,
            Ok(JsonRpcMessage::Error {
                id: Some(json!(1)),
                error: JsonRpcError {
                    code: -32602,
                    message: "Bad".to_owned(),
                    data: Some(json!({ "a": 1 }))
                },
            })
        );
        assert!(parse(r#"{"jsonrpc":"2.0","id":1,"method":"ping","extra":true}"#).is_err());
        assert!(parse(r#"{"hello":1}"#).is_err());
    }

    #[test]
    fn reads_an_mcp_error_as_the_sdk_writes_it() {
        assert_eq!(
            McpError::new(-32602, "Bad params").to_string(),
            "MCP error -32602: Bad params"
        );
        let timeout = McpError::request_timeout(DEFAULT_REQUEST_TIMEOUT);
        assert_eq!(timeout.to_string(), "MCP error -32001: Request timed out");
        assert_eq!(timeout.data, Some(json!({ "timeout": 60000 })));
        assert_eq!(
            McpError::connection_closed().to_string(),
            "MCP error -32000: Connection closed"
        );
    }
}
