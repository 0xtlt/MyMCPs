//! Port of the `npm MCP update endpoint` group of
//! `tests/functional/mcp_npm_update.spec.ts` (its settings group is in
//! `settings.rs`), and of what `tests/browser/mcp_edit_modal_ux.spec.ts`
//! checks of the answer to "Update MCP".

mod mcps_support;

use std::sync::{Arc, Mutex};

use http::StatusCode;
use mcps_support::*;
use mymcps_core::Core;
use mymcps_core::models::{Mcp, McpStatus, McpTransport};
use mymcps_upstream::{NpmUpdateRuntime, Upstream, UpstreamError};
use mymcps_web::AppState;
use mymcps_web::state::builtins;
use mymcps_web::testing::TestApp;
use mymcps_web::testing::factories::{create_admin, create_mcp, create_member};
use serde_json::json;

/// How the update of a test goes.
#[derive(Clone, Copy, PartialEq)]
enum Outcome {
    /// The cache reloads, and the MCP then answers.
    Ready,
    /// The cache reloads, and the MCP then fails to start.
    Broken,
    /// Deno cannot reload the cache.
    ReloadFails,
}

/// The two steps of an update, in place of Deno.
struct Updates {
    core: Arc<Core>,
    outcome: Outcome,
    reloaded: Arc<Mutex<Vec<i64>>>,
}

#[async_trait::async_trait]
impl NpmUpdateRuntime for Updates {
    async fn reload(&self, mcp: &Mcp) -> Result<(), UpstreamError> {
        self.reloaded.lock().unwrap().push(mcp.id);
        if self.outcome == Outcome::ReloadFails {
            let secret = self.core.decrypt_secret(mcp.auth_bearer.as_deref());
            return Err(mymcps_deno::DenoError::CacheReload {
                package: mcp.npm_package.clone().unwrap_or_default(),
                detail: format!("registry said no to {}", secret.unwrap_or_default()),
            }
            .into());
        }
        Ok(())
    }

    async fn probe(&self, mcp: &mut Mcp) -> Result<(), UpstreamError> {
        if self.outcome == Outcome::Broken {
            mcp.status = McpStatus::Error;
            mcp.last_error = Some("The package did not start".into());
        } else {
            mcp.status = McpStatus::Ready;
            mcp.last_error = None;
        }
        mcp.save(&*self.core.db).await?;
        Ok(())
    }
}

async fn app_updating(outcome: Outcome) -> (TestApp, Arc<Mutex<Vec<i64>>>) {
    let reloaded: Arc<Mutex<Vec<i64>>> = Arc::default();
    let app = TestApp::with_state(
        |_| {},
        |core| {
            let upstream = Upstream::builder(core.clone(), builtins())
                .npm_update_runtime(Updates {
                    core: core.clone(),
                    outcome,
                    reloaded: reloaded.clone(),
                })
                .build();
            AppState::with_upstream(core, upstream)
        },
    )
    .await;
    (app, reloaded)
}

#[tokio::test]
async fn requires_authentication() {
    let (app, reloaded) = app_updating(Outcome::Ready).await;
    create_admin(&app).await;

    let response = app.post("/mcps/1/update").csrf().send().await;

    assert_redirect(&response, "/login");
    assert!(reloaded.lock().unwrap().is_empty());
}

#[tokio::test]
async fn reloads_a_latest_tracking_npm_mcp() {
    let (app, reloaded) = app_updating(Outcome::Ready).await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.transport = McpTransport::Npm;
        mcp.npm_package = Some("@example/latest-mcp".into());
        mcp.npm_version = Some("latest".into());
        mcp.status = McpStatus::Error;
        mcp.last_error = Some("stale failure".into());
    })
    .await;

    let response = app
        .post(&format!("/mcps/{}/update", mcp.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;

    assert_redirect(&response, "/mcps");
    assert_eq!(
        response.flashed("success"),
        Some(json!("MCP updated to latest"))
    );
    // The dialog it was asked from opens again.
    assert_eq!(response.flashed("editingMcpId"), Some(json!(mcp.id)));
    assert_eq!(*reloaded.lock().unwrap(), [mcp.id]);

    let saved = find_mcp(&app, mcp.id).await;
    assert_eq!(saved.npm_version.as_deref(), Some("latest"));
    assert_eq!(saved.status, McpStatus::Ready);
    assert_eq!(saved.last_error, None);
}

#[tokio::test]
async fn refuses_http_mcps() {
    let (app, reloaded) = app_updating(Outcome::Ready).await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |mcp| mcp.transport = McpTransport::Http).await;

    let response = app
        .post(&format!("/mcps/{}/update", mcp.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(
        response.flashed("error"),
        Some(json!("Only Deno npm MCPs can be updated"))
    );
    assert!(reloaded.lock().unwrap().is_empty());
}

#[tokio::test]
async fn refuses_pinned_npm_versions_without_changing_them() {
    let (app, reloaded) = app_updating(Outcome::Ready).await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.transport = McpTransport::Npm;
        mcp.npm_package = Some("@example/pinned-mcp".into());
        mcp.npm_version = Some("1.14.4".into());
    })
    .await;

    let response = app
        .post(&format!("/mcps/{}/update", mcp.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;

    assert_eq!(response.status, StatusCode::FOUND);
    assert_eq!(
        response.flashed("error"),
        Some(json!("Pinned npm versions are not updated"))
    );
    assert!(reloaded.lock().unwrap().is_empty());

    let saved = find_mcp(&app, mcp.id).await;
    assert_eq!(saved.npm_version.as_deref(), Some("1.14.4"));
}

#[tokio::test]
async fn says_why_an_update_did_not_leave_the_mcp_ready() {
    // The package updates, then does not start.
    let (app, _reloaded) = app_updating(Outcome::Broken).await;
    let member = create_member(&app).await;
    let mcp = create_mcp(&app, member.id, |mcp| {
        mcp.transport = McpTransport::Npm;
        mcp.npm_package = Some("@example/latest-mcp".into());
    })
    .await;

    let response = app
        .post(&format!("/mcps/{}/update", mcp.id))
        .login_as(&member)
        .csrf()
        .send()
        .await;
    assert_redirect(&response, "/mcps");
    assert_eq!(
        response.flashed("error"),
        Some(json!("The package did not start"))
    );
    assert_eq!(response.flashed("success"), None);
    assert_eq!(response.flashed("editingMcpId"), Some(json!(mcp.id)));

    // Deno cannot reload the cache: the reason is shown without the secrets of the MCP.
    let (app, _reloaded) = app_updating(Outcome::ReloadFails).await;
    let admin = create_admin(&app).await;
    let token = encrypt(&app, "registry-token-value");
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.transport = McpTransport::Npm;
        mcp.npm_package = Some("@example/latest-mcp".into());
        mcp.auth_bearer = token;
    })
    .await;

    let response = app
        .post(&format!("/mcps/{}/update", mcp.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_eq!(
        response.flashed("error"),
        Some(json!(
            "Failed to reload Deno cache for \"@example/latest-mcp\". registry said no to [REDACTED]"
        ))
    );
    assert_eq!(response.flashed("editingMcpId"), Some(json!(mcp.id)));
}

#[tokio::test]
async fn answers_the_row_and_the_dialog_that_asked_for_the_update() {
    let (app, reloaded) = app_updating(Outcome::Ready).await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Shopify Dev".into();
        mcp.transport = McpTransport::Npm;
        mcp.npm_package = Some("@shopify/dev-mcp".into());
        mcp.npm_version = Some("latest".into());
    })
    .await;
    let path = format!("/mcps/{}/update", mcp.id);

    // A row of the list reads the result in the list, as it was.
    let from_list = app
        .post(&path)
        .login_as(&admin)
        .csrf()
        .header("host", "localhost:3333")
        .header("referer", "http://localhost:3333/mcps?transport=npm")
        .form(&[("from", "list")])
        .send()
        .await;
    assert_eq!(from_list.status, StatusCode::FOUND);
    assert_eq!(from_list.location(), Some("/mcps?transport=npm"));
    assert_eq!(
        from_list.flashed("success"),
        Some(json!("MCP updated to latest"))
    );
    assert_eq!(from_list.flashed("editingMcpId"), None);

    // The dialog sends its form through the page script, which is told where to go.
    let scripted = from_script(app.post(&path))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_eq!(scripted.status, StatusCode::NO_CONTENT);
    assert_eq!(scripted.header("x-location"), Some("/mcps"));
    assert_eq!(
        scripted.flashed("success"),
        Some(json!("MCP updated to latest"))
    );
    assert_eq!(scripted.flashed("editingMcpId"), Some(json!(mcp.id)));
    assert_eq!(*reloaded.lock().unwrap(), [mcp.id, mcp.id]);

    // The registry then shows the message over the dialog, open again.
    let page = app
        .get("/mcps")
        .login_as(&admin)
        .session(scripted.session())
        .send()
        .await
        .text();
    assert!(page.contains("<p class=\"toast__message\">MCP updated to latest</p>"));
    assert!(page.contains("data-open data-dialog-trigger="));
    assert!(page.contains("Edit Shopify Dev"));

    let missing = app
        .post("/mcps/999/update")
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_redirect(&missing, "/mcps");
    assert_eq!(missing.flashed("error"), Some(json!("MCP not found")));
}
