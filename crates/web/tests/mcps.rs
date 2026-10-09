//! The MCPs page: the registry with its search, filters and pages, the
//! actions of a row, and creating, editing and deleting an HTTP or npm MCP.
//!
//! The Node app had no functional spec of its own for this page: these cases
//! port what `vine_route_params.spec.ts` and `public_url_config.spec.ts`
//! check of it, what the browser specs check that the server decides
//! (`mcp_template_gallery`, `mcp_environment`, `mcp_edit_modal_ux`,
//! `mcp_oauth_pasted_callback`, `hardening_upstream_credentials`,
//! `public_url_config`), and cover the parts of the page that are new.

mod mcps_support;

use http::StatusCode;
use mcps_support::*;
use mymcps_core::models::{Mcp, McpAuthType, McpStatus, McpTransport};
use mymcps_core::secrets::{EnvironmentInput, merge_environment};
use mymcps_deno::{DenoRunner, DenoRuntime, HostEnvironment};
use mymcps_upstream::Upstream;
use mymcps_web::AppState;
use mymcps_web::state::builtins;
use mymcps_web::testing::factories::{create_admin, create_mcp, create_member};
use mymcps_web::testing::{TestApp, TestResponse};
use serde_json::json;

/// What an MCP form posts for an HTTP MCP, with `fields` over it.
fn http_form<'a>(fields: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut form = vec![
        ("name", "Docs"),
        ("description", ""),
        ("transport", "http"),
        ("httpUrl", "https://docs.example/mcp"),
        ("authType", "auto"),
        ("enabled", "on"),
    ];
    for (name, value) in fields {
        form.retain(|(existing, _)| existing != name);
        form.push((name, value));
    }
    form
}

fn environment_of(app: &TestApp, entries: &[(&str, &str)]) -> Option<String> {
    let entries: Vec<EnvironmentInput> = entries
        .iter()
        .map(|(name, value)| EnvironmentInput {
            name: name.to_string(),
            value: Some(value.to_string()),
        })
        .collect();
    merge_environment(&app.core.encryption, None, &entries)
}

async fn page(app: &TestApp, user: &mymcps_core::models::User, path: &str) -> TestResponse {
    let response = app.get(path).login_as(user).send().await;
    assert_eq!(response.status, StatusCode::OK, "{path}");
    response
}

/// The rows of the table, one piece of the page for each.
fn table_rows(page: &str) -> Vec<&str> {
    let body = between(page, "<tbody>", "</tbody>");
    body.split("<tr>").skip(1).collect()
}

// --- the registry ---

#[tokio::test]
async fn sends_a_visitor_to_sign_in() {
    let app = TestApp::new().await;
    create_admin(&app).await;

    assert_redirect(&app.get("/mcps").send().await, "/login");
    assert_redirect(&app.get("/mcps/new").send().await, "/login");
    assert_redirect(&app.get("/mcps/1/edit").send().await, "/login");
    assert_redirect(
        &app.post("/mcps").csrf().form(&http_form(&[])).send().await,
        "/login",
    );
    assert_redirect(&app.post("/mcps/1/toggle").csrf().send().await, "/login");
    assert_eq!(count_mcps(&app).await, 0);
}

#[tokio::test]
async fn invites_to_add_the_first_mcp() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let page = page(&app, &admin, "/mcps").await.text();

    assert!(page.contains("<title>MCPs · MyMCPs</title>"));
    assert!(page.contains(
        "Register upstream MCP servers. Agents reach them through MyMCPs with an access token."
    ));
    assert!(page.contains("No MCPs yet"));
    assert!(page.contains("Create an MCP to start routing agent traffic."));
    // Nothing to search or to filter yet.
    assert!(!page.contains("role=\"search\""));
    assert!(!page.contains("<table"));
    // The gallery is in the page, closed, for the button to open.
    assert!(page.contains("href=\"/mcps/new\" data-dialog-open=\"#add-mcp\""));
    assert!(page.contains("<dialog class=\"dialog dialog--xl\" id=\"add-mcp\" aria-labelledby=\"add-mcp-title\" data-dialog-return=\"/mcps\">"));
}

#[tokio::test]
async fn lists_the_mcps_by_name_with_what_each_one_is() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let notion = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Notion".into();
        mcp.slug = "notion".into();
        mcp.http_url = Some("https://mcp.notion.com/mcp".into());
    })
    .await;
    let shopify = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Shopify Dev".into();
        mcp.slug = "shopify-dev".into();
        mcp.transport = McpTransport::Npm;
        mcp.npm_package = Some("@shopify/dev-mcp".into());
        mcp.npm_version = Some("latest".into());
    })
    .await;
    let broken = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Broken".into();
        mcp.slug = "broken".into();
        mcp.auth_type = McpAuthType::Bearer;
        mcp.http_url = Some("https://broken.example/mcp".into());
        mcp.status = McpStatus::Error;
        mcp.last_error = Some("connect ECONNREFUSED <b>".into());
        mcp.enabled = false;
    })
    .await;

    let page = page(&app, &admin, "/mcps").await.text();
    let rows = table_rows(&page);

    assert_eq!(rows.len(), 3);
    assert!(rows[0].contains("<span class=\"cell-title\">Broken</span>"));
    assert!(rows[1].contains("<span class=\"cell-title\">Notion</span>"));
    assert!(rows[2].contains("<span class=\"cell-title\">Shopify Dev</span>"));
    assert!(page.contains("3 MCPs · 2 enabled"));
    assert!(page.contains("1–3 of 3"));
    assert!(page.contains("25 rows per page"));

    // Status, slug, endpoint, sign-in and the icon of its template.
    let notion_row = rows[1];
    assert!(notion_row.contains("<span class=\"status status--success\">ready</span>"));
    assert!(notion_row.contains("<span class=\"cell-sub cell-sub--code\">notion</span>"));
    assert!(notion_row.contains("<span class=\"cell-code\">https://mcp.notion.com/mcp</span>"));
    assert!(notion_row.contains("<code class=\"code\">auto</code>"));
    assert!(notion_row.contains(&mymcps_web::views::icon("notebook-text").into_string()));
    // No tool was listed since the server started.
    assert!(notion_row.contains("<td class=\"cell-tertiary\">—</td>"));
    assert!(notion_row.contains(&format!("action=\"/mcps/{}/toggle\"", notion.id)));
    assert!(notion_row.contains("role=\"switch\" name=\"enabled\" value=\"false\" aria-checked=\"true\" aria-label=\"Notion enabled\""));
    assert!(notion_row.contains(&format!(
        "id=\"mcp-{id}-edit\" href=\"/mcps/{id}/edit\" data-dialog-open=\"#edit-mcp\" data-dialog-fetch data-dialog-history>Edit</a>",
        id = notion.id
    )));

    // The menu of a row, in the order of the design.
    let menu = between(notion_row, "role=\"menu\"", "</td>");
    let position = |text: &str| {
        menu.find(text)
            .unwrap_or_else(|| panic!("{text} is not in the menu"))
    };
    assert!(position("Test connection") < position("Tool approvals"));
    assert!(position("Tool approvals") < position("Disable"));
    assert!(position("Disable") < position("Delete"));
    assert!(menu.contains(&format!("action=\"/mcps/{}/probe\"", notion.id)));
    assert!(menu.contains(&format!("href=\"/mcps/{}/tools\"", notion.id)));
    assert!(menu.contains(&format!(
        "action=\"/mcps/{}?_method=DELETE\" data-confirm=\"Delete Notion?\" data-confirm-label=\"Delete\" data-confirm-tone=\"critical\"",
        notion.id
    )));
    // Neither OAuth nor an npm package: nothing to connect or to update.
    assert!(!menu.contains("Update MCP"));
    assert!(!menu.contains("Re-authorize"));
    assert!(!menu.contains("Connect"));

    // An npm MCP that follows the latest version can be updated.
    let shopify_row = rows[2];
    assert!(shopify_row.contains("<span class=\"cell-code\">@shopify/dev-mcp</span>"));
    assert!(shopify_row.contains(&format!("action=\"/mcps/{}/update\"", shopify.id)));
    assert!(shopify_row.contains("Update MCP"));
    assert!(shopify_row.contains(&mymcps_web::views::icon("shopping-bag").into_string()));

    // An MCP in error says why, and a disabled one can be enabled.
    let broken_row = rows[0];
    assert!(broken_row.contains("<span class=\"status status--critical\">error</span>"));
    assert!(broken_row.contains("<code class=\"code\">bearer</code>"));
    assert!(broken_row.contains("cell-sub--critical\" title=\"connect ECONNREFUSED &lt;b&gt;\">connect ECONNREFUSED &lt;b&gt;</span>"));
    assert!(
        broken_row.contains("value=\"true\" aria-checked=\"false\" aria-label=\"Broken enabled\"")
    );
    assert!(broken_row.contains("Enable"));
    assert!(broken_row.contains(&mymcps_web::views::icon("plug").into_string()));

    // Under 768px the same MCPs are a list whose rows open the edit dialog.
    let list = between(&page, "id=\"registered-mcps-title\"", "</section>");
    assert!(list.contains("<span class=\"list-row__subtitle\">notion · HTTP</span>"));
    assert!(list.contains("<span class=\"list-row__subtitle\">shopify-dev · npm</span>"));
    assert!(list.contains("<span class=\"list-row__subtitle\">broken · HTTP · off</span>"));
    assert!(list.contains("<span class=\"badge badge--success\">ready</span>"));
    assert!(list.contains("<span class=\"badge\">error</span>"));
    assert!(list.contains(&format!(
        "<a class=\"list-row\" href=\"/mcps/{}/edit\" data-dialog-open=\"#edit-mcp\"",
        broken.id
    )));
}

#[tokio::test]
async fn a_member_manages_mcps_like_an_administrator() {
    let (app, _calls) = app_with_mcp_servers().await;
    let admin = create_admin(&app).await;
    let member = create_member(&app).await;
    create_mcp(&app, admin.id, |mcp| mcp.name = "Shared".into()).await;

    let page = page(&app, &member, "/mcps").await.text();
    assert!(page.contains("<span class=\"cell-title\">Shared</span>"));
    assert!(page.contains("Add MCP"));

    let created = app
        .post("/mcps")
        .login_as(&member)
        .csrf()
        .form(&http_form(&[("name", "From a member")]))
        .send()
        .await;
    assert_redirect(&created, "/mcps");
    assert_eq!(created.flashed("success"), Some(json!("MCP created")));
    let mcp = mcp_by_slug(&app, "from-a-member").await.unwrap();
    assert_eq!(mcp.created_by, member.id);
}

#[tokio::test]
async fn shows_how_many_tools_an_mcp_listed() {
    let (app, _calls) = app_answering(|call| {
        mcp_answer(
            call,
            &json!([
                { "name": "search", "inputSchema": { "type": "object" } },
                { "name": "fetch", "inputSchema": { "type": "object" } },
            ]),
        )
    })
    .await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Docs".into();
        mcp.http_url = Some("https://docs.example/mcp".into());
    })
    .await;

    let before = page(&app, &admin, "/mcps").await.text();
    assert!(table_rows(&before)[0].contains("<td class=\"cell-tertiary\">—</td>"));

    let probed = app
        .post(&format!("/mcps/{}/probe", mcp.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_eq!(probed.flashed("success"), Some(json!("Connection OK")));

    let after = page(&app, &admin, "/mcps").await.text();
    assert!(table_rows(&after)[0].contains("<td class=\"cell-secondary tabular\">2</td>"));
}

#[tokio::test]
async fn searches_and_filters_the_registry() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Notion".into();
        mcp.slug = "notion".into();
        mcp.description = Some("Pages and databases".into());
        mcp.http_url = Some("https://mcp.notion.com/mcp".into());
    })
    .await;
    create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Everything".into();
        mcp.slug = "everything".into();
        mcp.transport = McpTransport::Npm;
        mcp.npm_package = Some("@modelcontextprotocol/server-everything".into());
        mcp.status = McpStatus::Error;
        mcp.enabled = false;
    })
    .await;
    create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Context7".into();
        mcp.slug = "context7".into();
        mcp.auth_type = McpAuthType::Bearer;
        mcp.http_url = Some("https://mcp.context7.com/mcp".into());
        mcp.status = McpStatus::Draft;
    })
    .await;
    create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Strava".into();
        mcp.slug = "strava".into();
        mcp.transport = McpTransport::Builtin;
        mcp.builtin_key = Some("strava".into());
    })
    .await;
    create_mcp(&app, admin.id, |mcp| {
        mcp.name = "iCloud Mail".into();
        mcp.slug = "icloud-mail".into();
        mcp.transport = McpTransport::Builtin;
        mcp.builtin_key = Some("icloud-mail".into());
    })
    .await;

    let names = |page: &str| -> Vec<String> {
        table_rows(page)
            .iter()
            .map(|row| {
                between(row, "<span class=\"cell-title\">", "</span>")
                    ["<span class=\"cell-title\">".len()..]
                    .to_string()
            })
            .collect()
    };
    let listed = async |path: &str| page(&app, &admin, path).await.text();

    let all = listed("/mcps").await;
    assert_eq!(
        names(&all),
        ["Context7", "Everything", "Notion", "Strava", "iCloud Mail"]
    );
    assert!(all.contains("5 MCPs · 4 enabled"));
    // Every filter is a chip that opens a menu of links.
    assert!(all.contains(
        "<button type=\"button\" class=\"chip hide-mobile\" popovertarget=\"filter-status\">Status"
    ));
    assert!(all.contains("<a class=\"menu__item\" role=\"menuitemradio\" aria-checked=\"true\" href=\"/mcps\">All statuses</a>"));
    assert!(all.contains("<a class=\"menu__item\" role=\"menuitemradio\" aria-checked=\"false\" href=\"/mcps?status=error\">error</a>"));
    assert!(all.contains("href=\"/mcps?transport=builtin\">Built-in</a>"));
    assert!(all.contains("href=\"/mcps?auth=password\">password</a>"));

    // The search looks at the name, the slug, the description and the endpoint.
    assert_eq!(names(&listed("/mcps?q=NOTION").await), ["Notion"]);
    assert_eq!(names(&listed("/mcps?q=databases").await), ["Notion"]);
    assert_eq!(
        names(&listed("/mcps?q=server-everything").await),
        ["Everything"]
    );
    assert_eq!(names(&listed("/mcps?q=context7.com").await), ["Context7"]);
    assert_eq!(names(&listed("/mcps?q=imap").await), ["iCloud Mail"]);

    let errors = listed("/mcps?status=error").await;
    assert_eq!(names(&errors), ["Everything"]);
    assert!(errors.contains("1 MCP · 0 enabled"));
    assert!(errors.contains("<button type=\"button\" class=\"chip__label\" popovertarget=\"filter-status\">Status: error</button>"));
    assert!(
        errors.contains(
            "<a class=\"chip__clear\" href=\"/mcps\" aria-label=\"Clear status filter\">"
        )
    );
    // The other filters keep this one, and the search form too.
    assert!(errors.contains("href=\"/mcps?status=error&amp;transport=npm\">npm</a>"));
    assert!(errors.contains("<input type=\"hidden\" name=\"status\" value=\"error\">"));

    assert_eq!(
        names(&listed("/mcps?transport=builtin").await),
        ["Strava", "iCloud Mail"]
    );
    assert_eq!(names(&listed("/mcps?transport=npm").await), ["Everything"]);
    // A built-in MCP is listed under the way its provider signs in.
    assert_eq!(names(&listed("/mcps?auth=oauth").await), ["Strava"]);
    assert_eq!(names(&listed("/mcps?auth=password").await), ["iCloud Mail"]);
    assert_eq!(names(&listed("/mcps?auth=bearer").await), ["Context7"]);
    assert_eq!(
        names(&listed("/mcps?auth=auto").await),
        ["Everything", "Notion"]
    );
    assert_eq!(
        names(&listed("/mcps?q=o&status=ready&transport=http&auth=auto").await),
        ["Notion"]
    );

    // Nothing matches: the card says so, and the filters can be cleared.
    let nothing = listed("/mcps?q=zzz&status=draft").await;
    assert!(nothing.contains("No MCPs match"));
    assert!(nothing.contains("Try another search or clear the filters."));
    assert!(
        nothing.contains("<a class=\"button button--secondary\" href=\"/mcps\">Clear filters</a>")
    );
    assert!(nothing.contains("name=\"q\" value=\"zzz\""));
    assert!(!nothing.contains("<table"));

    // A filter the page does not know is ignored, not refused.
    let unknown = listed("/mcps?status=broken&transport=npm&page=abc").await;
    assert_eq!(names(&unknown), ["Everything"]);
    assert!(!unknown.contains("Status:"));
}

#[tokio::test]
async fn shows_25_mcps_a_page() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    for number in 1..=27 {
        create_mcp(&app, admin.id, |mcp| {
            mcp.name = format!("MCP {number:02}");
            mcp.slug = format!("mcp-{number:02}");
            mcp.enabled = number != 27;
        })
        .await;
    }
    let listed = async |path: &str| page(&app, &admin, path).await.text();

    let first = listed("/mcps").await;
    assert_eq!(table_rows(&first).len(), 25);
    assert!(first.contains("27 MCPs · 26 enabled"));
    assert!(first.contains("<span class=\"pagination__range\">1–25 of 27</span>"));
    assert!(first.contains("<a class=\"icon-button icon-button--secondary\" aria-disabled=\"true\" aria-label=\"Previous page\">"));
    assert!(first.contains("<a class=\"icon-button icon-button--secondary\" href=\"/mcps?page=2\" aria-label=\"Next page\">"));
    assert!(first.contains("MCP 25") && !first.contains("MCP 26"));

    let second = listed("/mcps?page=2").await;
    assert_eq!(table_rows(&second).len(), 2);
    assert!(second.contains("<span class=\"pagination__range\">26–27 of 27</span>"));
    assert!(second.contains("href=\"/mcps\" aria-label=\"Previous page\">"));
    assert!(second.contains("aria-disabled=\"true\" aria-label=\"Next page\">"));
    // What a row opens keeps the page it is on.
    assert!(second.contains("/edit?page=2\" data-dialog-open=\"#edit-mcp\""));
    assert!(second.contains("data-dialog-return=\"/mcps?page=2\""));

    // A page past the last one is the last one.
    let beyond = listed("/mcps?page=9").await;
    assert!(beyond.contains("<span class=\"pagination__range\">26–27 of 27</span>"));

    // A filter starts again on its first page, and pages keep the filter.
    let filtered = listed("/mcps?transport=http&page=2").await;
    assert!(filtered.contains("href=\"/mcps?transport=http\" aria-label=\"Previous page\">"));
    assert!(filtered.contains("href=\"/mcps?status=ready&amp;transport=http\">ready</a>"));
}

// --- the actions of a row ---

#[tokio::test]
async fn switches_an_mcp_on_and_off_from_its_row() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |mcp| mcp.name = "Docs".into()).await;
    let toggle = async |fields: &[(&str, &str)]| {
        app.post(&format!("/mcps/{}/toggle", mcp.id))
            .login_as(&admin)
            .csrf()
            .form(fields)
            .send()
            .await
    };

    let off = toggle(&[("enabled", "false")]).await;
    assert_redirect(&off, "/mcps");
    assert_eq!(off.flashed("success"), Some(json!("MCP disabled")));
    assert!(!find_mcp(&app, mcp.id).await.enabled);
    // The edit dialog does not open for it.
    assert_eq!(off.flashed("editingMcpId"), None);

    // A page that still shows the switch on asks for the same state again.
    let again = toggle(&[("enabled", "false")]).await;
    assert_eq!(again.flashed("success"), Some(json!("MCP disabled")));
    assert!(!find_mcp(&app, mcp.id).await.enabled);

    let on = toggle(&[("enabled", "true")]).await;
    assert_eq!(on.flashed("success"), Some(json!("MCP enabled")));
    assert!(find_mcp(&app, mcp.id).await.enabled);

    // Without a state, the MCP is switched over.
    toggle(&[]).await;
    assert!(!find_mcp(&app, mcp.id).await.enabled);

    // The list is given back as it was.
    let back = app
        .post(&format!("/mcps/{}/toggle", mcp.id))
        .login_as(&admin)
        .csrf()
        .header("host", "localhost:3333")
        .header("referer", "http://localhost:3333/mcps?status=ready&page=2")
        .form(&[("enabled", "true")])
        .send()
        .await;
    assert_eq!(back.location(), Some("/mcps?status=ready&page=2"));
    // Nothing else of the MCP changed.
    let saved = find_mcp(&app, mcp.id).await;
    assert_eq!(saved.status, McpStatus::Ready);
    assert_eq!(saved.http_url.as_deref(), Some("http://127.0.0.1:9999/mcp"));

    let missing = app
        .post("/mcps/999/toggle")
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_redirect(&missing, "/mcps");
    assert_eq!(missing.flashed("error"), Some(json!("MCP not found")));
}

#[tokio::test]
async fn an_id_that_is_not_a_number_is_answered_like_a_missing_record() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    for (method, path) in [
        ("delete", "/mcps/abc"),
        ("post", "/mcps/1.5/probe"),
        ("put", "/mcps/-3"),
        ("post", "/mcps/12abc/update"),
        ("post", "/mcps/0/toggle"),
        ("get", "/mcps/abc"),
        ("get", "/mcps/abc/edit"),
        ("get", "/mcps/abc/oauth/start"),
    ] {
        let request = match method {
            "delete" => app.delete(path),
            "put" => app.put(path).form(&http_form(&[])),
            "post" => app.post(path),
            _ => app.get(path),
        };
        let response = request.login_as(&admin).csrf().send().await;
        assert_redirect(&response, "/mcps");
        assert_eq!(
            response.flashed("error"),
            Some(json!("MCP not found")),
            "{method} {path}"
        );
    }
}

#[tokio::test]
async fn a_record_is_still_found_by_its_id() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |_| {}).await;

    let response = app
        .delete(&format!("/mcps/{}", mcp.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;

    assert_redirect(&response, "/mcps");
    assert_eq!(response.flashed("success"), Some(json!("MCP deleted")));
    assert!(Mcp::find(&*app.core.db, mcp.id).await.unwrap().is_none());
}

#[tokio::test]
async fn tests_a_connection_and_says_what_it_found() {
    let (app, calls) = app_answering(|call| {
        if call.hostname() == "down.example" {
            return status(503).body("upstream <unavailable>");
        }
        mcp_answer(call, &json!([]))
    })
    .await;
    let admin = create_admin(&app).await;
    let ready = create_mcp(&app, admin.id, |mcp| {
        mcp.http_url = Some("https://up.example/mcp".into());
        mcp.status = McpStatus::Draft;
    })
    .await;
    let down = create_mcp(&app, admin.id, |mcp| {
        mcp.http_url = Some("https://down.example/mcp".into());
    })
    .await;
    let probe = async |id: i64, fields: &[(&str, &str)]| {
        app.post(&format!("/mcps/{id}/probe"))
            .login_as(&admin)
            .csrf()
            .form(fields)
            .send()
            .await
    };

    // From the edit dialog, which opens again on the result.
    let ok = probe(ready.id, &[]).await;
    assert_redirect(&ok, "/mcps");
    assert_eq!(ok.flashed("success"), Some(json!("Connection OK")));
    assert_eq!(ok.flashed("editingMcpId"), Some(json!(ready.id)));
    assert_eq!(find_mcp(&app, ready.id).await.status, McpStatus::Ready);
    assert!(
        calls
            .all()
            .iter()
            .all(|call| call.hostname() == "up.example")
    );

    let failed = probe(down.id, &[]).await;
    assert_redirect(&failed, "/mcps");
    let saved = find_mcp(&app, down.id).await;
    assert_eq!(saved.status, McpStatus::Error);
    assert_eq!(failed.flashed("error"), Some(json!(saved.last_error)));
    assert_eq!(failed.flashed("success"), None);
    assert_eq!(failed.flashed("editingMcpId"), Some(json!(down.id)));

    // From a row of the list, the result is read in the list.
    let from_list = probe(ready.id, &[("from", "list")]).await;
    assert_redirect(&from_list, "/mcps");
    assert_eq!(from_list.flashed("success"), Some(json!("Connection OK")));
    assert_eq!(from_list.flashed("editingMcpId"), None);

    // The page script is told where to go instead of being redirected.
    let scripted = from_script(app.post(&format!("/mcps/{}/probe", ready.id)))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_eq!(scripted.status, StatusCode::NO_CONTENT);
    assert_eq!(scripted.header("x-location"), Some("/mcps"));
    assert_eq!(scripted.flashed("editingMcpId"), Some(json!(ready.id)));
}

// --- the pages whose address means a dialog is open ---

#[tokio::test]
async fn answers_the_address_of_an_mcp_by_opening_its_dialog() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |mcp| mcp.name = "Docs".into()).await;

    // `GET /mcps/{id}` answers as the Node app did.
    let shown = app
        .get(&format!("/mcps/{}", mcp.id))
        .login_as(&admin)
        .send()
        .await;
    assert_redirect(&shown, "/mcps");
    assert_eq!(shown.flashed("editingMcpId"), Some(json!(mcp.id)));
    assert_eq!(shown.flashed("error"), None);

    let missing = app.get("/mcps/999").login_as(&admin).send().await;
    assert_redirect(&missing, "/mcps");
    assert_eq!(missing.flashed("error"), Some(json!("MCP not found")));
    assert_eq!(missing.flashed("editingMcpId"), None);

    // The registry then draws the dialog open.
    let opened = app
        .get("/mcps")
        .login_as(&admin)
        .session(shown.session())
        .send()
        .await
        .text();
    assert!(opened.contains(&format!(
        "<dialog class=\"dialog\" id=\"edit-mcp\" aria-labelledby=\"edit-mcp-title\" data-dialog-static data-dialog-return=\"/mcps\" data-open data-dialog-trigger=\"#mcp-{}-edit\">",
        mcp.id
    )));
    assert!(opened.contains("<h2 class=\"dialog__title\" id=\"edit-mcp-title\">Edit Docs</h2>"));

    // Without it, the dialog is there, closed and empty.
    let closed = page(&app, &admin, "/mcps").await.text();
    assert!(closed.contains("<dialog class=\"dialog\" id=\"edit-mcp\" aria-labelledby=\"edit-mcp-title\" data-dialog-static data-dialog-return=\"/mcps\"><div data-fragment></div></dialog>"));

    // A flashed id that is not one the server wrote opens nothing.
    for forged in [json!("7"), json!(1.5), json!({ "id": mcp.id }), json!(999)] {
        let mut session = serde_json::Map::new();
        session.insert("__flash__".into(), json!({ "editingMcpId": forged }));
        let page = app
            .get("/mcps")
            .login_as(&admin)
            .session(session)
            .send()
            .await
            .text();
        assert!(
            page.contains("<div data-fragment></div></dialog>"),
            "{forged}"
        );
    }
}

#[tokio::test]
async fn opens_the_edit_dialog_at_its_own_address() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Docs".into();
        mcp.slug = "docs".into();
        mcp.description = Some("Search the docs".into());
        mcp.http_url = Some("https://docs.example/mcp".into());
    })
    .await;
    create_mcp(&app, admin.id, |mcp| mcp.name = "Other".into()).await;
    let path = format!("/mcps/{}/edit", mcp.id);

    // A full page: the registry, and the dialog open over it.
    let full = page(&app, &admin, &format!("{path}?transport=http"))
        .await
        .text();
    assert!(full.contains("<span class=\"cell-title\">Other</span>"));
    assert!(full.contains("data-dialog-return=\"/mcps?transport=http\" data-open"));
    let dialog = between(&full, "id=\"edit-mcp\"", "</dialog>");
    assert!(dialog.contains("<p class=\"dialog__subtitle\">docs</p>"));
    assert!(dialog.contains(&format!(
        "<form class=\"dialog__body\" id=\"edit-mcp-form\" method=\"post\" action=\"/mcps/{}?_method=PUT\" data-async>",
        mcp.id
    )));
    assert!(dialog.contains("name=\"name\" type=\"text\" value=\"Docs\""));
    assert!(dialog.contains("name=\"description\" type=\"text\" value=\"Search the docs\""));
    assert!(dialog.contains("name=\"transport\" value=\"http\" checked"));
    assert!(dialog.contains("name=\"httpUrl\" type=\"text\" value=\"https://docs.example/mcp\""));
    assert!(dialog.contains("name=\"authType\" value=\"auto\" checked"));
    assert!(dialog.contains("name=\"enabled\" checked"));
    assert!(dialog.contains("<span class=\"status status--success push-end\">ready</span>"));
    // The other requests of the dialog are forms of their own.
    assert!(dialog.contains(&format!(
        "<form id=\"edit-mcp-probe\" method=\"post\" action=\"/mcps/{}/probe\" data-async hidden>",
        mcp.id
    )));
    assert!(dialog.contains(&format!(
        "<form id=\"edit-mcp-delete\" method=\"post\" action=\"/mcps/{}?_method=DELETE\" data-confirm=\"Delete Docs?\"",
        mcp.id
    )));
    assert!(dialog.contains("form=\"edit-mcp-delete\">Delete</button>"));
    assert!(dialog.contains("form=\"edit-mcp-form\">Save changes</button>"));
    assert!(dialog.contains(&format!("href=\"/mcps/{}/tools\"", mcp.id)));
    // Nothing went wrong, nothing to connect: no banner.
    assert!(!dialog.contains("class=\"banner"));

    // The page script asks for the content of the dialog alone.
    let fragment = from_script(app.get(&path))
        .header("x-fragment", "edit-mcp")
        .login_as(&admin)
        .send()
        .await;
    assert_eq!(fragment.status, StatusCode::OK);
    let fragment = fragment.text();
    assert!(fragment.starts_with("<div class=\"dialog__header\">"));
    assert!(fragment.contains("Edit Docs"));
    assert!(!fragment.contains("<html"));
    assert!(!fragment.contains("cell-title"));

    // An MCP that is gone sends the script back to the registry.
    let gone = from_script(app.get("/mcps/999/edit"))
        .header("x-fragment", "edit-mcp")
        .login_as(&admin)
        .send()
        .await;
    assert_eq!(gone.status, StatusCode::NO_CONTENT);
    assert_eq!(gone.header("x-location"), Some("/mcps"));
    assert_eq!(gone.flashed("error"), Some(json!("MCP not found")));
}

#[tokio::test]
async fn opens_on_the_last_error_and_says_it_once() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Broken".into();
        mcp.status = McpStatus::Error;
        mcp.last_error = Some("HTTP 503 from upstream".into());
    })
    .await;

    let mut session = serde_json::Map::new();
    session.insert(
        "__flash__".into(),
        json!({ "editingMcpId": mcp.id, "error": "HTTP 503 from upstream" }),
    );
    let page = app
        .get("/mcps")
        .login_as(&admin)
        .session(session)
        .send()
        .await
        .text();

    let dialog = between(&page, "id=\"edit-mcp\"", "</dialog>");
    assert!(dialog.contains("<div class=\"banner banner--critical\" role=\"alert\">"));
    assert!(dialog.contains(
        "<p class=\"banner__title\">Last connection error</p><p>HTTP 503 from upstream</p>"
    ));
    assert!(dialog.contains("<span class=\"status status--critical push-end\">error</span>"));
    // The banner says it: no toast repeats it.
    assert!(!page.contains("<p class=\"toast__message\">HTTP 503 from upstream</p>"));

    // Another message is still shown.
    let mut session = serde_json::Map::new();
    session.insert(
        "__flash__".into(),
        json!({ "editingMcpId": mcp.id, "error": "Update failed" }),
    );
    let page = app
        .get("/mcps")
        .login_as(&admin)
        .session(session)
        .send()
        .await
        .text();
    assert!(page.contains("<p class=\"toast__message\">Update failed</p>"));
}

// --- the gallery of templates (tests/browser/mcp_template_gallery.spec.ts) ---

#[tokio::test]
async fn offers_the_templates_of_the_gallery() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let page = page(&app, &admin, "/mcps/new").await.text();
    let gallery = between(&page, "id=\"add-mcp\"", "</dialog>");

    // The address means the gallery is open.
    assert!(page.contains("<dialog class=\"dialog dialog--xl\" id=\"add-mcp\" aria-labelledby=\"add-mcp-title\" data-open data-dialog-return=\"/mcps\">"));
    assert!(gallery.contains("Start from a trusted template or configure your own server"));
    assert_eq!(
        gallery.matches("<article class=\"template-card\"").count(),
        19
    );
    // Without the page script the gallery shows its first tab: Popular.
    assert_eq!(
        gallery.matches("data-filter-item").count()
            - gallery
                .matches("\" hidden><div class=\"template-card__header\">")
                .count(),
        11
    );
    assert!(gallery.contains(
        "data-filter-count data-singular=\"template\" data-plural=\"templates\">11 templates</p>"
    ));
    for name in [
        "Notion",
        "Shopify Dev",
        "Atlassian Rovo",
        "GitHub",
        "Supabase",
        "Postman",
        "Microsoft Learn",
        "Google Ads",
    ] {
        assert!(
            gallery.contains(&format!("<h3 class=\"template-card__name\">{name}</h3>")),
            "{name}"
        );
    }

    // Every category is a tab of the filter, Marketing included.
    for (value, label) in [
        ("popular", "Popular"),
        ("all", "All"),
        ("productivity", "Productivity"),
        ("development", "Development"),
        ("commerce", "Commerce"),
        ("marketing", "Marketing"),
        ("infrastructure", "Infrastructure"),
        ("health", "Health &amp; fitness"),
    ] {
        assert!(
            gallery.contains(&format!(
                "role=\"tab\" data-filter-value=\"{value}\" aria-selected=\"{}\"",
                value == "popular"
            )),
            "{value}"
        );
        assert!(gallery.contains(&format!(">{label}</button>")), "{label}");
    }

    // What the search and the tabs look at.
    let shopify = between(gallery, "data-filter-text=\"shopify dev", "</article>");
    assert!(shopify.contains("data-filter-tags=\"popular commerce\""));
    assert!(shopify.contains("<p class=\"template-card__category\">Commerce</p>"));
    // A popular template says so next to its action, where a badge has room.
    assert!(shopify.contains("<a class=\"button button--secondary\" href=\"/mcps/new?template=shopify-dev\">Set up Shopify Dev</a><span class=\"badge badge--brand badge--no-dot\">Popular</span>"));
    assert!(!shopify.contains("Built-in"));
    let stripe = between(gallery, "data-filter-text=\"stripe", "</article>");
    assert!(stripe.contains("data-filter-tags=\"commerce\" hidden"));
    assert!(!stripe.contains("Popular"));
    // A built-in MCP says so in its header, and the search finds it by that word.
    for (text, category, tags) in [
        ("strava", "Health &amp; fitness", "popular health"),
        ("icloud mail", "Productivity", "popular productivity"),
        ("google ads", "Marketing", "popular marketing"),
    ] {
        let card = between(
            gallery,
            &format!("data-filter-text=\"{text} "),
            "</article>",
        );
        assert!(
            card.contains(&format!("built-in\" data-filter-tags=\"{tags}\"")),
            "{text}"
        );
        assert!(
            card.contains(&format!(
                "<p class=\"template-card__category\">{category}</p></div><span class=\"badge badge--info badge--no-dot\">Built-in</span>"
            )),
            "{text}"
        );
        assert!(card.contains("<span class=\"badge badge--brand badge--no-dot\">Popular</span>"));
    }
    assert_eq!(gallery.matches(">Built-in</span>").count(), 3);
    assert_eq!(gallery.matches(">Popular</span>").count(), 11);

    assert!(gallery.contains("No templates found"));
    assert!(gallery.contains("href=\"/mcps/new?template=custom\">"));
    assert!(gallery.contains("Custom MCP"));
    // A template that does not exist is the gallery again.
    let unknown = page_of(&app, &admin, "/mcps/new?template=nope").await;
    assert!(unknown.contains("id=\"add-mcp\" aria-labelledby=\"add-mcp-title\" data-open"));
    assert!(!unknown.contains("id=\"new-mcp\""));
}

async fn page_of(app: &TestApp, user: &mymcps_core::models::User, path: &str) -> String {
    page(app, user, path).await.text()
}

#[tokio::test]
async fn prefills_the_setup_form_of_a_template() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;

    let shopify = page_of(&app, &admin, "/mcps/new?template=shopify-dev").await;
    let dialog = between(&shopify, "id=\"new-mcp\"", "</dialog>");
    assert!(shopify.contains("<dialog class=\"dialog\" id=\"new-mcp\" aria-labelledby=\"new-mcp-title\" data-open data-dialog-static data-dialog-return=\"/mcps\">"));
    // The gallery stays in the page, closed.
    assert!(shopify.contains(
        "id=\"add-mcp\" aria-labelledby=\"add-mcp-title\" data-dialog-return=\"/mcps\">"
    ));
    assert!(
        dialog.contains("<h2 class=\"dialog__title\" id=\"new-mcp-title\">Set up Shopify Dev</h2>")
    );
    assert!(dialog.contains("Review the prefilled settings, then add this MCP"));
    assert!(dialog.contains("<form class=\"dialog__body\" id=\"new-mcp-form\" method=\"post\" action=\"/mcps\" data-async>"));
    assert!(dialog.contains("<input type=\"hidden\" name=\"template\" value=\"shopify-dev\">"));
    assert!(dialog.contains("name=\"name\" type=\"text\" value=\"Shopify Dev\""));
    assert!(dialog.contains("name=\"npmPackage\" type=\"text\" value=\"@shopify/dev-mcp\""));
    assert!(dialog.contains("name=\"npmVersion\" type=\"text\" value=\"latest\""));
    assert!(dialog.contains("name=\"transport\" value=\"npm\" checked"));
    assert!(!dialog.contains("name=\"transport\" value=\"http\" checked"));
    // The fields of the other transport are in the form, hidden.
    assert!(dialog.contains("<div class=\"field\" data-show-when=\"transport=http\" hidden>"));
    assert!(dialog.contains("<div class=\"stack\" data-show-when=\"transport=npm\">"));
    assert!(dialog.contains("href=\"/mcps/new\">"));
    assert!(dialog.contains("Back to templates"));
    assert!(dialog.contains("form=\"new-mcp-form\">Add MCP</button>"));

    let atlassian = page_of(&app, &admin, "/mcps/new?template=atlassian-rovo").await;
    assert!(atlassian.contains(
        "name=\"httpUrl\" type=\"text\" value=\"https://mcp.atlassian.com/v1/mcp/authv2\""
    ));
    assert!(atlassian.contains("<div class=\"field\" data-show-when=\"transport=http\">"));

    let mongodb = page_of(&app, &admin, "/mcps/new?template=mongodb").await;
    assert!(mongodb.contains("name=\"npmPackage\" type=\"text\" value=\"mongodb-mcp-server\""));
    assert!(mongodb.contains("name=\"npmArgs\" type=\"text\" value=\"--readOnly\""));
    assert!(
        mongodb
            .contains("name=\"npmEnv[0][name]\" type=\"text\" value=\"MDB_MCP_CONNECTION_STRING\"")
    );
    // Its value is the admin's to enter.
    assert!(
        mongodb
            .contains("name=\"npmEnv[0][value]\" type=\"password\" autocomplete=\"off\" required>")
    );

    let context7 = page_of(&app, &admin, "/mcps/new?template=context7").await;
    assert!(context7.contains("name=\"authType\" value=\"bearer\" checked"));
    assert!(context7.contains("<div class=\"field\" data-show-when=\"authType=bearer\">"));

    // A custom MCP starts from nothing: HTTP, Auto, enabled, the name to type.
    let custom = page_of(&app, &admin, "/mcps/new?template=custom&status=error").await;
    let dialog = between(&custom, "id=\"new-mcp\"", "</dialog>");
    assert!(dialog.contains(">Set up a custom MCP</h2>"));
    assert!(dialog.contains("Register an HTTP or npm upstream server"));
    assert!(!dialog.contains("name=\"template\""));
    assert!(dialog.contains("name=\"name\" type=\"text\" autocomplete=\"off\" autofocus>"));
    assert!(dialog.contains("name=\"transport\" value=\"http\" checked"));
    assert!(dialog.contains("name=\"authType\" value=\"auto\" checked"));
    assert!(dialog.contains("name=\"enabled\" checked"));
    assert!(dialog.contains("<div class=\"repeat-list\" data-repeat-list></div>"));
    assert!(!dialog.contains("class=\"banner"));
    // The list behind the dialog stays as it was.
    assert!(custom.contains("data-dialog-return=\"/mcps?status=error\""));
    assert!(dialog.contains("href=\"/mcps/new?status=error\">"));
}

// --- creating, editing and deleting ---

#[tokio::test]
async fn creates_an_http_mcp_and_tests_it() {
    let (app, calls) = app_with_mcp_servers().await;
    let admin = create_admin(&app).await;

    let response = app
        .post("/mcps")
        .login_as(&admin)
        .csrf()
        .form(&http_form(&[
            ("name", "  My Docs!  "),
            ("description", "Search the docs"),
            ("httpUrl", "HTTPS://Docs.Example/mcp?tenant=one"),
            ("authType", "bearer"),
            ("authBearer", "docs-token"),
        ]))
        .send()
        .await;

    assert_redirect(&response, "/mcps");
    assert_eq!(response.flashed("success"), Some(json!("MCP created")));
    assert_eq!(response.flashed("editingMcpId"), None);
    let mcp = mcp_by_slug(&app, "my-docs").await.unwrap();
    assert_eq!(mcp.name, "My Docs!");
    assert_eq!(mcp.description.as_deref(), Some("Search the docs"));
    assert_eq!(mcp.transport, McpTransport::Http);
    // The URL is saved in its canonical form.
    assert_eq!(
        mcp.http_url.as_deref(),
        Some("https://docs.example/mcp?tenant=one")
    );
    assert_eq!(mcp.auth_type, McpAuthType::Bearer);
    assert_ne!(mcp.auth_bearer.as_deref(), Some("docs-token"));
    assert_eq!(
        decrypt(&app, &mcp.auth_bearer).as_deref(),
        Some("docs-token")
    );
    assert!(mcp.enabled);
    assert_eq!(mcp.created_by, admin.id);
    assert_eq!(mcp.status, McpStatus::Ready);
    assert_eq!(mcp.last_error, None);
    assert_eq!(mcp.npm_package, None);
    assert_eq!(mcp.npm_env, None);
    // It was tested with the token it was given.
    assert!(!calls.is_empty());
    for call in calls.all() {
        assert_eq!(call.url, "https://docs.example/mcp?tenant=one");
        assert_eq!(
            call.header("authorization").as_deref(),
            Some("Bearer docs-token")
        );
    }

    // A second MCP of the same name gets a slug of its own.
    app.post("/mcps")
        .login_as(&admin)
        .csrf()
        .form(&http_form(&[("name", "My docs"), ("enabled", "")]))
        .send()
        .await;
    let second = mcp_by_slug(&app, "my-docs-2").await.unwrap();
    assert!(!second.enabled);
    assert_eq!(second.auth_bearer, None);
    assert_eq!(count_mcps(&app).await, 2);
}

#[tokio::test]
async fn reopens_the_dialog_of_an_mcp_that_wants_oauth() {
    let (app, _calls) = app_answering(|call| {
        if call.hostname() == "mcp.example" {
            return json_status(401, json!({ "error": "invalid_token" }));
        }
        not_found()
    })
    .await;
    let admin = create_admin(&app).await;

    let response = app
        .post("/mcps")
        .login_as(&admin)
        .csrf()
        .form(&http_form(&[
            ("name", "OAuth MCP"),
            ("httpUrl", "https://mcp.example/mcp"),
        ]))
        .send()
        .await;

    assert_redirect(&response, "/mcps");
    assert_eq!(response.flashed("success"), Some(json!("MCP created")));
    let mcp = mcp_by_slug(&app, "oauth-mcp").await.unwrap();
    assert_eq!(mcp.status, McpStatus::Draft);
    assert!(mcp.oauth_required);
    assert_eq!(
        mcp.last_error.as_deref(),
        Some("OAuth authorization required")
    );
    // Something is left to do: the dialog opens again.
    assert_eq!(response.flashed("editingMcpId"), Some(json!(mcp.id)));

    let page = app
        .get("/mcps")
        .login_as(&admin)
        .session(response.session())
        .send()
        .await
        .text();
    let dialog = between(&page, "id=\"edit-mcp\"", "</dialog>");
    assert!(page.contains("<p class=\"toast__message\">MCP created</p>"));
    assert!(dialog.contains("<div class=\"banner banner--warning\" role=\"status\">"));
    assert!(dialog.contains("<p class=\"banner__title\">Authorization required</p><p>This MCP requires OAuth. Connect your account to finish setup.</p>"));
    assert!(dialog.contains(&format!(
        "<a class=\"button button--secondary\" href=\"/mcps/{}/oauth/start\">Connect</a>",
        mcp.id
    )));
    assert!(dialog.contains("<span class=\"status status--warning push-end\">draft</span>"));
    // The row offers it too.
    assert!(table_rows(&page)[0].contains(&format!(
        "<a class=\"menu__item\" role=\"menuitem\" href=\"/mcps/{}/oauth/start\">",
        mcp.id
    )));
}

#[tokio::test]
async fn sends_a_refused_form_back_with_what_is_wrong() {
    let (app, calls) = app_with_mcp_servers().await;
    let admin = create_admin(&app).await;
    let wrong = [
        ("name", ""),
        ("description", "Kept"),
        ("httpUrl", "ftp://docs.example/mcp"),
        ("authType", "header"),
        ("authHeaderName", "X Api Key"),
        ("authHeaderValue", "typed-header-value"),
        ("template", "notion"),
    ];

    // A plain post goes back with the first error.
    let plain = app
        .post("/mcps")
        .login_as(&admin)
        .csrf()
        .header("host", "localhost:3333")
        .header("referer", "http://localhost:3333/mcps/new?template=notion")
        .form(&http_form(&wrong))
        .send()
        .await;
    assert_eq!(plain.status, StatusCode::FOUND);
    assert_eq!(plain.location(), Some("/mcps/new?template=notion"));
    assert_eq!(
        plain.flashed("error"),
        Some(json!("The name field must be defined"))
    );
    // Nothing of the form is kept in the session.
    assert!(!format!("{:?}", plain.session()).contains("typed-header-value"));

    // The page script gets the dialog again, with every field that is wrong.
    let scripted = from_script(app.post("/mcps"))
        .login_as(&admin)
        .csrf()
        .form(&http_form(&wrong))
        .send()
        .await;
    assert_eq!(scripted.status, StatusCode::UNPROCESSABLE_ENTITY);
    let dialog = scripted.text();
    assert!(dialog.starts_with("<div class=\"dialog__header\">"));
    assert!(dialog.contains(">Set up Notion</h2>"));
    assert!(dialog.contains("name=\"name\" type=\"text\" autocomplete=\"off\" aria-invalid=\"true\" aria-describedby=\"new-mcp-name-error\"><p class=\"field__error\" id=\"new-mcp-name-error\">The name field must be defined</p>"));
    assert!(dialog.contains("value=\"ftp://docs.example/mcp\""));
    assert!(dialog.contains(
        "<p class=\"field__error\" id=\"new-mcp-http-url-error\">MCP URL must use HTTP or HTTPS</p>"
    ));
    assert!(dialog.contains("<p class=\"field__error\" id=\"new-mcp-auth-header-name-error\">The authHeaderName field format is invalid</p>"));
    // What was typed comes back, so nothing has to be typed twice.
    assert!(dialog.contains("name=\"description\" type=\"text\" value=\"Kept\""));
    assert!(dialog.contains("name=\"authHeaderName\" type=\"text\" value=\"X Api Key\""));
    assert!(
        dialog.contains("name=\"authHeaderValue\" type=\"password\" value=\"typed-header-value\"")
    );
    assert!(dialog.contains("name=\"authType\" value=\"header\" checked"));
    // The first field to correct takes the focus, not the name by default.
    assert!(!dialog.contains("autofocus"));

    assert_eq!(count_mcps(&app).await, 0);
    assert!(calls.is_empty());

    // A refusal no field can show is said above the form.
    let transport = from_script(app.post("/mcps"))
        .login_as(&admin)
        .csrf()
        .form(&http_form(&[("transport", "ftp"), ("authType", "oauth")]))
        .send()
        .await
        .text();
    assert!(transport.contains("<p class=\"banner__title\">This MCP could not be saved</p><p>The selected transport is invalid</p>"));
    assert!(transport.contains("<p>The selected authType is invalid</p>"));
}

#[tokio::test]
async fn updates_an_mcp_and_tests_it_again() {
    let (app, calls) = app_with_mcp_servers().await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Docs".into();
        mcp.slug = "docs".into();
        mcp.http_url = Some("https://docs.example/mcp".into());
        mcp.status = McpStatus::Error;
        mcp.last_error = Some("old failure".into());
    })
    .await;
    let other = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Guides".into();
        mcp.slug = "guides".into();
    })
    .await;

    let response = app
        .put(&format!("/mcps/{}", mcp.id))
        .login_as(&admin)
        .csrf()
        .form(&http_form(&[
            ("name", "Guides"),
            ("description", "Now with guides"),
            ("httpUrl", "https://docs.example/v2/mcp"),
            ("enabled", ""),
        ]))
        .send()
        .await;

    assert_redirect(&response, "/mcps");
    assert_eq!(response.flashed("success"), Some(json!("MCP updated")));
    assert_eq!(response.flashed("editingMcpId"), None);
    let saved = find_mcp(&app, mcp.id).await;
    assert_eq!(saved.name, "Guides");
    // The slug of another MCP is taken, its own would not be.
    assert_eq!(saved.slug, "guides-2");
    assert_eq!(saved.description.as_deref(), Some("Now with guides"));
    assert_eq!(
        saved.http_url.as_deref(),
        Some("https://docs.example/v2/mcp")
    );
    assert!(!saved.enabled);
    assert_eq!(saved.status, McpStatus::Ready);
    assert_eq!(saved.last_error, None);
    assert!(
        calls
            .all()
            .iter()
            .all(|call| call.url == "https://docs.example/v2/mcp")
    );
    assert_eq!(find_mcp(&app, other.id).await.slug, "guides");

    // Saving it again keeps the slug it has.
    app.put(&format!("/mcps/{}", mcp.id))
        .login_as(&admin)
        .csrf()
        .form(&http_form(&[
            ("name", "Guides"),
            ("httpUrl", "https://docs.example/v2/mcp"),
        ]))
        .send()
        .await;
    assert_eq!(find_mcp(&app, mcp.id).await.slug, "guides-2");

    // A refused edit is drawn from the row as it is saved, with what was typed.
    let refused = from_script(app.put(&format!("/mcps/{}", mcp.id)))
        .login_as(&admin)
        .csrf()
        .form(&http_form(&[
            ("name", "Typed name"),
            ("httpUrl", "not a url"),
        ]))
        .send()
        .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    let dialog = refused.text();
    assert!(dialog.contains("<h2 class=\"dialog__title\" id=\"edit-mcp-title\">Edit Guides</h2>"));
    assert!(dialog.contains("name=\"name\" type=\"text\" value=\"Typed name\""));
    assert!(dialog.contains("<p class=\"field__error\" id=\"edit-mcp-http-url-error\">The httpUrl field must be a valid URL</p>"));
    assert_eq!(find_mcp(&app, mcp.id).await.name, "Guides");

    let missing = app
        .put("/mcps/999")
        .login_as(&admin)
        .csrf()
        .form(&http_form(&[]))
        .send()
        .await;
    assert_redirect(&missing, "/mcps");
    assert_eq!(missing.flashed("error"), Some(json!("MCP not found")));
}

// --- the environment of an npm MCP (tests/browser/mcp_environment.spec.ts) ---

/// The app around an upstream whose Deno is never found: saving probes the
/// MCP, and these tests are about what happens before Deno starts.
async fn app_without_deno() -> TestApp {
    TestApp::with_state(
        |_| {},
        |core| {
            let runtime = DenoRuntime::new(&core.config).binary(|| {
                Err(mymcps_deno::DenoError::Other(
                    "Deno is not started in this test".into(),
                ))
            });
            let upstream = Upstream::builder(core.clone(), builtins())
                .deno(DenoRunner::with_runtime(core.clone(), runtime))
                .build();
            AppState::with_upstream(core, upstream)
        },
    )
    .await
}

fn npm_form<'a>(fields: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut form = vec![
        ("name", "Browser environment MCP"),
        ("description", ""),
        ("transport", "npm"),
        ("httpUrl", ""),
        ("npmPackage", "@example/environment-mcp"),
        ("npmVersion", ""),
        ("npmArgs", ""),
        ("authType", "auto"),
        ("enabled", "on"),
    ];
    for (name, value) in fields {
        form.retain(|(existing, _)| existing != name);
        form.push((name, value));
    }
    form
}

#[tokio::test]
async fn saves_the_environment_rows_of_the_form() {
    let app = app_without_deno().await;
    let admin = create_admin(&app).await;

    // The rows the page script adds are numbered from a template.
    let custom = page_of(&app, &admin, "/mcps/new?template=custom").await;
    assert!(custom.contains(
        "<fieldset class=\"field-group\" data-repeat data-repeat-min=\"0\" data-repeat-max=\"50\">"
    ));
    assert!(custom.contains("Values are encrypted and only provided to this MCP process."));
    let template = between(&custom, "<template data-repeat-template>", "</template>");
    assert!(
        template.contains("name=\"npmEnv[__i__][name]\" type=\"text\" placeholder=\"API_KEY\"")
    );
    assert!(template.contains("for=\"new-mcp-env-__i__-value\">Value</label>"));
    assert!(template.contains(
        "name=\"npmEnv[__i__][value]\" type=\"password\" autocomplete=\"off\" required>"
    ));
    assert!(
        template.contains(
            "data-repeat-remove aria-label=\"Remove environment variable\">Remove</button>"
        )
    );
    assert!(custom.contains("data-repeat-add>"));

    // Rows keep their order whatever index the script gave them, also past
    // the ones the body parser reads as a list.
    let response = app
        .post("/mcps")
        .login_as(&admin)
        .csrf()
        .form(&npm_form(&[
            ("npmArgs", "--verbose  --port 8080"),
            ("npmEnv[31][name]", "REGION"),
            ("npmEnv[31][value]", "eu-west-3"),
            ("npmEnv[2][name]", "API_KEY"),
            ("npmEnv[2][value]", "secret-one"),
        ]))
        .send()
        .await;

    assert_redirect(&response, "/mcps");
    assert_eq!(response.flashed("success"), Some(json!("MCP created")));
    let mcp = mcp_by_slug(&app, "browser-environment-mcp").await.unwrap();
    assert_eq!(mcp.transport, McpTransport::Npm);
    assert_eq!(mcp.npm_package.as_deref(), Some("@example/environment-mcp"));
    assert_eq!(mcp.npm_version, None);
    assert_eq!(mcp.npm_args_list(), ["--verbose", "--port", "8080"]);
    assert_eq!(mcp.http_url, None);
    let saved = mcp.npm_env.clone().unwrap();
    assert!(!saved.contains("secret-one") && !saved.contains("eu-west-3"));
    assert_eq!(
        environment(&app, &mcp),
        [
            ("API_KEY".to_string(), "secret-one".to_string()),
            ("REGION".to_string(), "eu-west-3".to_string()),
        ]
    );
    // Deno could not start: the MCP is saved, in error.
    assert_eq!(mcp.status, McpStatus::Error);

    // The registry and the edit dialog name the variables and never show a value.
    let edit = page_of(&app, &admin, &format!("/mcps/{}/edit", mcp.id)).await;
    assert!(!edit.contains("secret-one") && !edit.contains("eu-west-3"));
    assert!(!edit.contains(&saved));
    let dialog = between(&edit, "id=\"edit-mcp\"", "</dialog>");
    assert!(dialog.contains("name=\"npmEnv[0][name]\" type=\"text\" value=\"API_KEY\""));
    assert!(dialog.contains("name=\"npmEnv[1][name]\" type=\"text\" value=\"REGION\""));
    assert!(dialog.contains("aria-label=\"Remove API_KEY\">Remove</button>"));
    assert!(dialog.contains("name=\"npmArgs\" type=\"text\" value=\"--verbose --port 8080\""));
}

#[tokio::test]
async fn refuses_environment_rows_the_form_cannot_save() {
    let app = app_without_deno().await;
    let admin = create_admin(&app).await;
    let refused = async |fields: &[(&str, &str)]| {
        let response = from_script(app.post("/mcps"))
            .login_as(&admin)
            .csrf()
            .form(&npm_form(fields))
            .send()
            .await;
        assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
        response.text()
    };

    // A new variable needs a value, and its row says so.
    let blank = refused(&[("npmEnv[0][name]", "NEW_SECRET"), ("npmEnv[0][value]", "")]).await;
    assert!(blank.contains("name=\"npmEnv[0][name]\" type=\"text\" value=\"NEW_SECRET\""));
    assert!(blank.contains("<p class=\"field__error\" id=\"new-mcp-env-0-value-error\">A value is required for a new environment variable</p>"));

    let duplicate = refused(&[
        ("npmEnv[0][name]", "DUPLICATE"),
        ("npmEnv[0][value]", "one"),
        ("npmEnv[1][name]", "DUPLICATE"),
        ("npmEnv[1][value]", "two"),
    ])
    .await;
    assert!(duplicate.contains("<p class=\"field__error\" id=\"new-mcp-env-1-name-error\">Environment variable names must be unique</p>"));
    // What was typed is in the form again.
    assert!(duplicate.contains("name=\"npmEnv[1][value]\" type=\"password\" value=\"two\""));

    let reserved = refused(&[
        ("npmEnv[0][name]", "HOME"),
        ("npmEnv[0][value]", "elsewhere"),
    ])
    .await;
    assert!(
        reserved
            .contains("&quot;HOME&quot; is set by MyMCPs for the sandbox and cannot be changed")
    );

    // A refusal about the whole list is said above the form.
    let value = "x".repeat(8192);
    let mut oversized: Vec<(String, String)> = Vec::new();
    for index in 0..9 {
        oversized.push((format!("npmEnv[{index}][name]"), format!("VALUE_{index}")));
        oversized.push((format!("npmEnv[{index}][value]"), value.clone()));
    }
    let oversized: Vec<(&str, &str)> = oversized
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let too_large = refused(&oversized).await;
    assert!(too_large.contains("<p class=\"banner__title\">Environment variables error</p><p>Environment variables must not exceed 64 KiB in total</p>"));

    assert_eq!(count_mcps(&app).await, 0);
}

// --- saved credentials in the edit dialog (tests/browser/hardening_upstream_credentials.spec.ts) ---

#[tokio::test]
async fn offers_to_keep_a_saved_bearer_token_only_for_the_same_origin() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let token = encrypt(&app, "saved-bearer");
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Bearer MCP".into();
        mcp.auth_type = McpAuthType::Bearer;
        mcp.http_url = Some("https://old.example/mcp".into());
        mcp.auth_bearer = token.clone();
    })
    .await;

    let page = page_of(&app, &admin, &format!("/mcps/{}/edit", mcp.id)).await;
    let dialog = between(&page, "id=\"edit-mcp\"", "</dialog>");

    // The saved token is never in the page, as text or as ciphertext.
    assert!(!page.contains("saved-bearer"));
    assert!(!page.contains(token.as_deref().unwrap()));
    assert!(dialog.contains("name=\"authBearer\" type=\"password\" autocomplete=\"off\""));

    // The label offers to keep it while the URL has the origin it was saved
    // for: the page script follows what is typed with this condition.
    let keeps = "transport=http&amp;httpUrl:origin=https://old.example";
    assert!(dialog.contains(&format!(
        "for=\"edit-mcp-auth-bearer\">Bearer token<span data-show-when=\"{keeps}\"> (leave blank to keep)</span> <span class=\"field__optional\" data-show-when=\"{keeps}\">Optional</span></label>"
    )));
    // Once it has another one, the form says the saved token stays behind.
    assert!(dialog.contains(&format!(
        "<p class=\"field__help\" id=\"edit-mcp-auth-bearer-help\" data-hide-when=\"{keeps}\" hidden>The saved token is not sent to a different server. Enter the token for this one.</p>"
    )));

    // An MCP without a saved token has nothing to keep.
    let plain = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "No token".into();
        mcp.auth_type = McpAuthType::Bearer;
    })
    .await;
    let page = page_of(&app, &admin, &format!("/mcps/{}/edit", plain.id)).await;
    let dialog = between(&page, "id=\"edit-mcp\"", "</dialog>");
    assert!(dialog.contains("for=\"edit-mcp-auth-bearer\">Bearer token</label>"));
    assert!(!dialog.contains("leave blank to keep"));
    assert!(!dialog.contains("The saved token is not sent"));

    // A form sent back refused for another origin already says so.
    let moved = from_script(app.put(&format!("/mcps/{}", mcp.id)))
        .login_as(&admin)
        .csrf()
        .form(&http_form(&[
            ("name", ""),
            ("httpUrl", "https://attacker.example/mcp"),
            ("authType", "bearer"),
        ]))
        .send()
        .await
        .text();
    assert!(moved.contains(&format!(
        "<span data-show-when=\"{keeps}\" hidden> (leave blank to keep)</span>"
    )));
    assert!(moved.contains(&format!(
        "data-hide-when=\"{keeps}\">The saved token is not sent to a different server."
    )));
    // The same server at another path still receives the saved token.
    let same = from_script(app.put(&format!("/mcps/{}", mcp.id)))
        .login_as(&admin)
        .csrf()
        .form(&http_form(&[
            ("name", ""),
            ("httpUrl", "https://old.example/v2/mcp"),
            ("authType", "bearer"),
        ]))
        .send()
        .await
        .text();
    assert!(same.contains(&format!(
        "<span data-show-when=\"{keeps}\"> (leave blank to keep)</span>"
    )));
}

#[tokio::test]
async fn offers_to_keep_a_saved_header_value_the_same_way() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let value = encrypt(&app, "saved-header-value");
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Header MCP".into();
        mcp.auth_type = McpAuthType::Header;
        mcp.auth_header_name = Some("X-Api-Key".into());
        mcp.auth_header_value = value.clone();
        mcp.http_url = Some("https://old.example:8443/mcp".into());
    })
    .await;

    let page = page_of(&app, &admin, &format!("/mcps/{}/edit", mcp.id)).await;
    let dialog = between(&page, "id=\"edit-mcp\"", "</dialog>");

    assert!(!page.contains("saved-header-value"));
    assert!(!page.contains(value.as_deref().unwrap()));
    assert!(dialog.contains("name=\"authHeaderName\" type=\"text\" value=\"X-Api-Key\""));
    let keeps = "transport=http&amp;httpUrl:origin=https://old.example:8443";
    assert!(dialog.contains(&format!(
        ">Header value<span data-show-when=\"{keeps}\"> (leave blank to keep)</span>"
    )));
    assert!(dialog.contains(&format!(
        "data-hide-when=\"{keeps}\" hidden>The saved value is not sent to a different server. Enter the value for this one.</p>"
    )));
    assert!(dialog.contains("<div class=\"grid grid--2\" data-show-when=\"authType=header\">"));
    assert!(dialog.contains("<div class=\"field\" data-show-when=\"authType=bearer\" hidden>"));
}

#[tokio::test]
async fn asks_for_environment_values_again_once_the_package_changes() {
    let app = app_without_deno().await;
    let admin = create_admin(&app).await;
    let saved = environment_of(&app, &[("API_KEY", "saved-api-key")]);
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Package MCP".into();
        mcp.transport = McpTransport::Npm;
        mcp.npm_package = Some("@example/trusted-mcp".into());
        mcp.npm_version = Some("1.0.0".into());
        mcp.npm_env = saved.clone();
    })
    .await;

    let page = page_of(&app, &admin, &format!("/mcps/{}/edit", mcp.id)).await;
    let dialog = between(&page, "id=\"edit-mcp\"", "</dialog>");

    assert!(!page.contains("saved-api-key"));
    assert!(!page.contains(saved.as_deref().unwrap()));
    // A saved variable keeps its value when the field is left blank, while
    // the form runs the package it was entered for and the row keeps its name.
    let keeps = "npmPackage=@example/trusted-mcp&amp;npmEnv[0][name]=API_KEY";
    assert!(dialog.contains(&format!(
        "for=\"edit-mcp-env-0-value\">Value <span class=\"field__optional\" data-show-when=\"{keeps}\">Optional</span></label>"
    )));
    assert!(dialog.contains(&format!(
        "name=\"npmEnv[0][value]\" type=\"password\" autocomplete=\"off\" data-optional-when=\"{keeps}\" aria-describedby=\"edit-mcp-env-0-value-help\">"
    )));
    assert!(dialog.contains(
        "<p class=\"field__help\" id=\"edit-mcp-env-0-value-help\" data-show-when=\"npmEnv[0][name]=API_KEY\">\
         <span data-hide-when=\"npmPackage!=@example/trusted-mcp\">Leave blank to keep the saved value</span>\
         <span data-show-when=\"npmPackage!=@example/trusted-mcp\" hidden>Enter the value again for the new package</span></p>"
    ));

    // A form sent back for another package asks for the value again.
    let moved = from_script(app.put(&format!("/mcps/{}", mcp.id)))
        .login_as(&admin)
        .csrf()
        .form(&npm_form(&[
            ("name", "Package MCP"),
            ("npmPackage", "@example/other-mcp"),
            ("npmEnv[0][name]", "API_KEY"),
            ("npmEnv[0][value]", ""),
        ]))
        .send()
        .await;
    assert_eq!(moved.status, StatusCode::UNPROCESSABLE_ENTITY);
    let moved = moved.text();
    assert!(
        moved.contains(
            "Enter this value again: saved values are not passed on to a different package"
        )
    );
    assert!(moved.contains("<span data-hide-when=\"npmPackage!=@example/trusted-mcp\" hidden>Leave blank to keep the saved value</span>"));
    assert!(moved.contains("<span data-show-when=\"npmPackage!=@example/trusted-mcp\">Enter the value again for the new package</span>"));
    assert!(moved.contains(&format!(
        "name=\"npmEnv[0][value]\" type=\"password\" autocomplete=\"off\" required data-optional-when=\"{keeps}\""
    )));
    assert!(!moved.contains("saved-api-key"));
    assert_eq!(
        find_mcp(&app, mcp.id).await.npm_package.as_deref(),
        Some("@example/trusted-mcp")
    );
}

// --- the edit dialog of an npm MCP (tests/browser/mcp_edit_modal_ux.spec.ts) ---

#[tokio::test]
async fn shows_the_version_deno_has_cached_and_offers_the_update() {
    let directory = tempfile::tempdir().unwrap();
    let deno_dir = directory.path().join("deno-dir");
    let package = deno_dir.join("npm/registry.npmjs.org/@shopify/dev-mcp");
    std::fs::create_dir_all(package.join("1.14.4")).unwrap();
    std::fs::write(
        package.join("registry.json"),
        json!({ "dist-tags": { "latest": "1.14.4" } }).to_string(),
    )
    .unwrap();

    let host = HostEnvironment {
        deno_dir: Some(deno_dir.display().to_string()),
        ..HostEnvironment::from_process()
    };
    let app = TestApp::with_state(
        |_| {},
        |core| {
            let runtime = DenoRuntime::with_host(None, host);
            let upstream = Upstream::builder(core.clone(), builtins())
                .deno(DenoRunner::with_runtime(core.clone(), runtime))
                .build();
            AppState::with_upstream(core, upstream)
        },
    )
    .await;
    let admin = create_admin(&app).await;
    let latest = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Shopify Dev".into();
        mcp.transport = McpTransport::Npm;
        mcp.npm_package = Some("@shopify/dev-mcp".into());
        mcp.npm_version = Some("latest".into());
    })
    .await;
    let pinned = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Pinned".into();
        mcp.transport = McpTransport::Npm;
        mcp.npm_package = Some("@shopify/dev-mcp".into());
        mcp.npm_version = Some("1.0.0".into());
    })
    .await;

    let page = page_of(&app, &admin, &format!("/mcps/{}/edit", latest.id)).await;
    let rows = table_rows(&page);
    assert!(rows[1].contains("<span class=\"cell-code\">@shopify/dev-mcp</span><span class=\"cell-sub\">cached 1.14.4</span>"));
    // The pinned version is not the one in the cache.
    assert!(!rows[0].contains("cached"));

    let dialog = between(&page, "id=\"edit-mcp\"", "</dialog>");
    // Version and extra args sit side by side, the cached version under the first.
    assert!(dialog.contains("<div class=\"grid grid--2\"><div class=\"field\"><label class=\"field__label\" for=\"edit-mcp-npm-version\">"));
    assert!(dialog.contains("name=\"npmVersion\" type=\"text\" value=\"latest\" placeholder=\"latest\" autocomplete=\"off\" autocapitalize=\"off\" spellcheck=\"false\" aria-describedby=\"edit-mcp-npm-version-help\"><p class=\"field__help\" id=\"edit-mcp-npm-version-help\">Cached in Deno: 1.14.4</p>"));
    assert!(dialog.contains("form=\"edit-mcp-update\">"));
    assert!(dialog.contains("Update MCP"));
    assert!(dialog.contains(&format!(
        "<form id=\"edit-mcp-update\" method=\"post\" action=\"/mcps/{}/update\" data-async hidden>",
        latest.id
    )));

    // A pinned version is not updated: its dialog does not offer it.
    let page = page_of(&app, &admin, &format!("/mcps/{}/edit", pinned.id)).await;
    let dialog = between(&page, "id=\"edit-mcp\"", "</dialog>");
    assert!(!dialog.contains("Update MCP"));
    assert!(!dialog.contains("Cached in Deno"));
}

// --- OAuth through a pasted callback (tests/browser/mcp_oauth_pasted_callback.spec.ts) ---

#[tokio::test]
async fn takes_a_pasted_callback_for_providers_that_redirect_to_a_loopback_address() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let figma = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Figma".into();
        mcp.oauth_required = true;
        mcp.http_url = Some("https://mcp.figma.com/mcp".into());
        mcp.status = McpStatus::Draft;
    })
    .await;
    let notion = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Notion".into();
        mcp.oauth_required = true;
        mcp.http_url = Some("https://mcp.notion.com/mcp".into());
        mcp.status = McpStatus::Draft;
    })
    .await;

    let page = page_of(&app, &admin, &format!("/mcps/{}/edit", figma.id)).await;
    let dialog = between(&page, "id=\"edit-mcp\"", "</dialog>");
    assert!(dialog.contains("This MCP requires OAuth. Connect opens the provider in a new tab; paste the address it ends on below."));
    // Connect opens another tab, so that this page stays to receive the address.
    assert!(dialog.contains(&format!(
        "<a class=\"button button--secondary\" href=\"/mcps/{}/oauth/start\" target=\"_blank\" rel=\"noopener\">Connect</a>",
        figma.id
    )));
    assert!(dialog.contains("<div class=\"field\" data-oauth-paste>"));
    assert!(dialog.contains(
        "<label class=\"field__label\" for=\"edit-mcp-callback\">Callback address</label>"
    ));
    // The field has no name: it is not part of the form that saves the MCP.
    assert!(dialog.contains(
        "<input class=\"input grow\" id=\"edit-mcp-callback\" type=\"text\" inputmode=\"url\""
    ));
    assert!(dialog.contains("data-oauth-paste-submit>Finish connecting</button>"));
    assert!(dialog.contains(
        "data-oauth-paste-error hidden>Paste the full localhost address, including ?code=…</p>"
    ));
    // From the list, connecting goes through the dialog that takes the address.
    let row = table_rows(&page)
        .into_iter()
        .find(|row| row.contains("Figma"))
        .unwrap();
    assert!(row.contains(&format!(
        "<a class=\"menu__item\" role=\"menuitem\" href=\"/mcps/{}/edit\" data-dialog-open=\"#edit-mcp\" data-dialog-fetch data-dialog-history>",
        figma.id
    )));

    // No authorization was started in this session, so reaching the callback
    // with the pasted response is reported as an invalid callback.
    let callback = app
        .get("/mcps/oauth/callback?code=pasted-code&state=pasted-state")
        .login_as(&admin)
        .send()
        .await;
    assert_eq!(callback.status, StatusCode::FOUND);
    assert_eq!(callback.location(), Some("/mcps"));
    assert_eq!(
        callback.flashed("error"),
        Some(json!("Invalid OAuth callback"))
    );

    // Providers that accept the instance's callback keep the direct redirect.
    let page = page_of(&app, &admin, &format!("/mcps/{}/edit", notion.id)).await;
    let dialog = between(&page, "id=\"edit-mcp\"", "</dialog>");
    assert!(dialog.contains(&format!(
        "<a class=\"button button--secondary\" href=\"/mcps/{}/oauth/start\">Connect</a>",
        notion.id
    )));
    assert!(dialog.contains("This MCP requires OAuth. Connect your account to finish setup."));
    assert!(!dialog.contains("Callback address"));
    assert!(!dialog.contains("data-oauth-paste"));
}

#[tokio::test]
async fn authorizes_a_connected_loopback_provider_again_from_its_dialog() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let token = encrypt(&app, "figma-access-token");
    let figma = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Figma".into();
        mcp.http_url = Some("https://mcp.figma.com/mcp".into());
        mcp.oauth_access_token = token.clone();
    })
    .await;

    let page = page_of(&app, &admin, &format!("/mcps/{}/edit", figma.id)).await;
    let dialog = between(&page, "id=\"edit-mcp\"", "</dialog>");

    assert!(!page.contains("figma-access-token") && !page.contains(token.as_deref().unwrap()));
    // The link opens the provider in another tab and brings up what takes the address.
    assert!(dialog.contains(&format!(
        "<a class=\"button button--secondary\" href=\"/mcps/{}/oauth/start\" target=\"_blank\" rel=\"noopener\" data-reveal=\"#edit-mcp-authorize\">Re-authorize</a>",
        figma.id
    )));
    let waiting = between(
        dialog,
        "<div class=\"stack\" id=\"edit-mcp-authorize\" hidden>",
        "Test connection",
    );
    assert!(waiting.contains("Authorization required"));
    assert!(waiting.contains("data-oauth-paste"));
}

// --- without a public address (tests/*/public_url_config.spec.ts) ---

#[tokio::test]
async fn disables_oauth_actions_without_a_public_address() {
    let (fetcher, calls) = mock_fetch(|_| not_found());
    let app = TestApp::with_state(
        |config| config.app_url = None,
        |core| {
            let upstream = Upstream::builder(core.clone(), builtins())
                .fetcher(fetcher)
                .address_guard(public_names())
                .build();
            AppState::with_upstream(core, upstream)
        },
    )
    .await;
    let admin = create_admin(&app).await;
    let mcp = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "OAuth MCP".into();
        mcp.oauth_required = true;
        mcp.http_url = Some("https://mcp.example/mcp".into());
        mcp.status = McpStatus::Draft;
    })
    .await;

    let page = page_of(&app, &admin, &format!("/mcps/{}/edit", mcp.id)).await;
    assert!(page.contains("Set APP_URL to enable public links"));
    let dialog = between(&page, "id=\"edit-mcp\"", "</dialog>");
    assert!(dialog.contains("<a class=\"button button--secondary\" aria-disabled=\"true\" title=\"Set APP_URL to connect with OAuth\">Connect</a>"));
    assert!(table_rows(&page)[0].contains("<a class=\"menu__item\" role=\"menuitem\" aria-disabled=\"true\" title=\"Set APP_URL to connect with OAuth\">"));
    assert!(!page.contains("/oauth/start"));

    // A direct OAuth start is rejected before making an upstream request.
    let response = app
        .get(&format!("/mcps/{}/oauth/start", mcp.id))
        .login_as(&admin)
        .send()
        .await;
    assert_redirect(&response, "/mcps");
    assert_eq!(
        response.flashed("error"),
        Some(json!(
            "APP_URL is not configured. Set it to the public HTTPS origin and redeploy."
        ))
    );
    assert_eq!(response.flashed("editingMcpId"), Some(json!(mcp.id)));
    assert!(calls.is_empty());
    assert_eq!(find_mcp(&app, mcp.id).await.oauth_client_id, None);
}
