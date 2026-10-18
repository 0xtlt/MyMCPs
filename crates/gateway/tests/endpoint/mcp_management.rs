//! The tools that manage the MCP registry, at the level of the gateway: who
//! is shown them and who is refused, with a manager that records what it is
//! asked instead of the one the web server brings.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mymcps_core::models::{OauthClient, ScopeMode, Team, TeamRole, User};
use mymcps_gateway::access_token::{self, NewOauthGrant};
use mymcps_gateway::mcp_management::{
    EnvVariable, ManagementActor, ManagementError, McpFields, McpRegistryManager,
};
use serde_json::{Value, json};

use crate::support::mcp::*;
use crate::support::*;

const RESOURCE: &str = "http://localhost:3333/mcp";
const REGISTRY_TOOLS: [&str; 6] = [
    "mcp_registry_list",
    "mcp_registry_create",
    "mcp_registry_update",
    "mcp_registry_delete",
    "mcp_registry_test",
    "mcp_registry_auth_link",
];

/// What the manager was asked, in order.
#[derive(Debug, Clone, PartialEq)]
enum Asked {
    List,
    Create(McpFields),
    Update(String, McpFields),
    Delete(String),
    Test(String),
    AuthLink(String),
}

#[derive(Debug, Default)]
struct Recorder {
    asked: Mutex<Vec<(ManagementActor, Asked)>>,
    /// What every operation answers with.
    answer: Mutex<Option<Result<Value, ManagementError>>>,
}

impl Recorder {
    fn record(&self, actor: &ManagementActor, asked: Asked) -> Result<Value, ManagementError> {
        self.asked.lock().unwrap().push((*actor, asked));
        self.answer
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| Ok(json!({ "mcp": { "id": 7, "slug": "docs" } })))
    }

    fn asked(&self) -> Vec<(ManagementActor, Asked)> {
        self.asked.lock().unwrap().clone()
    }
}

#[async_trait]
impl McpRegistryManager for Recorder {
    async fn list(&self, actor: &ManagementActor) -> Result<Value, ManagementError> {
        self.record(actor, Asked::List)
    }

    async fn create(
        &self,
        actor: &ManagementActor,
        fields: McpFields,
    ) -> Result<Value, ManagementError> {
        self.record(actor, Asked::Create(fields))
    }

    async fn update(
        &self,
        actor: &ManagementActor,
        slug: &str,
        fields: McpFields,
    ) -> Result<Value, ManagementError> {
        self.record(actor, Asked::Update(slug.to_owned(), fields))
    }

    async fn delete(&self, actor: &ManagementActor, slug: &str) -> Result<Value, ManagementError> {
        self.record(actor, Asked::Delete(slug.to_owned()))
    }

    async fn test(&self, actor: &ManagementActor, slug: &str) -> Result<Value, ManagementError> {
        self.record(actor, Asked::Test(slug.to_owned()))
    }

    async fn auth_link(
        &self,
        actor: &ManagementActor,
        slug: &str,
    ) -> Result<Value, ManagementError> {
        self.record(actor, Asked::AuthLink(slug.to_owned()))
    }
}

async fn gateway(recorder: &Arc<Recorder>) -> TestMcpGateway {
    TestMcpGateway::offline()
        .await
        .with_mcp_management(recorder.clone())
}

async fn grant(gateway: &TestMcpGateway, user: &User, scopes: &str) -> (String, i64) {
    let db = gateway.db();
    let mut client = OauthClient {
        client_id: format!("mcp_client_{}", user.id),
        client_name: "Claude".into(),
        redirect_uris: json!(["http://127.0.0.1/callback"]).to_string(),
        token_endpoint_auth_method: "none".into(),
        grant_types: json!(["authorization_code", "refresh_token"]).to_string(),
        response_types: json!(["code"]).to_string(),
        scope: "mcp:tools".into(),
        ..Default::default()
    };
    client.insert(&**db).await.unwrap();
    let created = access_token::create_oauth_grant(
        &**db,
        NewOauthGrant {
            name: "Claude",
            client_id: client.id,
            client_supports_refresh: true,
            scopes,
            resource: RESOURCE,
            created_by: user.id,
            team_id: Team::ROOT_ID,
        },
    )
    .await
    .unwrap();
    (created.plaintext, created.token.id)
}

async fn tool_names(gateway: &TestMcpGateway, plaintext: &str, mode: &str) -> Vec<String> {
    gateway.rpc(plaintext, "tools/list", json!({}), mode).await["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect()
}

fn has_registry_tools(names: &[String]) -> bool {
    names
        .iter()
        .any(|name| REGISTRY_TOOLS.contains(&name.as_str()))
}

#[tokio::test]
async fn lists_the_tools_for_a_token_that_may_manage_mcps_in_both_tool_modes() {
    let recorder = Arc::new(Recorder::default());
    let gateway = gateway(&recorder).await;
    let admin = create_admin(gateway.db()).await;
    let (manager, _) = grant(&gateway, &admin, "mcp:tools mcps:manage").await;

    for mode in ["eager", "lazy"] {
        let names = tool_names(&gateway, &manager, mode).await;
        assert_eq!(names[names.len() - 6..], REGISTRY_TOOLS, "{mode}");
        if mode == "lazy" {
            assert_eq!(names[..3], ["list_mcps", "tool_search", "call_tool"]);
        }
    }
}

#[tokio::test]
async fn shows_no_tool_when_the_token_or_the_person_may_not() {
    let recorder = Arc::new(Recorder::default());
    let gateway = gateway(&recorder).await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let member = create_user(db, "member", TeamRole::Member).await;
    let (plain, _) = grant(&gateway, &admin, "mcp:tools").await;
    let (demoted, _) = grant(&gateway, &member, "mcp:tools mcps:manage").await;
    let manual = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    for plaintext in [&plain, &demoted, &manual] {
        for mode in ["eager", "lazy"] {
            assert!(!has_registry_tools(
                &tool_names(&gateway, plaintext, mode).await
            ));
        }
    }
}

#[tokio::test]
async fn has_no_such_tool_when_no_manager_is_installed() {
    let gateway = TestMcpGateway::offline().await;
    let admin = create_admin(gateway.db()).await;
    let (manager, _) = grant(&gateway, &admin, "mcp:tools mcps:manage").await;

    for mode in ["eager", "lazy"] {
        assert!(!has_registry_tools(
            &tool_names(&gateway, &manager, mode).await
        ));
    }
    // The name is unknown, as any other without a namespace.
    let result = gateway.call(&manager, "mcp_registry_list", json!({})).await;
    assert_eq!(result["isError"], true);
    assert_eq!(result_text(&result), "Invalid tool name");
}

#[tokio::test]
async fn refuses_a_call_without_the_permission_before_it_reaches_the_manager() {
    let recorder = Arc::new(Recorder::default());
    let gateway = gateway(&recorder).await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let member = create_user(db, "member", TeamRole::Member).await;
    let (plain, _) = grant(&gateway, &admin, "mcp:tools").await;
    let (demoted, _) = grant(&gateway, &member, "mcp:tools mcps:manage").await;
    let manual = create_access_token(db, admin.id, ScopeMode::All, &[])
        .await
        .plaintext;

    for (plaintext, expected) in [
        (&plain, "not allowed to manage MCPs"),
        (&manual, "not allowed to manage MCPs"),
        (&demoted, "not an administrator or an owner"),
    ] {
        for name in REGISTRY_TOOLS {
            let result = gateway
                .call(
                    plaintext,
                    name,
                    json!({ "slug": "docs", "name": "Docs", "transport": "http" }),
                )
                .await;
            assert_eq!(result["isError"], true, "{name}");
            assert!(result_text(&result).contains(expected), "{name}");
        }
    }
    assert!(recorder.asked().is_empty());
}

#[tokio::test]
async fn hands_a_valid_call_to_the_manager_for_the_person_who_connected_the_client() {
    let recorder = Arc::new(Recorder::default());
    let gateway = gateway(&recorder).await;
    let admin = create_admin(gateway.db()).await;
    let (manager, token_id) = grant(&gateway, &admin, "mcp:tools mcps:manage").await;
    let actor = ManagementActor {
        user_id: admin.id,
        team_id: Team::ROOT_ID,
        access_token_id: token_id,
    };

    let list = gateway.call(&manager, "mcp_registry_list", json!({})).await;
    assert_eq!(
        list["structuredContent"],
        json!({ "mcp": { "id": 7, "slug": "docs" } })
    );
    assert!(list.get("isError").is_none());

    gateway
        .call(
            &manager,
            "mcp_registry_create",
            json!({
                "name": "Docs", "transport": "npm", "npm_package": "@example/docs",
                "npm_args": ["--flag"], "npm_env": [{ "name": "KEY", "value": "v" }, { "name": "OLD" }],
                "auth_type": "bearer", "bearer_token": "t", "enabled": false,
            }),
        )
        .await;
    gateway
        .call(
            &manager,
            "mcp_registry_update",
            json!({ "slug": "docs", "description": "" }),
        )
        .await;
    gateway
        .call(&manager, "mcp_registry_delete", json!({ "slug": "docs" }))
        .await;
    gateway
        .call(&manager, "mcp_registry_test", json!({ "slug": "docs" }))
        .await;
    // The lazy mode reaches the same tools.
    gateway
        .rpc(
            &manager,
            "tools/call",
            json!({ "name": "mcp_registry_auth_link", "arguments": { "slug": "docs" } }),
            "lazy",
        )
        .await;

    let asked = recorder.asked();
    assert!(asked.iter().all(|(who, _)| *who == actor));
    let asked: Vec<Asked> = asked.into_iter().map(|(_, asked)| asked).collect();
    assert_eq!(
        asked[1],
        Asked::Create(McpFields {
            name: Some("Docs".into()),
            transport: Some(mymcps_core::models::McpTransport::Npm),
            npm_package: Some("@example/docs".into()),
            npm_args: Some(vec!["--flag".into()]),
            npm_env: Some(vec![
                EnvVariable {
                    name: "KEY".into(),
                    value: Some("v".into())
                },
                EnvVariable {
                    name: "OLD".into(),
                    value: None
                },
            ]),
            auth_type: Some(mymcps_core::models::McpAuthType::Bearer),
            bearer_token: Some("t".into()),
            enabled: Some(false),
            ..Default::default()
        })
    );
    assert_eq!(
        asked[2],
        Asked::Update(
            "docs".into(),
            McpFields {
                description: Some(String::new()),
                ..Default::default()
            }
        )
    );
    assert_eq!(asked[0], Asked::List);
    assert_eq!(asked[3], Asked::Delete("docs".into()));
    assert_eq!(asked[4], Asked::Test("docs".into()));
    assert_eq!(asked[5], Asked::AuthLink("docs".into()));
}

#[tokio::test]
async fn answers_a_wrong_argument_without_calling_the_manager() {
    let recorder = Arc::new(Recorder::default());
    let gateway = gateway(&recorder).await;
    let admin = create_admin(gateway.db()).await;
    let (manager, _) = grant(&gateway, &admin, "mcp:tools mcps:manage").await;

    for (name, arguments, expected) in [
        (
            "mcp_registry_list",
            json!({ "slug": "docs" }),
            "slug is not an argument of this tool",
        ),
        ("mcp_registry_delete", json!({}), "slug"),
        (
            "mcp_registry_delete",
            json!({ "slug": "docs", "force": true }),
            "force is not an argument",
        ),
        (
            "mcp_registry_create",
            json!({ "name": "Docs" }),
            "transport",
        ),
        (
            "mcp_registry_create",
            json!({ "name": "Docs", "transport": "builtin" }),
            "built-in MCPs",
        ),
        (
            "mcp_registry_update",
            json!({ "slug": "docs", "enabled": "no" }),
            "enabled must be true or false",
        ),
        (
            "mcp_registry_update",
            json!({ "slug": "docs", "npm_env": [{ "value": "x" }] }),
            "npm_env",
        ),
    ] {
        let result = gateway.call(&manager, name, arguments.clone()).await;
        assert_eq!(result["isError"], true, "{name} {arguments}");
        assert!(
            result_text(&result).contains(expected),
            "{name} {arguments}: {}",
            result_text(&result)
        );
    }
    assert!(recorder.asked().is_empty());
}

#[tokio::test]
async fn tells_the_agent_why_but_never_what_broke_inside() {
    let recorder = Arc::new(Recorder::default());
    let gateway = gateway(&recorder).await;
    let admin = create_admin(gateway.db()).await;
    let (manager, _) = grant(&gateway, &admin, "mcp:tools mcps:manage").await;

    *recorder.answer.lock().unwrap() = Some(Err(ManagementError::Refused(
        "MCP \"docs\" was not found".into(),
    )));
    let result = gateway
        .call(&manager, "mcp_registry_test", json!({ "slug": "docs" }))
        .await;
    assert_eq!(result["isError"], true);
    assert_eq!(result_text(&result), "MCP \"docs\" was not found");

    *recorder.answer.lock().unwrap() = Some(Err(ManagementError::Failed(
        "database exploded: sk-secret".into(),
    )));
    let result = gateway
        .call(&manager, "mcp_registry_test", json!({ "slug": "docs" }))
        .await;
    assert_eq!(result["isError"], true);
    assert!(!result_text(&result).contains("sk-secret"));
}

#[tokio::test]
async fn keeps_these_calls_out_of_the_call_log() {
    let recorder = Arc::new(Recorder::default());
    let gateway = gateway(&recorder).await;
    let admin = create_admin(gateway.db()).await;
    let (manager, _) = grant(&gateway, &admin, "mcp:tools mcps:manage").await;
    let (plain, _) = grant(&gateway, &create_admin(gateway.db()).await, "mcp:tools").await;

    gateway
        .call(&manager, "mcp_registry_create", json!({ "name": "Docs", "transport": "http", "url": "https://docs.example/mcp", "bearer_token": "log-me-not" }))
        .await;
    gateway
        .call(
            &plain,
            "mcp_registry_create",
            json!({ "bearer_token": "log-me-not" }),
        )
        .await;
    gateway.gateway.call_log.flush().await;
    let logged: i64 = sqlx::query_scalar("select count(*) from `mcp_call_logs`")
        .fetch_one(&**gateway.db())
        .await
        .unwrap();
    assert_eq!(logged, 0);
}
