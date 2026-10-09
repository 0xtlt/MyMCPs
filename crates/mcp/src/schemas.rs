//! The schemas of `@modelcontextprotocol/sdk` 1.32 (`types.js`) that the
//! gateway's requests and answers go through, in the SDK's key order.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use crate::zod::{
    Schema, array, boolean, custom, enumeration, intersection, literal, null, number, object,
    preprocess, record, string, union, unknown,
};

pub(crate) const RELATED_TASK_META_KEY: &str = "io.modelcontextprotocol/related-task";

/// `AssertObjectSchema`: anything `typeof` calls an object, arrays included.
fn assert_object() -> Schema {
    custom(|value| matches!(value, Some(Value::Object(_) | Value::Array(_))))
}

fn progress_token() -> Schema {
    union(vec![string(), number().int()])
}

fn request_id() -> Schema {
    union(vec![string(), number().int()])
}

fn request_meta() -> Schema {
    object(vec![
        ("progressToken", progress_token().optional()),
        (
            RELATED_TASK_META_KEY,
            object(vec![("taskId", string())]).optional(),
        ),
    ])
    .loose()
}

fn base_request_params() -> Schema {
    object(vec![("_meta", request_meta().optional())])
}

fn task_augmented_request_params() -> Schema {
    base_request_params().extend(vec![(
        "task",
        object(vec![("ttl", number().optional())]).optional(),
    )])
}

fn request() -> Schema {
    object(vec![
        ("method", string()),
        ("params", base_request_params().loose().optional()),
    ])
}

fn result() -> Schema {
    object(vec![("_meta", request_meta().optional())]).loose()
}

fn icon() -> Schema {
    object(vec![
        ("src", string()),
        ("mimeType", string().optional()),
        ("sizes", array(string()).optional()),
        ("theme", enumeration(&["light", "dark"]).optional()),
    ])
}

fn implementation() -> Schema {
    object(vec![
        ("name", string()),
        ("title", string().optional()),
        ("icons", array(icon()).optional()),
        ("version", string()),
        ("websiteUrl", string().optional()),
        ("description", string().optional()),
    ])
}

fn client_capabilities() -> Schema {
    let form = intersection(
        object(vec![("applyDefaults", boolean().optional())]),
        record(unknown()),
    );
    // An empty elicitation capability means form mode, for older clients.
    let elicitation = preprocess(
        |value| match value {
            Some(Value::Object(entries)) if entries.is_empty() => Some(json!({ "form": {} })),
            other => other.cloned(),
        },
        intersection(
            object(vec![
                ("form", form.optional()),
                ("url", assert_object().optional()),
            ]),
            record(unknown()).optional(),
        ),
    );
    let tasks = object(vec![
        ("list", assert_object().optional()),
        ("cancel", assert_object().optional()),
        (
            "requests",
            object(vec![
                (
                    "sampling",
                    object(vec![("createMessage", assert_object().optional())])
                        .loose()
                        .optional(),
                ),
                (
                    "elicitation",
                    object(vec![("create", assert_object().optional())])
                        .loose()
                        .optional(),
                ),
            ])
            .loose()
            .optional(),
        ),
    ])
    .loose();

    object(vec![
        ("experimental", record(assert_object()).optional()),
        (
            "sampling",
            object(vec![
                ("context", assert_object().optional()),
                ("tools", assert_object().optional()),
            ])
            .optional(),
        ),
        ("elicitation", elicitation.optional()),
        (
            "roots",
            object(vec![("listChanged", boolean().optional())]).optional(),
        ),
        ("tasks", tasks.optional()),
        ("extensions", record(assert_object()).optional()),
    ])
}

fn server_capabilities() -> Schema {
    let tasks = object(vec![
        ("list", assert_object().optional()),
        ("cancel", assert_object().optional()),
        (
            "requests",
            object(vec![(
                "tools",
                object(vec![("call", assert_object().optional())])
                    .loose()
                    .optional(),
            )])
            .loose()
            .optional(),
        ),
    ])
    .loose();

    object(vec![
        ("experimental", record(assert_object()).optional()),
        ("logging", assert_object().optional()),
        ("completions", assert_object().optional()),
        (
            "prompts",
            object(vec![("listChanged", boolean().optional())]).optional(),
        ),
        (
            "resources",
            object(vec![
                ("subscribe", boolean().optional()),
                ("listChanged", boolean().optional()),
            ])
            .optional(),
        ),
        (
            "tools",
            object(vec![("listChanged", boolean().optional())]).optional(),
        ),
        ("tasks", tasks.optional()),
        ("extensions", record(assert_object()).optional()),
    ])
}

/// The pattern of `z.iso.datetime({ offset: true })`, as zod reports it in an issue.
const ISO_DATETIME_SOURCE: &str = r"/^(?:(?:\d\d[2468][048]|\d\d[13579][26]|\d\d0[48]|[02468][048]00|[13579][26]00)-02-29|\d{4}-(?:(?:0[13578]|1[02])-(?:0[1-9]|[12]\d|3[01])|(?:0[469]|11)-(?:0[1-9]|[12]\d|30)|(?:02)-(?:0[1-9]|1\d|2[0-8])))T(?:(?:[01]\d|2[0-3]):[0-5]\d:[0-5]\d(?:\.\d+)?(?:Z|([+-](?:[01]\d|2[0-3]):[0-5]\d)))$/";

fn iso_datetime_regex() -> &'static Regex {
    static REGEX: LazyLock<Regex> = LazyLock::new(|| {
        // `\d` is ASCII-only in JavaScript.
        let source = ISO_DATETIME_SOURCE
            .trim_matches('/')
            .replace(r"\d", "[0-9]");
        Regex::new(&source).expect("the ISO datetime pattern of zod is a valid regex")
    });
    &REGEX
}

/// What `atob` accepts: base64 with optional padding, ASCII whitespace ignored.
fn is_forgiving_base64(value: Option<&Value>) -> bool {
    let Some(Value::String(text)) = value else {
        return false;
    };
    let mut data: Vec<char> = text
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' '))
        .collect();
    if data.len().is_multiple_of(4) {
        if data.ends_with(&['=', '=']) {
            data.truncate(data.len() - 2);
        } else if data.ends_with(&['=']) {
            data.truncate(data.len() - 1);
        }
    }
    data.len() % 4 != 1
        && data
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/'))
}

fn base64() -> Schema {
    string().refine(is_forgiving_base64, "Invalid Base64 string")
}

fn annotations() -> Schema {
    object(vec![
        (
            "audience",
            array(enumeration(&["user", "assistant"])).optional(),
        ),
        ("priority", number().min(0).max(1).optional()),
        (
            "lastModified",
            string()
                .format(
                    "datetime",
                    "ISO datetime",
                    ISO_DATETIME_SOURCE,
                    iso_datetime_regex,
                )
                .optional(),
        ),
    ])
}

fn resource_contents() -> Schema {
    object(vec![
        ("uri", string()),
        ("mimeType", string().optional()),
        ("_meta", record(unknown()).optional()),
    ])
}

fn resource() -> Schema {
    object(vec![
        ("name", string()),
        ("title", string().optional()),
        ("icons", array(icon()).optional()),
        ("uri", string()),
        ("description", string().optional()),
        ("mimeType", string().optional()),
        ("size", number().optional()),
        ("annotations", annotations().optional()),
        ("_meta", object(vec![]).loose().optional()),
    ])
}

fn content_block() -> Schema {
    let text = object(vec![
        ("type", literal("text")),
        ("text", string()),
        ("annotations", annotations().optional()),
        ("_meta", record(unknown()).optional()),
    ]);
    let media = |kind: &str| {
        object(vec![
            ("type", literal(kind)),
            ("data", base64()),
            ("mimeType", string()),
            ("annotations", annotations().optional()),
            ("_meta", record(unknown()).optional()),
        ])
    };
    let resource_link = resource().extend(vec![("type", literal("resource_link"))]);
    let embedded_resource = object(vec![
        ("type", literal("resource")),
        (
            "resource",
            union(vec![
                resource_contents().extend(vec![("text", string())]),
                resource_contents().extend(vec![("blob", base64())]),
            ]),
        ),
        ("annotations", annotations().optional()),
        ("_meta", record(unknown()).optional()),
    ]);
    union(vec![
        text,
        media("image"),
        media("audio"),
        resource_link,
        embedded_resource,
    ])
}

fn tool() -> Schema {
    let json_schema = || {
        object(vec![
            ("type", literal("object")),
            ("properties", record(assert_object()).optional()),
            ("required", array(string()).optional()),
        ])
        .catchall(unknown())
    };
    object(vec![
        ("name", string()),
        ("title", string().optional()),
        ("icons", array(icon()).optional()),
        ("description", string().optional()),
        ("inputSchema", json_schema()),
        ("outputSchema", json_schema().optional()),
        (
            "annotations",
            object(vec![
                ("title", string().optional()),
                ("readOnlyHint", boolean().optional()),
                ("destructiveHint", boolean().optional()),
                ("idempotentHint", boolean().optional()),
                ("openWorldHint", boolean().optional()),
            ])
            .optional(),
        ),
        (
            "execution",
            object(vec![(
                "taskSupport",
                enumeration(&["required", "optional", "forbidden"]).optional(),
            )])
            .optional(),
        ),
        ("_meta", record(unknown()).optional()),
    ])
}

fn json_rpc_request() -> Schema {
    object(vec![
        ("jsonrpc", literal("2.0")),
        ("id", request_id()),
        ("method", string()),
        ("params", base_request_params().loose().optional()),
    ])
    .strict()
}

fn json_rpc_notification() -> Schema {
    object(vec![
        ("jsonrpc", literal("2.0")),
        ("method", string()),
        ("params", base_request_params().loose().optional()),
    ])
    .strict()
}

fn json_rpc_result_response() -> Schema {
    object(vec![
        ("jsonrpc", literal("2.0")),
        ("id", request_id()),
        ("result", result()),
    ])
    .strict()
}

fn json_rpc_error_response() -> Schema {
    object(vec![
        ("jsonrpc", literal("2.0")),
        ("id", request_id().optional()),
        (
            "error",
            object(vec![
                ("code", number().int()),
                ("message", string()),
                ("data", unknown().optional()),
            ]),
        ),
    ])
    .strict()
}

pub(crate) static JSON_RPC_MESSAGE: LazyLock<Schema> = LazyLock::new(|| {
    union(vec![
        json_rpc_request(),
        json_rpc_notification(),
        json_rpc_result_response(),
        json_rpc_error_response(),
    ])
});

pub(crate) static TASK_AUGMENTED_REQUEST_PARAMS: LazyLock<Schema> =
    LazyLock::new(task_augmented_request_params);

pub(crate) static INITIALIZE_REQUEST: LazyLock<Schema> = LazyLock::new(|| {
    request().extend(vec![
        ("method", literal("initialize")),
        (
            "params",
            base_request_params().extend(vec![
                ("protocolVersion", string()),
                ("capabilities", client_capabilities()),
                ("clientInfo", implementation()),
            ]),
        ),
    ])
});

pub(crate) static INITIALIZE_RESULT: LazyLock<Schema> = LazyLock::new(|| {
    result().extend(vec![
        ("protocolVersion", string()),
        ("capabilities", server_capabilities()),
        ("serverInfo", implementation()),
        ("instructions", string().optional()),
    ])
});

pub(crate) static PING_REQUEST: LazyLock<Schema> = LazyLock::new(|| {
    request().extend(vec![
        ("method", literal("ping")),
        ("params", base_request_params().optional()),
    ])
});

pub(crate) static LIST_TOOLS_REQUEST: LazyLock<Schema> = LazyLock::new(|| {
    request().extend(vec![
        (
            "params",
            base_request_params()
                .extend(vec![("cursor", string().optional())])
                .optional(),
        ),
        ("method", literal("tools/list")),
    ])
});

pub(crate) static LIST_TOOLS_RESULT: LazyLock<Schema> = LazyLock::new(|| {
    result().extend(vec![
        ("nextCursor", string().optional()),
        ("tools", array(tool())),
    ])
});

pub(crate) static CALL_TOOL_REQUEST: LazyLock<Schema> = LazyLock::new(|| {
    request().extend(vec![
        ("method", literal("tools/call")),
        (
            "params",
            task_augmented_request_params().extend(vec![
                ("name", string()),
                ("arguments", record(unknown()).optional()),
            ]),
        ),
    ])
});

pub(crate) static CALL_TOOL_RESULT: LazyLock<Schema> = LazyLock::new(|| {
    result().extend(vec![
        ("content", array(content_block()).default_value(json!([]))),
        ("structuredContent", record(unknown()).optional()),
        ("isError", boolean().optional()),
    ])
});

/// What a task-augmented `tools/call` has to answer with.
pub(crate) static CREATE_TASK_RESULT: LazyLock<Schema> = LazyLock::new(|| {
    result().extend(vec![(
        "task",
        object(vec![
            ("taskId", string()),
            (
                "status",
                enumeration(&[
                    "working",
                    "input_required",
                    "completed",
                    "failed",
                    "cancelled",
                ]),
            ),
            ("ttl", union(vec![number(), null()])),
            ("createdAt", string()),
            ("lastUpdatedAt", string()),
            ("pollInterval", number().optional()),
            ("statusMessage", string().optional()),
        ]),
    )])
});

#[cfg(test)]
pub(crate) fn by_name(name: &str) -> Option<&'static Schema> {
    Some(match name {
        "JSONRPCMessage" => &JSON_RPC_MESSAGE,
        "JSONRPCRequest" => {
            static SCHEMA: LazyLock<Schema> = LazyLock::new(json_rpc_request);
            &SCHEMA
        }
        "JSONRPCNotification" => {
            static SCHEMA: LazyLock<Schema> = LazyLock::new(json_rpc_notification);
            &SCHEMA
        }
        "JSONRPCResultResponse" => {
            static SCHEMA: LazyLock<Schema> = LazyLock::new(json_rpc_result_response);
            &SCHEMA
        }
        "JSONRPCErrorResponse" => {
            static SCHEMA: LazyLock<Schema> = LazyLock::new(json_rpc_error_response);
            &SCHEMA
        }
        "TaskAugmentedRequestParams" => &TASK_AUGMENTED_REQUEST_PARAMS,
        "InitializeRequest" => &INITIALIZE_REQUEST,
        "InitializeResult" => &INITIALIZE_RESULT,
        "PingRequest" => &PING_REQUEST,
        "ListToolsRequest" => &LIST_TOOLS_REQUEST,
        "ListToolsResult" => &LIST_TOOLS_RESULT,
        "CallToolRequest" => &CALL_TOOL_REQUEST,
        "CallToolResult" => &CALL_TOOL_RESULT,
        "CreateTaskResult" => &CREATE_TASK_RESULT,
        _ => return None,
    })
}
