//! Port of `tests/functional/hardening_upstream_mcps.spec.ts`, and of the
//! unit specs of `assignMcpFromPayload`:
//! `tests/unit/hardening_upstream_credentials.spec.ts`, the cases of
//! `tests/unit/mcp_environment.spec.ts` about the form, and the endpoint case
//! of `tests/unit/security.spec.ts`.

mod mcps_support;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use http::{Method, StatusCode};
use mcps_support::*;
use mymcps_core::models::{Mcp, McpAuthType, McpStatus, McpTransport, User};
use mymcps_core::secrets::{EnvironmentInput, merge_environment};
use mymcps_deno::{DenoError, DenoRunner, DenoRuntime};
use mymcps_net::CannedResponse;
use mymcps_upstream::Upstream;
use mymcps_web::AppState;
use mymcps_web::routes::mcps::{AssignError, assign_mcp_from_payload};
use mymcps_web::state::builtins;
use mymcps_web::testing::TestApp;
use mymcps_web::testing::factories::{create_admin, create_mcp, create_member};
use mymcps_web::validators::mcp::{CREATE_MCP_VALIDATOR, McpPayload};
use mymcps_web::views::mcps::McpView;
use serde_json::{Value, json};
use url::Url;

// --- assigning a form to an MCP ---

/// The form as the page submits it, with `values` over it.
fn form(values: Value) -> McpPayload {
    try_form(values).expect("a valid form")
}

fn try_form(values: Value) -> Result<McpPayload, mymcps_vine::Error> {
    let mut form = json!({
        "name": "Repointed MCP",
        "description": "",
        "transport": "http",
        "httpUrl": "",
        "npmPackage": "",
        "npmVersion": "",
        "npmArgs": "",
        "npmEnv": [],
        "authType": "auto",
        "authBearer": "",
        "authHeaderName": "",
        "authHeaderValue": "",
        "enabled": true,
    });
    for (name, value) in values.as_object().unwrap() {
        form[name] = value.clone();
    }
    CREATE_MCP_VALIDATOR.validate_as(&form)
}

/// Apply an edit to the row as saved, the way each request does.
async fn edit(app: &TestApp, id: i64, values: Value) -> Result<Mcp, AssignError> {
    let mut mcp = find_mcp(app, id).await;
    assign_mcp_from_payload(&app.state.upstream, &mut mcp, &form(values), Some(id)).await?;
    Ok(mcp)
}

fn refusal(error: AssignError) -> mymcps_vine::ValidationError {
    match error {
        AssignError::Refused(error) => error,
        AssignError::Database(error) => panic!("{error}"),
    }
}

fn saved_environment(app: &TestApp, entries: &[(&str, &str)]) -> Option<String> {
    let entries: Vec<EnvironmentInput> = entries
        .iter()
        .map(|(name, value)| EnvironmentInput {
            name: name.to_string(),
            value: Some(value.to_string()),
        })
        .collect();
    merge_environment(&app.core.encryption, None, &entries)
}

fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

async fn bearer_mcp(app: &TestApp) -> Mcp {
    let admin = create_admin(app).await;
    let token = encrypt(app, "saved-bearer");
    create_mcp(app, admin.id, |mcp| {
        mcp.name = "Repointed MCP".into();
        mcp.auth_type = McpAuthType::Bearer;
        mcp.http_url = Some("https://old.example/mcp".into());
        mcp.auth_bearer = token;
    })
    .await
}

async fn header_mcp(app: &TestApp) -> Mcp {
    let admin = create_admin(app).await;
    let value = encrypt(app, "saved-header-value");
    create_mcp(app, admin.id, |mcp| {
        mcp.name = "Repointed MCP".into();
        mcp.auth_type = McpAuthType::Header;
        mcp.http_url = Some("https://old.example/mcp".into());
        mcp.auth_header_name = Some("X-Api-Key".into());
        mcp.auth_header_value = value;
    })
    .await
}

#[tokio::test]
async fn keeps_a_blank_bearer_token_only_while_the_origin_stays_the_same() {
    let app = TestApp::new().await;
    let id = bearer_mcp(&app).await.id;

    for http_url in [
        "https://old.example/mcp",
        "https://old.example/v2/mcp?tenant=one",
        "https://user:pass@old.example/mcp",
    ] {
        let kept = edit(
            &app,
            id,
            json!({ "httpUrl": http_url, "authType": "bearer" }),
        )
        .await
        .unwrap();
        assert_eq!(
            decrypt(&app, &kept.auth_bearer).as_deref(),
            Some("saved-bearer"),
            "{http_url}"
        );
    }

    for http_url in [
        "https://attacker.example/mcp",
        "https://sub.old.example/mcp",
        "https://old.example:8443/mcp",
        "http://old.example/mcp",
    ] {
        let dropped = edit(
            &app,
            id,
            json!({ "httpUrl": http_url, "authType": "bearer" }),
        )
        .await
        .unwrap();
        assert_eq!(dropped.auth_bearer, None, "{http_url}");
    }
}

#[tokio::test]
async fn stores_a_bearer_token_typed_again_for_the_new_origin() {
    let app = TestApp::new().await;
    let id = bearer_mcp(&app).await.id;

    let moved = edit(
        &app,
        id,
        json!({
            "httpUrl": "https://new.example/mcp",
            "authType": "bearer",
            "authBearer": "bearer-for-new-origin",
        }),
    )
    .await
    .unwrap();

    assert_eq!(
        decrypt(&app, &moved.auth_bearer).as_deref(),
        Some("bearer-for-new-origin")
    );
}

#[tokio::test]
async fn applies_the_same_rule_to_a_custom_header_value() {
    let app = TestApp::new().await;
    let id = header_mcp(&app).await.id;
    let header = |http_url: &str, value: &str| {
        json!({
            "httpUrl": http_url,
            "authType": "header",
            "authHeaderName": "X-Api-Key",
            "authHeaderValue": value,
        })
    };

    let kept = edit(&app, id, header("https://old.example/other", ""))
        .await
        .unwrap();
    assert_eq!(
        decrypt(&app, &kept.auth_header_value).as_deref(),
        Some("saved-header-value")
    );

    let dropped = edit(&app, id, header("https://attacker.example/mcp", ""))
        .await
        .unwrap();
    assert_eq!(dropped.auth_header_value, None);
    assert_eq!(dropped.auth_header_name.as_deref(), Some("X-Api-Key"));

    let retyped = edit(
        &app,
        id,
        header("https://attacker.example/mcp", "value-for-new-origin"),
    )
    .await
    .unwrap();
    assert_eq!(
        decrypt(&app, &retyped.auth_header_value).as_deref(),
        Some("value-for-new-origin")
    );
}

#[tokio::test]
async fn drops_them_when_the_mcp_becomes_an_npm_package() {
    let app = TestApp::new().await;
    let id = bearer_mcp(&app).await.id;

    let npm = edit(
        &app,
        id,
        json!({
            "transport": "npm",
            "npmPackage": "@example/reads-everything",
            "authType": "bearer",
        }),
    )
    .await
    .unwrap();

    assert_eq!(npm.transport, McpTransport::Npm);
    assert_eq!(npm.auth_bearer, None);
    assert_eq!(npm.http_url, None);
}

async fn npm_mcp(app: &TestApp) -> Mcp {
    let admin = create_admin(app).await;
    let environment = saved_environment(
        app,
        &[("API_KEY", "saved-api-key"), ("REGION", "eu-west-3")],
    );
    create_mcp(app, admin.id, |mcp| {
        mcp.name = "Repointed MCP".into();
        mcp.transport = McpTransport::Npm;
        mcp.npm_package = Some("@example/trusted-mcp".into());
        mcp.npm_version = Some("1.0.0".into());
        mcp.npm_env = environment;
    })
    .await
}

fn blank() -> Value {
    json!([{ "name": "API_KEY", "value": "" }, { "name": "REGION", "value": "" }])
}

#[tokio::test]
async fn keeps_blank_values_across_a_version_or_argument_change() {
    let app = TestApp::new().await;
    let id = npm_mcp(&app).await.id;

    let upgraded = edit(
        &app,
        id,
        json!({
            "transport": "npm",
            "npmPackage": "@example/trusted-mcp",
            "npmVersion": "2.0.0",
            "npmArgs": "--verbose",
            "npmEnv": blank(),
        }),
    )
    .await
    .unwrap();

    assert_eq!(
        environment(&app, &upgraded),
        pairs(&[("API_KEY", "saved-api-key"), ("REGION", "eu-west-3")])
    );
    assert_eq!(upgraded.npm_version.as_deref(), Some("2.0.0"));
    assert_eq!(upgraded.npm_args_list(), ["--verbose"]);
}

#[tokio::test]
async fn asks_for_the_values_again_when_the_package_changes() {
    let app = TestApp::new().await;
    let id = npm_mcp(&app).await.id;

    let failure = edit(
        &app,
        id,
        json!({ "transport": "npm", "npmPackage": "@attacker/exfiltrate", "npmEnv": blank() }),
    )
    .await
    .unwrap_err();

    let failure = refusal(failure);
    let message = &failure.messages[0];
    assert_eq!(message.field, "npmEnv.0.value");
    assert_eq!(message.rule, "npmEnvironment");
    assert!(message.message.contains("Enter this value again"));
    // The request is refused, so the saved row is untouched.
    let saved = find_mcp(&app, id).await;
    assert_eq!(
        environment(&app, &saved),
        pairs(&[("API_KEY", "saved-api-key"), ("REGION", "eu-west-3")])
    );
}

#[tokio::test]
async fn hands_the_new_package_only_the_values_typed_for_it() {
    let app = TestApp::new().await;
    let id = npm_mcp(&app).await.id;

    let moved = edit(
        &app,
        id,
        json!({
            "transport": "npm",
            "npmPackage": "@example/other-mcp",
            "npmEnv": [{ "name": "API_KEY", "value": "key-for-other-package" }],
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        environment(&app, &moved),
        pairs(&[("API_KEY", "key-for-other-package")])
    );

    let emptied = edit(
        &app,
        id,
        json!({ "transport": "npm", "npmPackage": "@example/other-mcp" }),
    )
    .await
    .unwrap();
    assert_eq!(emptied.npm_env, None);
}

// --- tests/unit/mcp_environment.spec.ts ---

fn npm_payload(environment: Value) -> Value {
    json!({
        "name": "Environment MCP",
        "transport": "npm",
        "npmPackage": "@example/environment-mcp",
        "npmEnv": environment,
    })
}

#[tokio::test]
async fn encrypts_values_and_only_exposes_variable_names_to_the_page() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mut mcp = Mcp {
        status: McpStatus::Draft,
        created_by: admin.id,
        ..Default::default()
    };

    let payload = form(npm_payload(json!([
        { "name": "API_KEY", "value": "top-secret" },
        { "name": "REGION", "value": "eu-west-3" },
    ])));
    assign_mcp_from_payload(&app.state.upstream, &mut mcp, &payload, None)
        .await
        .unwrap();
    mcp.insert(&*app.core.db).await.unwrap();

    let stored = mcp.npm_env.clone().unwrap();
    assert!(!stored.contains("top-secret"));
    assert!(!stored.contains("eu-west-3"));
    assert_eq!(
        environment(&app, &mcp),
        pairs(&[("API_KEY", "top-secret"), ("REGION", "eu-west-3")])
    );
    assert_eq!(mcp.slug, "environment-mcp");
    assert_eq!(mcp.transport, McpTransport::Npm);

    let view = McpView::of(&app.state.upstream, &mcp, None);
    assert_eq!(view.npm_env, ["API_KEY", "REGION"]);
    assert!(!format!("{view:?}").contains("top-secret"));
    assert!(!format!("{view:?}").contains(&stored));
}

#[tokio::test]
async fn preserves_blank_existing_values_replaces_values_and_removes_omitted_rows() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let environment_saved = saved_environment(
        &app,
        &[("KEEP_ME", "original"), ("REMOVE_ME", "delete-this")],
    );
    let mut mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Editable environment".into();
        mcp.transport = McpTransport::Npm;
        mcp.npm_package = Some("@example/environment-mcp".into());
        mcp.npm_env = environment_saved;
    })
    .await;

    let mut payload = npm_payload(json!([
        { "name": "KEEP_ME", "value": "" },
        { "name": "NEW_VALUE", "value": "replacement" },
    ]));
    payload["name"] = json!("Editable environment");
    let id = mcp.id;
    assign_mcp_from_payload(&app.state.upstream, &mut mcp, &form(payload), Some(id))
        .await
        .unwrap();

    assert_eq!(
        environment(&app, &mcp),
        pairs(&[("KEEP_ME", "original"), ("NEW_VALUE", "replacement")])
    );
    // Its own slug is not taken by itself.
    assert_eq!(mcp.slug, "editable-environment");
}

#[tokio::test]
async fn rejects_blank_new_values_duplicate_names_reserved_names_and_oversized_totals() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let saved = create_mcp(&app, admin.id, |mcp| {
        mcp.transport = McpTransport::Npm;
        mcp.npm_package = Some("@example/environment-mcp".into());
    })
    .await;
    let refused = async |environment: Value| {
        let mut mcp = saved.clone();
        let payload = form(npm_payload(environment));
        let error =
            assign_mcp_from_payload(&app.state.upstream, &mut mcp, &payload, Some(saved.id))
                .await
                .unwrap_err();
        let error = refusal(error);
        (
            error.messages[0].field.clone(),
            error.messages[0].message.clone(),
        )
    };

    assert_eq!(
        refused(json!([{ "name": "NEW_SECRET", "value": "" }])).await,
        (
            "npmEnv.0.value".to_string(),
            "A value is required for a new environment variable".to_string()
        )
    );
    assert_eq!(
        refused(json!([
            { "name": "DUPLICATE", "value": "one" },
            { "name": "DUPLICATE", "value": "two" },
        ]))
        .await,
        (
            "npmEnv.1.name".to_string(),
            "Environment variable names must be unique".to_string()
        )
    );

    // A reserved name does not get past the validator.
    assert!(matches!(
        try_form(npm_payload(
            json!([{ "name": "HOME", "value": "elsewhere" }])
        )),
        Err(mymcps_vine::Error::Validation(_))
    ));

    let oversized: Vec<Value> = (0..9)
        .map(|index| json!({ "name": format!("VALUE_{index}"), "value": "x".repeat(8192) }))
        .collect();
    assert_eq!(
        refused(Value::Array(oversized)).await,
        (
            "npmEnv".to_string(),
            "Environment variables must not exceed 64 KiB in total".to_string()
        )
    );
}

#[tokio::test]
async fn clears_variables_for_http() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let environment_saved = saved_environment(&app, &[("API_KEY", "secret")]);
    let mut mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.transport = McpTransport::Npm;
        mcp.npm_package = Some("@example/environment-mcp".into());
        mcp.npm_version = Some("1.2.3".into());
        mcp.npm_args = Some("[\"--flag\"]".into());
        mcp.npm_env = environment_saved;
    })
    .await;

    let payload = form(json!({
        "name": "Environment MCP",
        "transport": "http",
        "httpUrl": "http://127.0.0.1:9999/mcp",
        "npmPackage": "@example/environment-mcp",
        "npmVersion": "1.2.3",
        "npmArgs": "--flag",
        "npmEnv": [{ "name": "API_KEY", "value": "" }],
    }));
    let id = mcp.id;
    assign_mcp_from_payload(&app.state.upstream, &mut mcp, &payload, Some(id))
        .await
        .unwrap();

    assert_eq!(mcp.npm_env, None);
    // Nothing of the package is left on an HTTP MCP.
    assert_eq!(mcp.npm_package, None);
    assert_eq!(mcp.npm_version, None);
    assert_eq!(mcp.npm_args, None);
}

// --- tests/unit/security.spec.ts ---

#[tokio::test]
async fn allows_endpoint_query_and_url_credentials_while_requiring_an_http_url() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let http = |http_url: &str| try_form(json!({ "name": "Secure HTTP MCP", "httpUrl": http_url }));

    for http_url in ["file:///tmp/socket", "https://example.test/mcp#secret"] {
        assert!(
            matches!(http(http_url), Err(mymcps_vine::Error::Validation(_))),
            "{http_url}"
        );
    }

    let mut mcp = Mcp {
        status: McpStatus::Draft,
        created_by: admin.id,
        ..Default::default()
    };
    let http_url =
        "https://user:password@example.test/mcp?api_key=provider-required&code=fr&key=primary";
    assign_mcp_from_payload(
        &app.state.upstream,
        &mut mcp,
        &http(http_url).unwrap(),
        None,
    )
    .await
    .unwrap();

    assert_eq!(mcp.http_url.as_deref(), Some(http_url));
}

// --- Re-pointing an MCP from the registry ---

const HTTP_FORM: [(&str, &str); 7] = [
    ("name", "Repointed MCP"),
    ("description", ""),
    ("transport", "http"),
    ("npmPackage", ""),
    ("npmVersion", ""),
    ("npmArgs", ""),
    ("enabled", "on"),
];

fn with<'a>(base: &[(&'a str, &'a str)], fields: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut form = base.to_vec();
    form.extend_from_slice(fields);
    form
}

#[tokio::test]
async fn does_not_send_a_saved_bearer_token_to_the_new_origin_when_probing_it() {
    // Records what the gateway sends upstream and answers like an MCP without tools.
    let (app, calls) = app_with_mcp_servers().await;
    let admin = create_admin(&app).await;
    let member = create_member(&app).await;
    let token = encrypt(&app, "saved-bearer");
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Repointed MCP".into();
        mcp.auth_type = McpAuthType::Bearer;
        mcp.http_url = Some("https://old.example/mcp".into());
        mcp.auth_bearer = token;
    })
    .await;

    let response = app
        .put(&format!("/mcps/{}", mcp.id))
        .login_as(&member)
        .csrf()
        .form(&with(
            &HTTP_FORM,
            &[
                ("httpUrl", "https://attacker.example/mcp"),
                ("authType", "bearer"),
                ("authBearer", ""),
            ],
        ))
        .send()
        .await;

    assert_redirect(&response, "/mcps");
    assert!(!calls.is_empty(), "the new origin is probed after saving");
    for call in calls.all() {
        assert_eq!(call.origin(), "https://attacker.example");
        assert_eq!(call.header("authorization"), None);
    }
    let saved = find_mcp(&app, mcp.id).await;
    assert_eq!(saved.auth_bearer, None);
}

#[tokio::test]
async fn still_sends_it_after_a_path_change_within_the_same_origin() {
    let (app, calls) = app_with_mcp_servers().await;
    let admin = create_admin(&app).await;
    let value = encrypt(&app, "saved-header-value");
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Repointed MCP".into();
        mcp.auth_type = McpAuthType::Header;
        mcp.http_url = Some("https://old.example/mcp".into());
        mcp.auth_header_name = Some("X-Api-Key".into());
        mcp.auth_header_value = value;
    })
    .await;

    let response = app
        .put(&format!("/mcps/{}", mcp.id))
        .login_as(&admin)
        .csrf()
        .form(&with(
            &HTTP_FORM,
            &[
                ("httpUrl", "https://old.example/v2/mcp"),
                ("authType", "header"),
                ("authHeaderName", "X-Api-Key"),
                ("authHeaderValue", ""),
            ],
        ))
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert!(!calls.is_empty());
    for call in calls.all() {
        assert_eq!(call.url, "https://old.example/v2/mcp");
        assert_eq!(
            call.header("x-api-key").as_deref(),
            Some("saved-header-value")
        );
    }
}

// --- Sandbox of an npm MCP ---

/// An app whose Deno is never started, and counts how often one was asked
/// for: saving probes the MCP, and these tests are about what happens before
/// Deno starts.
struct Sandboxed {
    app: TestApp,
    starts: Arc<AtomicUsize>,
    admin: User,
    mcp: Mcp,
    /// A file the previous package left in the sandbox.
    state: PathBuf,
}

impl Sandboxed {
    async fn new() -> Self {
        let starts = Arc::new(AtomicUsize::new(0));
        let app = TestApp::with_state(
            |_| {},
            |core| {
                let starts = starts.clone();
                let runtime = DenoRuntime::new(&core.config).binary(move || {
                    starts.fetch_add(1, Ordering::SeqCst);
                    Err(DenoError::Other("Deno is not started in this test".into()))
                });
                let upstream = Upstream::builder(core.clone(), builtins())
                    .deno(DenoRunner::with_runtime(core.clone(), runtime))
                    .build();
                AppState::with_upstream(core, upstream)
            },
        )
        .await;

        let admin = create_admin(&app).await;
        let environment = saved_environment(&app, &[("API_KEY", "saved-api-key")]);
        let mcp = create_mcp(&app, admin.id, |mcp| {
            mcp.name = "Sandboxed MCP".into();
            mcp.transport = McpTransport::Npm;
            mcp.npm_package = Some("@example/trusted-mcp".into());
            mcp.npm_version = Some("1.0.0".into());
            mcp.npm_env = environment;
        })
        .await;
        let root = app.state.upstream.deno().sandbox_root_for(mcp.id);
        std::fs::create_dir_all(root.join(".config")).unwrap();
        let state = root.join(".config").join("session.json");
        std::fs::write(&state, "{\"token\":\"left by the previous package\"}").unwrap();
        Self {
            app,
            starts,
            admin,
            mcp,
            state,
        }
    }

    fn root(&self) -> PathBuf {
        self.app.state.upstream.deno().sandbox_root_for(self.mcp.id)
    }

    fn starts(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }
}

const NPM_FORM: [(&str, &str); 7] = [
    ("name", "Sandboxed MCP"),
    ("description", ""),
    ("transport", "npm"),
    ("httpUrl", ""),
    ("npmArgs", ""),
    ("authType", "auto"),
    ("enabled", "on"),
];

#[tokio::test]
async fn the_sandbox_is_deleted_with_the_mcp() {
    let sandbox = Sandboxed::new().await;

    let response = sandbox
        .app
        .delete(&format!("/mcps/{}", sandbox.mcp.id))
        .login_as(&sandbox.admin)
        .csrf()
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(response.flashed("success"), Some(json!("MCP deleted")));
    assert!(
        Mcp::find(&*sandbox.app.core.db, sandbox.mcp.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!sandbox.state.exists());
    assert!(!sandbox.root().exists());
}

#[tokio::test]
async fn the_sandbox_is_emptied_when_the_mcp_runs_another_package() {
    let sandbox = Sandboxed::new().await;

    let response = sandbox
        .app
        .put(&format!("/mcps/{}", sandbox.mcp.id))
        .login_as(&sandbox.admin)
        .csrf()
        .form(&with(
            &NPM_FORM,
            &[
                ("npmPackage", "@example/other-mcp"),
                ("npmVersion", ""),
                ("npmEnv[0][name]", "API_KEY"),
                ("npmEnv[0][value]", "key-for-other-package"),
            ],
        ))
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    let saved = find_mcp(&sandbox.app, sandbox.mcp.id).await;
    assert_eq!(saved.npm_package.as_deref(), Some("@example/other-mcp"));
    assert!(!sandbox.state.exists());
}

#[tokio::test]
async fn the_sandbox_is_kept_across_a_version_change() {
    let sandbox = Sandboxed::new().await;

    let response = sandbox
        .app
        .put(&format!("/mcps/{}", sandbox.mcp.id))
        .login_as(&sandbox.admin)
        .csrf()
        .form(&with(
            &NPM_FORM,
            &[
                ("npmPackage", "@example/trusted-mcp"),
                ("npmVersion", "2.0.0"),
                ("npmEnv[0][name]", "API_KEY"),
                ("npmEnv[0][value]", ""),
            ],
        ))
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    let saved = find_mcp(&sandbox.app, sandbox.mcp.id).await;
    assert_eq!(saved.npm_version.as_deref(), Some("2.0.0"));
    assert_eq!(
        environment(&sandbox.app, &saved),
        pairs(&[("API_KEY", "saved-api-key")])
    );
    assert!(sandbox.state.is_file());
}

#[tokio::test]
async fn never_starts_another_package_with_the_saved_environment_values() {
    let sandbox = Sandboxed::new().await;
    let member = create_member(&sandbox.app).await;
    let fields = with(
        &NPM_FORM,
        &[
            ("npmPackage", "@attacker/exfiltrate"),
            ("npmVersion", ""),
            ("npmEnv[0][name]", "API_KEY"),
            ("npmEnv[0][value]", ""),
        ],
    );

    let response = sandbox
        .app
        .put(&format!("/mcps/{}", sandbox.mcp.id))
        .login_as(&member)
        .csrf()
        .form(&fields)
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(
        response.flashed("error"),
        Some(json!(
            "Enter this value again: saved values are not passed on to a different package"
        ))
    );
    // The page script is told the same, on the row it is about.
    let scripted = from_script(sandbox.app.put(&format!("/mcps/{}", sandbox.mcp.id)))
        .login_as(&member)
        .csrf()
        .form(&fields)
        .send()
        .await;
    assert_eq!(scripted.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        scripted
            .text()
            .contains("id=\"edit-mcp-env-0-value-error\">Enter this value again")
    );

    assert_eq!(sandbox.starts(), 0);
    let saved = find_mcp(&sandbox.app, sandbox.mcp.id).await;
    assert_eq!(saved.npm_package.as_deref(), Some("@example/trusted-mcp"));
    assert_eq!(
        environment(&sandbox.app, &saved),
        pairs(&[("API_KEY", "saved-api-key")])
    );
    assert!(sandbox.state.is_file());
}

#[tokio::test]
async fn refuses_variables_that_would_reconfigure_the_sandbox_with_a_reason() {
    let sandbox = Sandboxed::new().await;
    let member = create_member(&sandbox.app).await;
    let fields = with(
        &NPM_FORM,
        &[
            ("npmPackage", "@example/trusted-mcp"),
            ("npmVersion", "1.0.0"),
            ("npmEnv[0][name]", "API_KEY"),
            ("npmEnv[0][value]", ""),
            ("npmEnv[1][name]", "PATH"),
            ("npmEnv[1][value]", "/tmp/member-bin"),
            ("npmEnv[2][name]", "LD_PRELOAD"),
            ("npmEnv[2][value]", "/tmp/member.so"),
        ],
    );

    // A plain post is answered with the first reason.
    let response = sandbox
        .app
        .put(&format!("/mcps/{}", sandbox.mcp.id))
        .login_as(&member)
        .csrf()
        .form(&fields)
        .send()
        .await;
    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(
        response.flashed("error"),
        Some(json!(
            "\"PATH\" is set by MyMCPs for the sandbox and cannot be changed"
        ))
    );

    // The page script gets every reason, each on its row.
    let scripted = from_script(sandbox.app.put(&format!("/mcps/{}", sandbox.mcp.id)))
        .login_as(&member)
        .csrf()
        .form(&fields)
        .send()
        .await;
    assert_eq!(scripted.status, StatusCode::UNPROCESSABLE_ENTITY);
    let dialog = scripted.text();
    assert!(dialog.contains(
        "id=\"edit-mcp-env-1-name-error\">&quot;PATH&quot; is set by MyMCPs for the sandbox and cannot be changed</p>"
    ));
    assert!(dialog.contains(
        "id=\"edit-mcp-env-2-name-error\">&quot;LD_PRELOAD&quot; changes how the sandbox process itself is loaded"
    ));

    assert_eq!(sandbox.starts(), 0);
    let saved = find_mcp(&sandbox.app, sandbox.mcp.id).await;
    assert_eq!(saved.npm_env_names(), ["API_KEY"]);
}

// --- Starting an upstream OAuth authorization ---

fn oauth_provider(call: &Call) -> CannedResponse {
    if call.url.contains("/.well-known/oauth-protected-resource") {
        return json_response(json!({
            "resource": "https://mcp.example/mcp",
            "authorization_servers": ["https://auth.example"],
            "scopes_supported": ["read"],
        }));
    }
    match call.url.as_str() {
        "https://auth.example/.well-known/oauth-authorization-server" => {
            json_response(authorization_server("https://auth.example"))
        }
        "https://auth.example/register" => json_response(registered_client("registered-client")),
        _ => not_found(),
    }
}

async fn oauth_mcp(app: &TestApp, name: &str) -> (User, Mcp) {
    let admin = create_admin(app).await;
    let mcp = create_mcp(app, admin.id, |mcp| {
        mcp.name = name.to_string();
        mcp.auth_type = McpAuthType::Auto;
        mcp.oauth_required = true;
        mcp.http_url = Some("https://mcp.example/mcp".into());
        mcp.status = McpStatus::Draft;
    })
    .await;
    (admin, mcp)
}

#[tokio::test]
async fn starting_has_no_effect_for_a_head_request() {
    let (app, calls) = app_answering(oauth_provider).await;
    let (admin, mcp) = oauth_mcp(&app, "OAuth MCP").await;

    let response = app
        .request(Method::HEAD, &format!("/mcps/{}/oauth/start", mcp.id))
        .login_as(&admin)
        .send()
        .await;

    assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.header("allow"), Some("GET"));
    assert!(calls.is_empty());
    let saved = find_mcp(&app, mcp.id).await;
    assert_eq!(saved.oauth_client_id, None);
}

#[tokio::test]
async fn starting_cannot_be_triggered_from_another_site() {
    let (app, calls) = app_answering(oauth_provider).await;
    let (admin, mcp) = oauth_mcp(&app, "OAuth MCP").await;

    for site in ["cross-site", "same-site"] {
        let response = app
            .get(&format!("/mcps/{}/oauth/start?from=elsewhere", mcp.id))
            .header("sec-fetch-site", site)
            .login_as(&admin)
            .send()
            .await;

        assert_eq!(response.status, StatusCode::FOUND, "{site}");
        assert_eq!(response.location(), Some("/mcps"));
        assert_eq!(
            response.flashed("error"),
            Some(json!("Start the OAuth connection from the MCPs page"))
        );
    }
    assert!(calls.is_empty());
    let saved = find_mcp(&app, mcp.id).await;
    assert_eq!(saved.oauth_client_id, None);
    assert_eq!(saved.oauth_issuer, None);
}

#[tokio::test]
async fn starts_for_the_app_itself_a_typed_address_and_clients_that_are_not_browsers() {
    let (app, _calls) = app_answering(oauth_provider).await;

    for site in [Some("same-origin"), Some("none"), None] {
        let (admin, mcp) = oauth_mcp(&app, &format!("OAuth MCP {site:?}")).await;
        let mut request = app
            .get(&format!("/mcps/{}/oauth/start", mcp.id))
            .login_as(&admin);
        if let Some(site) = site {
            request = request.header("sec-fetch-site", site);
        }
        let response = request.send().await;

        assert_eq!(response.status, StatusCode::FOUND, "{site:?}");
        let location = Url::parse(response.location().unwrap()).unwrap();
        assert_eq!(
            location.origin().ascii_serialization(),
            "https://auth.example"
        );
        let saved = find_mcp(&app, mcp.id).await;
        assert_eq!(saved.oauth_client_id.as_deref(), Some("registered-client"));
    }
}

#[tokio::test]
async fn does_not_forward_its_own_query_string_to_the_provider() {
    let (app, _calls) = app_answering(oauth_provider).await;
    let (admin, mcp) = oauth_mcp(&app, "OAuth MCP").await;

    let response = app
        .get(&format!(
            "/mcps/{}/oauth/start?scope=admin&prompt=none&redirect_uri=https%3A%2F%2Fattacker.example%2Fcb",
            mcp.id
        ))
        .login_as(&admin)
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    let location = response.location().unwrap();
    let authorization = Url::parse(location).unwrap();
    assert_eq!(
        authorization.origin().ascii_serialization(),
        "https://auth.example"
    );
    let all = |name: &str| -> Vec<String> {
        authorization
            .query_pairs()
            .filter(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
            .collect()
    };
    assert_eq!(all("scope"), ["read"]);
    assert_eq!(all("redirect_uri"), [CALLBACK]);
    assert!(all("prompt").is_empty());
    assert_eq!(location.split('?').count(), 2);
    assert!(!location.contains("attacker.example"));
    assert!(!location.contains("admin"));
}

#[tokio::test]
async fn refuses_to_start_for_an_mcp_that_does_not_sign_in_with_oauth() {
    let (app, calls) = app_answering(oauth_provider).await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.auth_type = McpAuthType::Bearer;
        mcp.http_url = Some("https://mcp.example/mcp".into());
    })
    .await;

    let response = app
        .get(&format!("/mcps/{}/oauth/start?keep=1", mcp.id))
        .login_as(&admin)
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    // The registry is given the query string, as the Node app's redirects did.
    assert_eq!(response.location(), Some("/mcps?keep=1"));
    assert_eq!(
        response.flashed("error"),
        Some(json!("This MCP does not require OAuth authorization"))
    );
    assert_eq!(response.flashed("editingMcpId"), Some(json!(mcp.id)));
    assert!(calls.is_empty());
}
