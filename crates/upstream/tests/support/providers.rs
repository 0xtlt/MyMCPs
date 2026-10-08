//! Small built-in MCPs for the tests of the runtime: one that signs in with
//! OAuth, one that signs in with a password, and one named like the provider
//! whose tools commit money.

use std::sync::{Arc, LazyLock};

use mymcps_builtin::arguments::{NO_ARGUMENTS_VALIDATOR, NoArguments, TOOL_VINE, integer};
use mymcps_builtin::{
    ApprovalDetail, ApprovalSummary, BuiltinError, BuiltinFile, BuiltinMcpDefinition,
    BuiltinOauthConfig, BuiltinPasswordConfig, BuiltinPasswordContext, BuiltinProvider,
    BuiltinRegistry, BuiltinResult, BuiltinSettingField, BuiltinTool, BuiltinToolContext,
    BuiltinUploadTarget,
};
use mymcps_net::{FetchRequest, UpstreamResponseLimits};
use mymcps_vine as vine;
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};

static AMOUNT_VALIDATOR: LazyLock<vine::Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "amount" => integer(1..=1000),
    })
});

#[derive(Deserialize)]
struct Amount {
    amount: i64,
}

fn no_arguments_schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

fn amount_schema() -> Value {
    json!({
        "type": "object",
        "properties": { "amount": { "type": "integer", "minimum": 1, "maximum": 1000 } },
        "required": ["amount"],
        "additionalProperties": false,
    })
}

/// One authenticated request to the API of the example provider.
async fn example_api(context: &BuiltinToolContext, path: &str) -> BuiltinResult<Value> {
    let url = format!("https://api.example.test{path}")
        .parse()
        .map_err(BuiltinError::internal)?;
    let request = FetchRequest::get(url)
        .header("Authorization", &format!("Bearer {}", context.access_token))?;
    let mut response = context
        .env
        .fetcher
        .fetch_with_same_origin_redirects(request, "Example API", UpstreamResponseLimits::default())
        .await?;
    match response.status().as_u16() {
        200 => Ok(response.json().await?),
        401 => Err(BuiltinError::authorization(
            "Example rejected the saved authorization. Re-authorize this MCP in MyMCPs.",
        )),
        status => Err(BuiltinError::internal(format!(
            "Example API returned HTTP {status}"
        ))),
    }
}

/// A provider that signs in with OAuth: read tools, one of which needs a
/// scope, and write tools, one of which asks for approval.
pub fn example() -> BuiltinMcpDefinition {
    let get_profile = BuiltinTool::new(
        "get_profile",
        "Returns the profile of the connected account.",
        no_arguments_schema(),
        &NO_ARGUMENTS_VALIDATOR,
        |_: NoArguments, context: Arc<BuiltinToolContext>| async move {
            let profile = example_api(&context, "/profile").await?;
            Ok(json!({
                "profile": profile,
                "account": context.settings.get("account"),
                "scopes": context.granted_scopes,
                "mcpId": context.mcp_id,
            }))
        },
    );
    let list_items = BuiltinTool::new(
        "list_items",
        "Lists the items of the connected account.",
        no_arguments_schema(),
        &NO_ARGUMENTS_VALIDATOR,
        |_: NoArguments, context: Arc<BuiltinToolContext>| async move {
            example_api(&context, "/items").await
        },
    )
    .requires_any_scope(&["items:read", "items:read_all"]);
    let create_item = BuiltinTool::new(
        "create_item",
        "Creates an item.",
        amount_schema(),
        &AMOUNT_VALIDATOR,
        |input: Amount, _: Arc<BuiltinToolContext>| async move {
            Ok(json!({ "created": input.amount }))
        },
    )
    .write()
    .requires_any_scope(&["items:write"]);
    let set_budget =
        BuiltinTool::new(
            "set_budget",
            "Sets the daily budget.",
            amount_schema(),
            &AMOUNT_VALIDATOR,
            |input: Amount, _: Arc<BuiltinToolContext>| async move {
                Ok(json!({ "budget": input.amount }))
            },
        )
        .write()
        .asks_approval()
        .requires_any_scope(&["items:write"])
        .describe(
            |input: Amount, context: Arc<BuiltinToolContext>| async move {
                let profile = example_api(&context, "/profile").await?;
                Ok(Some(ApprovalSummary {
                    title: format!(
                        "Change the daily budget of {}",
                        profile["name"].as_str().unwrap_or("the account")
                    ),
                    details: vec![
                        ApprovalDetail::new("Daily budget", format!("€{}", input.amount))
                            .replacing("€2"),
                    ],
                    warnings: None,
                }))
            },
        );

    let provider = BuiltinProvider::new(
        "example",
        "Example",
        vec![get_profile, list_items, create_item, set_budget],
        |context: Arc<BuiltinToolContext>| async move {
            example_api(&context, "/profile").await.map(|_| ())
        },
    )
    .settings(vec![
        BuiltinSettingField {
            key: "account",
            required: true,
            pattern: Regex::new(r"^\d+$").unwrap(),
            hint: "The account number".to_owned(),
            normalize: None,
        },
        BuiltinSettingField {
            key: "region",
            required: false,
            pattern: Regex::new(r"^[a-z]+$").unwrap(),
            hint: "The region".to_owned(),
            normalize: None,
        },
    ])
    .download(
        |reference: Value, context: Arc<BuiltinToolContext>| async move {
            let Some(name) = reference["name"].as_str() else {
                return Err(BuiltinError::tool("This link is no longer valid"));
            };
            Ok(BuiltinFile {
                filename: name.to_owned(),
                content_type: "text/plain".to_owned(),
                content: vec![format!("file of {}", context.access_token).into()],
            })
        },
    )
    .upload(|reference: Value, _: Arc<BuiltinToolContext>| async move {
        Ok(BuiltinUploadTarget {
            id: reference["id"].as_str().unwrap_or_default().to_owned(),
            filename: "upload.bin".to_owned(),
            content_type: None,
            max_bytes: 1024,
        })
    });

    BuiltinMcpDefinition::Oauth {
        provider,
        oauth: BuiltinOauthConfig {
            issuer: "https://www.example.test",
            authorize_url: "https://www.example.test/oauth/authorize",
            token_url: "https://www.example.test/oauth/token",
            scopes: vec!["read", "items:read_all"],
            write_scopes: vec!["items:write"],
            scope_separator: ",",
            authorize_params: vec![("approval_prompt", "force")],
            sends_redirect_uri_with_code: true,
            client_id_pattern: None,
            client_id_hint: None,
        },
    }
}

/// A provider that signs in with a password, whose permissions MyMCPs
/// enforces itself.
pub fn mailbox() -> BuiltinMcpDefinition {
    let list_messages = BuiltinTool::new(
        "list_messages",
        "Lists the messages of the mailbox.",
        no_arguments_schema(),
        &NO_ARGUMENTS_VALIDATOR,
        |_: NoArguments, context: Arc<BuiltinPasswordContext>| async move {
            Ok(json!({
                "username": context.username,
                "permissions": context.permissions,
                "aliases": context.aliases,
                "mcpId": context.mcp_id,
            }))
        },
    )
    .requires_any_scope(&["read"]);
    let send_message = BuiltinTool::new(
        "send_message",
        "Sends a message.",
        no_arguments_schema(),
        &NO_ARGUMENTS_VALIDATOR,
        |_: NoArguments, _: Arc<BuiltinPasswordContext>| async move { Ok(json!({ "sent": true })) },
    )
    .requires_any_scope(&["send"]);

    let provider = BuiltinProvider::new(
        "mailbox",
        "Mailbox",
        vec![list_messages, send_message],
        |context: Arc<BuiltinPasswordContext>| async move {
            if context.password == "app-password" {
                Ok(())
            } else {
                Err(BuiltinError::authorization(
                    "Mailbox refused the app password. Enter a new one in MyMCPs.",
                ))
            }
        },
    )
    .upload(
        |reference: Value, _: Arc<BuiltinPasswordContext>| async move {
            Ok(BuiltinUploadTarget {
                id: reference["id"].as_str().unwrap_or_default().to_owned(),
                filename: "attachment.bin".to_owned(),
                content_type: Some("application/octet-stream".to_owned()),
                max_bytes: 2048,
            })
        },
    );

    BuiltinMcpDefinition::Password {
        provider,
        password: BuiltinPasswordConfig {
            username_pattern: Regex::new(r"^\S+@\S+$").unwrap(),
            username_hint: "The address of the mailbox",
            password_pattern: Regex::new(r"^[a-z-]+$").unwrap(),
            password_hint: "An app password",
            permissions: vec!["read", "send"],
            alias_hint: "Other addresses of the mailbox",
        },
    }
}

/// Stands in for the Google Ads provider in the approval policy: the same
/// key and tool names, with the tools that commit money asking by default.
pub fn google_ads() -> BuiltinMcpDefinition {
    let tool = |name: &'static str| {
        BuiltinTool::new(
            name,
            format!("Runs {name}."),
            no_arguments_schema(),
            &NO_ARGUMENTS_VALIDATOR,
            |_: NoArguments, _: Arc<BuiltinToolContext>| async move { Ok(json!({})) },
        )
    };
    let provider = BuiltinProvider::new(
        "google-ads",
        "Google Ads",
        vec![
            tool("list_campaigns"),
            tool("update_campaign_budget").write().asks_approval(),
            tool("set_campaign_status").write().asks_approval(),
            tool("add_keywords").write(),
        ],
        |_: Arc<BuiltinToolContext>| async move { Ok(()) },
    );
    BuiltinMcpDefinition::Oauth {
        provider,
        oauth: BuiltinOauthConfig {
            issuer: "https://accounts.google.com",
            authorize_url: "https://accounts.google.com/o/oauth2/v2/auth",
            token_url: "https://oauth2.googleapis.com/token",
            scopes: vec!["https://www.googleapis.com/auth/adwords"],
            write_scopes: vec![],
            scope_separator: " ",
            authorize_params: vec![("access_type", "offline"), ("prompt", "consent")],
            sends_redirect_uri_with_code: true,
            client_id_pattern: None,
            client_id_hint: None,
        },
    }
}

pub fn registry() -> BuiltinRegistry {
    BuiltinRegistry::new(vec![example(), mailbox(), google_ads()])
}
