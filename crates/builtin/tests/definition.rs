//! How a provider crate describes its MCP, and what the gateway can do with it.

use std::collections::BTreeMap;
use std::sync::Arc;

use mymcps_builtin::{
    ApprovalDetail, ApprovalSummary, BuiltinEnv, BuiltinError, BuiltinFile, BuiltinMcpDefinition,
    BuiltinOauthConfig, BuiltinProvider, BuiltinRegistry, BuiltinTool, BuiltinToolContext,
    ToolInput,
};
use mymcps_core::TestCore;
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// Stands in for a validator: a positive `amount` is required.
struct AmountInput;

impl ToolInput<BuiltinToolContext> for AmountInput {
    fn validate(&self, arguments: Value, _: &BuiltinToolContext) -> Result<Value, String> {
        match arguments.get("amount").and_then(Value::as_i64) {
            Some(amount) if amount > 0 => Ok(json!({ "amount": amount })),
            Some(_) => Err("amount must be positive".into()),
            None => Err("amount is required".into()),
        }
    }

    fn json_schema(&self) -> Value {
        json!({ "type": "object", "properties": { "amount": { "type": "integer" } }, "required": ["amount"] })
    }
}

#[derive(Deserialize)]
struct Amount {
    amount: i64,
}

struct TestSeam(&'static str);

fn definition() -> BuiltinMcpDefinition {
    let schema = json!({ "type": "object", "properties": { "amount": { "type": "integer" } }, "required": ["amount"] });
    let read = BuiltinTool::new(
        "get_budget",
        "Reads the budget",
        schema.clone(),
        AmountInput,
        |input: Amount, context: Arc<BuiltinToolContext>| async move {
            Ok(
                json!({ "budget": input.amount, "account": context.settings.get("account"), "token": context.access_token.len() }),
            )
        },
    );
    let write = BuiltinTool::new(
        "set_budget",
        "Sets the budget",
        schema,
        AmountInput,
        |input: Amount, _: Arc<BuiltinToolContext>| async move {
            if input.amount > 1_000 {
                return Err(BuiltinError::tool("The provider refuses budgets over 1000"));
            }
            Ok(json!({ "budget": input.amount }))
        },
    )
    .write()
    .asks_approval()
    .requires_any_scope(&["budget:write"])
    .describe(|input: Amount, _: Arc<BuiltinToolContext>| async move {
        Ok(Some(ApprovalSummary {
            title: "Change the daily budget".into(),
            details: vec![
                ApprovalDetail::new("Daily budget", format!("€{}", input.amount)).replacing("€2"),
            ],
            warnings: None,
        }))
    });

    let provider = BuiltinProvider::new(
        "example",
        "Example",
        vec![read, write],
        |context: Arc<BuiltinToolContext>| async move {
            if context.access_token.is_empty() {
                Err(BuiltinError::authorization("Example is not connected"))
            } else {
                Ok(())
            }
        },
    )
    .download(|reference: Value, _: Arc<BuiltinToolContext>| async move {
        Ok(BuiltinFile {
            filename: reference["name"].as_str().unwrap_or("file").into(),
            content_type: "text/plain".into(),
            content: vec!["hi".into()],
        })
    });

    BuiltinMcpDefinition::Oauth {
        provider,
        oauth: BuiltinOauthConfig {
            issuer: "https://example.test",
            authorize_url: "https://example.test/authorize",
            token_url: "https://example.test/token",
            scopes: vec!["read"],
            write_scopes: vec!["budget:write"],
            scope_separator: " ",
            authorize_params: vec![],
            sends_redirect_uri_with_code: false,
            client_id_pattern: None,
            client_id_hint: None,
        },
    }
}

fn arguments(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

#[tokio::test]
async fn a_tool_only_runs_with_arguments_that_passed_its_validator() {
    let core = TestCore::new().await;
    let env = BuiltinEnv::new(core.core.clone()).with_extension(TestSeam("fake server"));
    assert_eq!(env.extension::<TestSeam>().unwrap().0, "fake server");
    assert!(env.extension::<String>().is_none());

    let registry = BuiltinRegistry::new(vec![definition()]);
    assert_eq!(registry.keys(), ["example"]);
    assert!(registry.get(None).is_none());
    assert!(registry.get(Some("strava")).is_none());
    let definition = registry.get(Some("example")).unwrap();
    assert_eq!(
        (definition.key(), definition.name()),
        ("example", "Example")
    );
    assert!(definition.oauth().is_some() && definition.password().is_none());
    assert!(definition.has_download() && !definition.has_upload());

    let tools = definition.tools();
    assert_eq!(
        tools.iter().map(|tool| tool.name).collect::<Vec<_>>(),
        ["get_budget", "set_budget"]
    );
    assert!(!tools[0].write && !tools[0].asks_approval && tools[0].requires_any_scope.is_empty());
    assert!(tools[1].write && tools[1].asks_approval);
    assert_eq!(tools[1].requires_any_scope, ["budget:write"]);
    assert_eq!(definition.enforced_schemas()[0].1, *tools[0].input_schema);

    let BuiltinMcpDefinition::Oauth { provider, .. } = &**definition else {
        panic!("an OAuth provider")
    };
    let context = Arc::new(BuiltinToolContext {
        env,
        mcp_id: 7,
        access_token: "token".into(),
        granted_scopes: None,
        settings: BTreeMap::from([("account".to_string(), "123".to_string())]),
    });

    let read = provider.tool("get_budget").unwrap();
    assert_eq!(
        read.run(
            arguments(json!({ "amount": 5, "ignored": true })),
            context.clone()
        )
        .await
        .unwrap(),
        json!({ "budget": 5, "account": "123", "token": 5 })
    );
    let refused = read
        .run(arguments(json!({})), context.clone())
        .await
        .unwrap_err();
    assert!(refused.is_tool_error());
    assert_eq!(refused.to_string(), "amount is required");
    assert_eq!(
        read.describe_call(arguments(json!({ "amount": 5 })), context.clone())
            .await
            .unwrap(),
        None
    );

    let write = provider.tool("set_budget").unwrap();
    let summary = write
        .describe_call(arguments(json!({ "amount": 250 })), context.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&summary).unwrap(),
        json!({ "title": "Change the daily budget", "details": [{ "label": "Daily budget", "value": "€250", "before": "€2" }] })
    );
    // Nobody is asked to approve a call that would be refused.
    assert_eq!(
        write
            .describe_call(arguments(json!({ "amount": 0 })), context.clone())
            .await
            .unwrap_err()
            .to_string(),
        "amount must be positive"
    );
    assert_eq!(
        write
            .run(arguments(json!({ "amount": 5000 })), context.clone())
            .await
            .unwrap_err()
            .to_string(),
        "The provider refuses budgets over 1000"
    );

    provider.verify(context.clone()).await.unwrap();
    let file = provider
        .download_file(json!({ "name": "a.txt" }), context.clone())
        .await
        .unwrap();
    assert_eq!(file.filename, "a.txt");
    let no_upload = provider
        .upload_target(json!({}), context)
        .await
        .unwrap_err();
    assert!(no_upload.is_tool_error());
    assert_eq!(no_upload.to_string(), "Example takes no files");
}
