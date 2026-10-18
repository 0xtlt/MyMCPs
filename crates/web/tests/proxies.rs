//! The Proxies page: the Tailscale nodes a team runs for its HTTP MCPs.
//! The nodes are a fake runtime that reports the status a test gives it and
//! records what it was asked, so nothing here starts `tailscaled`.

mod mcps_support;

use http::StatusCode;
use mcps_support::{assert_redirect, between, from_script};
use mymcps_core::models::{MAX_PROXIES_PER_TEAM, Proxy, Team, TeamRole, User};
use mymcps_web::proxy_nodes::{Availability, NodeState};
use mymcps_web::testing::factories::{
    create_account, create_admin, create_mcp, create_member, create_owner, create_proxy,
    create_team, join_team,
};
use mymcps_web::testing::{FakeProxyNodes, NodeCall, TestApp, TestResponse};
use serde_json::json;

const KEY: &str = "tskey-auth-kAbC123CNTRL-0123456789abcdef";
const OTHER_KEY: &str = "tskey-auth-kZyX987CNTRL-fedcba9876543210";

async fn app_with_nodes() -> (TestApp, std::sync::Arc<FakeProxyNodes>) {
    let nodes = FakeProxyNodes::available();
    (TestApp::with_proxy_nodes(nodes.clone()).await, nodes)
}

fn flash(response: &TestResponse, key: &str) -> Option<String> {
    response
        .flashed(key)
        .and_then(|value| value.as_str().map(str::to_string))
}

async fn proxies(app: &TestApp) -> Vec<Proxy> {
    Proxy::all(&*app.core.db).await.unwrap()
}

async fn proxy(app: &TestApp, id: i64) -> Proxy {
    Proxy::find(&*app.core.db, id).await.unwrap().unwrap()
}

async fn page(app: &TestApp, user: &User) -> String {
    let response = app.get("/proxies").login_as(user).send().await;
    assert_eq!(response.status, StatusCode::OK);
    response.text()
}

fn create_form<'a>(fields: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut form = vec![
        ("name", "Home"),
        ("tailscaleHostname", "mymcps"),
        ("tailscaleAuthKey", KEY),
    ];
    for (name, value) in fields {
        form.retain(|(existing, _)| existing != name);
        form.push((name, value));
    }
    form
}

async fn create(app: &TestApp, user: &User, fields: &[(&str, &str)]) -> TestResponse {
    app.post("/proxies")
        .login_as(user)
        .csrf()
        .form(&create_form(fields))
        .send()
        .await
}

// --- who may reach it ---

#[tokio::test]
async fn is_for_the_admins_of_a_team() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;
    let member = create_member(&app).await;
    let saved = create_proxy(&app, admin.id, |_| {}).await;

    // The navigation offers it to an admin, after Approvals, and not to a member.
    let admin_home = app.get("/").login_as(&admin).send().await.text();
    assert!(admin_home.contains(
        "<a class=\"nav-item\" href=\"/proxies\"><svg class=\"icon\" viewBox=\"0 0 24 24\" aria-hidden=\"true\">"
    ));
    assert!(
        admin_home.find("href=\"/approvals\"").unwrap()
            < admin_home.find("href=\"/proxies\"").unwrap()
    );
    let member_home = app.get("/").login_as(&member).send().await.text();
    assert!(!member_home.contains("/proxies"));

    // A member is sent home on every route, and nothing changes.
    let id = saved.id;
    for (method, path) in [
        ("get", "/proxies".to_string()),
        ("post", "/proxies".to_string()),
        ("get", format!("/proxies/{id}/edit")),
        ("put", format!("/proxies/{id}")),
        ("delete", format!("/proxies/{id}")),
        ("post", format!("/proxies/{id}/reconnect")),
    ] {
        let request = match method {
            "get" => app.get(&path),
            "post" => app.post(&path).form(&create_form(&[])),
            "put" => app.put(&path).form(&create_form(&[])),
            _ => app.delete(&path),
        };
        let response = request.login_as(&member).csrf().send().await;
        assert_redirect(&response, "/");
        assert_eq!(
            flash(&response, "error").as_deref(),
            Some("Admin access required"),
            "{method} {path}"
        );
    }
    assert_eq!(proxies(&app).await.len(), 1);
    assert_eq!(proxy(&app, id).await.name, saved.name);
    assert!(nodes.calls().is_empty());

    // A visitor is sent to sign in.
    assert_redirect(&app.get("/proxies").send().await, "/login");
    assert_redirect(
        &app.post("/proxies")
            .csrf()
            .form(&create_form(&[]))
            .send()
            .await,
        "/login",
    );
}

#[tokio::test]
async fn answers_not_found_for_the_proxy_of_another_team() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;
    let outsider = create_account(&app, None).await;
    let team = create_team(&app, "Research").await;
    join_team(&app, team.id, &outsider, TeamRole::Owner).await;
    let foreign = create_proxy(&app, outsider.id, |proxy| {
        proxy.team_id = team.id;
        proxy.name = "Lab".into();
    })
    .await;

    // Not in the list of the Root Team.
    assert!(!page(&app, &admin).await.contains("Lab"));

    for (method, path) in [
        ("get", format!("/proxies/{}/edit", foreign.id)),
        ("put", format!("/proxies/{}", foreign.id)),
        ("delete", format!("/proxies/{}", foreign.id)),
        ("post", format!("/proxies/{}/reconnect", foreign.id)),
        ("get", "/proxies/999/edit".to_string()),
        ("get", "/proxies/abc/edit".to_string()),
    ] {
        let request = match method {
            "get" => app.get(&path),
            "put" => app.put(&path).form(&create_form(&[("name", "Taken")])),
            "delete" => app.delete(&path),
            _ => app.post(&path),
        };
        let response = request.login_as(&admin).csrf().send().await;
        assert_redirect(&response, "/proxies");
        assert_eq!(
            flash(&response, "error").as_deref(),
            Some("Proxy not found"),
            "{method} {path}"
        );
    }
    let kept = proxy(&app, foreign.id).await;
    assert_eq!((kept.name.as_str(), kept.team_id), ("Lab", team.id));
    assert!(nodes.calls().is_empty());
}

// --- the page ---

#[tokio::test]
async fn invites_to_add_the_first_proxy() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;

    let page = page(&app, &admin).await;

    assert!(page.contains("<title>Proxies · MyMCPs</title>"));
    assert!(page.contains("<h1 class=\"page-header__title\">Proxies</h1>"));
    assert!(page.contains(
        "Reach MCPs on a private network through a Tailscale node that MyMCPs runs. Choose a proxy in the Network field of an HTTP MCP."
    ));
    assert!(page.contains("No proxies yet"));
    assert!(page.contains("Add a Tailscale proxy to reach MCPs that only listen on your tailnet."));
    assert!(!page.contains("<table"));
    // The button opens the dialog, which is in the page.
    assert!(page.contains("data-dialog-open=\"#add-proxy\""));
    assert!(!page.contains("aria-disabled=\"true\" aria-describedby=\"add-proxy-reason\""));
    assert!(page.contains("<dialog class=\"dialog\" id=\"add-proxy\""));
    assert!(page.contains("action=\"/proxies\" data-async"));
    // Its node takes the default name.
    assert!(page.contains("name=\"tailscaleHostname\" type=\"text\" value=\"mymcps\""));
    assert!(page.contains("Its name in your tailnet. Lowercase letters, digits, and hyphens."));
    assert!(page.contains("href=\"https://login.tailscale.com/admin/settings/keys\" target=\"_blank\" rel=\"noopener noreferrer\""));
    assert!(page.contains("Without a key, a login link appears here after you save."));
    assert!(page.contains("name=\"tailscaleAuthKey\" type=\"password\""));
    // Nothing banners an available runtime.
    assert!(!page.contains("Tailscale proxies cannot start"));
    assert!(!page.contains("Tailscale proxies are off"));
    assert!(nodes.calls().is_empty());
}

#[tokio::test]
async fn shows_each_proxy_with_what_its_node_is_doing() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;
    let connected = create_proxy(&app, admin.id, |proxy| proxy.name = "A home".into()).await;
    let login = create_proxy(&app, admin.id, |proxy| proxy.name = "B office".into()).await;
    let approval = create_proxy(&app, admin.id, |proxy| proxy.name = "C lab".into()).await;
    let failed = create_proxy(&app, admin.id, |proxy| proxy.name = "D broken".into()).await;
    let stopped = create_proxy(&app, admin.id, |proxy| proxy.name = "E off".into()).await;
    // An MCP goes through the first one.
    create_mcp(&app, admin.id, |mcp| {
        mcp.name = "T3".into();
        mcp.proxy_id = Some(connected.id);
    })
    .await;
    nodes.set_status(
        connected.id,
        FakeProxyNodes::connected("example.ts.net", "mymcps.example.ts.net", "100.64.0.7"),
    );
    nodes.set_status(
        login.id,
        FakeProxyNodes::with_state(NodeState::NeedsLogin {
            auth_url: Some("https://login.tailscale.com/a/abc123".into()),
        }),
    );
    nodes.set_status(
        approval.id,
        FakeProxyNodes::with_state(NodeState::NeedsApproval),
    );
    nodes.set_status(
        failed.id,
        FakeProxyNodes::with_state(NodeState::Failed {
            error: "Tailscale refused the auth key: it has expired".into(),
        }),
    );

    let page = page(&app, &admin).await;
    let table = between(&page, "<tbody>", "</tbody>");
    let rows: Vec<&str> = table.split("<tr>").skip(1).collect();

    assert_eq!(rows.len(), 5);
    assert!(page.contains("5 of 5 proxies in this team"));
    // Connected: the tailnet, the name in it and its first address.
    assert!(rows[0].contains("<span class=\"status status--success\">Connected</span>"));
    assert!(rows[0].contains("<span class=\"cell-title\">A home</span>"));
    assert!(rows[0].contains("<span class=\"cell-sub\">Tailscale · example.ts.net</span>"));
    assert!(rows[0].contains("<span class=\"cell-code\">mymcps.example.ts.net</span>"));
    assert!(rows[0].contains("<span class=\"cell-sub cell-sub--code\">100.64.0.7</span>"));
    assert!(rows[0].contains("<td class=\"cell-secondary\">1 MCP</td>"));
    // A node with no address is a dash, and a proxy no MCP uses says so.
    assert!(rows[1].contains("<span class=\"status status--warning\">Needs login</span>"));
    assert!(rows[1].contains("<span class=\"cell-sub\">Tailscale</span>"));
    assert!(rows[1].contains("<td class=\"cell-tertiary\">—</td>"));
    assert!(rows[1].contains("<td class=\"cell-secondary\">None</td>"));
    assert!(rows[2].contains("<span class=\"status status--warning\">Waiting for approval</span>"));
    assert!(rows[3].contains("<span class=\"status status--critical\">Error</span>"));
    assert!(rows[4].contains("<span class=\"status\">Stopped</span>"));

    // The login link is in the banner and in the menu of the row, as an external link.
    assert!(page.contains("B office needs a Tailscale login"));
    assert_eq!(
        page.matches("href=\"https://login.tailscale.com/a/abc123\" target=\"_blank\" rel=\"noopener noreferrer\"")
            .count(),
        2
    );
    assert!(page.contains("Log in to Tailscale"));
    // The other attention banners.
    assert!(page.contains("C lab is waiting for approval"));
    assert!(page.contains("Approve the device under Machines in the Tailscale admin console."));
    assert!(page.contains("D broken could not connect"));
    assert!(page.contains("Tailscale refused the auth key: it has expired"));
    let banner = between(&page, "D broken could not connect", "</form>");
    assert!(banner.contains(&format!("action=\"/proxies/{}/reconnect\"", failed.id)));
    assert!(banner.contains("Reconnect"));
    // No banner for a node that is fine, starting or stopped.
    assert!(!page.contains("A home needs"));
    assert!(!page.contains("E off is"));

    // The row menu: edit, reconnect, then delete, behind a prompt.
    assert!(rows[0].contains(&format!(
        "href=\"/proxies/{0}/edit\" data-dialog-open=\"#edit-proxy\" data-dialog-fetch>Edit</a>",
        connected.id
    )));
    let menu = between(rows[0], "<div class=\"menu\"", "</div></td>");
    assert!(menu.contains("Reconnect"));
    assert!(!menu.contains("Log in to Tailscale"));
    assert!(menu.contains(&format!(
        "action=\"/proxies/{}?_method=DELETE\" data-confirm=\"Delete A home?\" data-confirm-label=\"Delete\" data-confirm-tone=\"critical\"",
        connected.id
    )));
    assert!(rows[1].contains("Log in to Tailscale"));
    let _ = stopped;
}

#[tokio::test]
async fn escapes_what_a_node_and_a_name_say() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;
    let saved = create_proxy(&app, admin.id, |proxy| proxy.name = "<b>Home</b>".into()).await;
    nodes.set_status(
        saved.id,
        FakeProxyNodes::with_state(NodeState::NeedsLogin {
            auth_url: Some("javascript:alert(1)".into()),
        }),
    );

    let page = page(&app, &admin).await;

    assert!(!page.contains("<b>Home</b>"));
    assert!(page.contains("&lt;b&gt;Home&lt;/b&gt;"));
    // An address that is not a web address is not offered as a link.
    assert!(!page.contains("javascript:alert(1)"));
    assert!(!page.contains("Log in to Tailscale"));
    // It reads as a login without a link, which the edit dialog answers.
    assert!(page.contains("&lt;b&gt;Home&lt;/b&gt; needs a Tailscale login"));
    assert!(page.contains("Tailscale has not given a login link yet."));
}

#[tokio::test]
async fn sends_a_login_without_a_link_to_the_edit_dialog() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;
    let waiting = create_proxy(&app, admin.id, |proxy| proxy.name = "Office".into()).await;
    let linked = create_proxy(&app, admin.id, |proxy| proxy.name = "Lab".into()).await;
    let mut status = FakeProxyNodes::with_state(NodeState::NeedsLogin { auth_url: None });
    status.health = vec![
        "Tailscale is <not> logged in".into(),
        "A second issue".into(),
    ];
    nodes.set_status(waiting.id, status);
    nodes.set_status(
        linked.id,
        FakeProxyNodes::with_state(NodeState::NeedsLogin {
            auth_url: Some("https://login.tailscale.com/a/abc123".into()),
        }),
    );

    let page = page(&app, &admin).await;

    let banner = between(&page, "Office needs a Tailscale login", "</a></div>");
    assert!(banner.contains(
        "Tailscale has not given a login link yet. Paste an auth key in the proxy to log it in, or wait for the link."
    ));
    // The first health issue is a second line, escaped; the others are not shown here.
    assert!(banner.contains("<p>Tailscale is &lt;not&gt; logged in</p>"));
    assert!(!banner.contains("A second issue"));
    assert!(banner.contains(&format!(
        "<a class=\"button button--secondary\" href=\"/proxies/{}/edit\" data-dialog-open=\"#edit-proxy\" data-dialog-fetch>Edit",
        waiting.id
    )));
    // The proxy with a link keeps its own banner and the link, not this text.
    let linked_banner = between(&page, "Lab needs a Tailscale login", "</a></div>");
    assert!(linked_banner.contains("Log in to Tailscale"));
    assert!(!linked_banner.contains("has not given a login link"));
    assert_eq!(page.matches("has not given a login link").count(), 1);
}

#[tokio::test]
async fn shows_a_login_without_a_link_and_without_health_issues_on_one_line() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;
    let saved = create_proxy(&app, admin.id, |proxy| proxy.name = "Office".into()).await;
    nodes.set_status(
        saved.id,
        FakeProxyNodes::with_state(NodeState::NeedsLogin { auth_url: None }),
    );

    let page = page(&app, &admin).await;

    let banner = between(&page, "Office needs a Tailscale login", "</a></div>");
    assert_eq!(banner.matches("<p>").count(), 1);
}

#[tokio::test]
async fn writes_the_subtitle_of_the_mobile_list_as_a_sentence() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;
    let unused = create_proxy(&app, admin.id, |proxy| proxy.name = "A unused".into()).await;
    let single = create_proxy(&app, admin.id, |proxy| proxy.name = "B single".into()).await;
    let double = create_proxy(&app, admin.id, |proxy| proxy.name = "C double".into()).await;
    for (name, proxy_id) in [("M1", single.id), ("M2", double.id), ("M3", double.id)] {
        create_mcp(&app, admin.id, |mcp| {
            mcp.name = name.into();
            mcp.proxy_id = Some(proxy_id);
        })
        .await;
    }
    nodes.set_status(
        single.id,
        FakeProxyNodes::connected("example.ts.net", "b.example.ts.net", "100.64.0.2"),
    );
    nodes.set_status(
        double.id,
        FakeProxyNodes::connected("example.ts.net", "c.example.ts.net", "100.64.0.3"),
    );
    let _ = unused;

    let page = page(&app, &admin).await;

    let list = between(
        &page,
        "aria-labelledby=\"proxies-list-title\"",
        "</section>",
    );
    // No tailnet yet, and no MCP.
    assert!(list.contains("<span class=\"list-row__subtitle\">Tailscale · not used</span>"));
    assert!(
        list.contains(
            "<span class=\"list-row__subtitle\">Tailscale · example.ts.net · 1 MCP</span>"
        )
    );
    assert!(
        list.contains(
            "<span class=\"list-row__subtitle\">Tailscale · example.ts.net · 2 MCPs</span>"
        )
    );
    assert!(!list.contains("None"));
    // The table keeps its own column.
    assert!(page.contains("<td class=\"cell-secondary\">None</td>"));
}

#[tokio::test]
async fn opens_the_edit_dialog_of_a_proxy() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;
    let saved = create_proxy(&app, admin.id, |proxy| {
        proxy.name = "Home".into();
        proxy.tailscale_hostname = "box".into();
        proxy.tailscale_auth_key = app.core.encrypt_secret(Some(KEY));
    })
    .await;
    let mut status =
        FakeProxyNodes::connected("example.ts.net", "box.example.ts.net", "100.64.0.9");
    status.health = vec!["Tailscale could not reach the DERP relay".into()];
    nodes.set_status(saved.id, status);

    // The page script asks for the content of the dialog alone.
    let path = format!("/proxies/{}/edit", saved.id);
    let fragment = from_script(app.get(&path))
        .header("x-fragment", "edit-proxy")
        .login_as(&admin)
        .send()
        .await;
    assert_eq!(fragment.status, StatusCode::OK);
    let fragment = fragment.text();
    assert!(fragment.starts_with("<div class=\"dialog__header\">"));
    assert!(
        fragment.contains("<h2 class=\"dialog__title\" id=\"edit-proxy-title\">Edit Home</h2>")
    );
    assert!(fragment.contains(&format!(
        "action=\"/proxies/{}?_method=PUT\" data-async",
        saved.id
    )));
    // What the node is doing now.
    assert!(fragment.contains("<span class=\"badge badge--success\">Connected</span>"));
    assert!(fragment.contains("example.ts.net"));
    assert!(fragment.contains("box.example.ts.net"));
    assert!(fragment.contains("100.64.0.9"));
    assert!(fragment.contains("Health issues"));
    assert!(fragment.contains("Tailscale could not reach the DERP relay"));
    // The fields: the key is never read back.
    assert!(fragment.contains("name=\"name\" type=\"text\" value=\"Home\""));
    assert!(fragment.contains("name=\"tailscaleHostname\" type=\"text\" value=\"box\""));
    assert!(fragment.contains("Auth key (leave blank to keep)"));
    assert!(fragment.contains("name=\"tailscaleAuthKey\" type=\"password\""));
    assert!(!fragment.contains(KEY));
    // Delete and Reconnect are forms of their own.
    assert!(fragment.contains(&format!("action=\"/proxies/{}?_method=DELETE\"", saved.id)));
    assert!(fragment.contains(&format!("action=\"/proxies/{}/reconnect\"", saved.id)));
    assert!(fragment.contains("Save changes"));

    // Without the script, the page with the dialog open.
    let full = app.get(&path).login_as(&admin).send().await.text();
    assert!(full.contains("<h1 class=\"page-header__title\">Proxies</h1>"));
    assert!(full.contains("id=\"edit-proxy\""));
    assert!(full.contains("data-dialog-return=\"/proxies\" data-open"));
    assert!(full.contains("Edit Home"));

    // The login link of a node that needs one is in the dialog too.
    nodes.set_status(
        saved.id,
        FakeProxyNodes::with_state(NodeState::NeedsLogin {
            auth_url: Some("https://login.tailscale.com/a/xyz".into()),
        }),
    );
    let fragment = from_script(app.get(&path))
        .header("x-fragment", "edit-proxy")
        .login_as(&admin)
        .send()
        .await
        .text();
    assert!(fragment.contains(
        "href=\"https://login.tailscale.com/a/xyz\" target=\"_blank\" rel=\"noopener noreferrer\""
    ));
    assert!(fragment.contains("<span class=\"badge badge--warning\">Needs login</span>"));
}

#[tokio::test]
async fn does_not_offer_to_keep_a_key_the_proxy_has_not() {
    let (app, _nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;
    let saved = create_proxy(&app, admin.id, |_| {}).await;

    let fragment = from_script(app.get(&format!("/proxies/{}/edit", saved.id)))
        .header("x-fragment", "edit-proxy")
        .login_as(&admin)
        .send()
        .await
        .text();

    assert!(!fragment.contains("leave blank to keep"));
    assert!(fragment.contains("Auth key <span class=\"field__optional\">Optional</span>"));
}

// --- adding a proxy ---

#[tokio::test]
async fn adds_a_proxy_and_starts_its_node() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;

    let response = create(&app, &admin, &[("name", "  Home ")]).await;

    assert_redirect(&response, "/proxies");
    assert_eq!(flash(&response, "success").as_deref(), Some("Proxy added"));
    let saved = proxies(&app).await;
    assert_eq!(saved.len(), 1);
    let proxy = &saved[0];
    assert_eq!(
        (proxy.name.as_str(), proxy.tailscale_hostname.as_str()),
        ("Home", "mymcps")
    );
    assert_eq!(proxy.team_id, Team::ROOT_ID);
    assert_eq!(proxy.created_by, Some(admin.id));
    // The key is saved encrypted, and the node gets it as it was typed.
    let stored = proxy.tailscale_auth_key.as_deref().unwrap();
    assert_ne!(stored, KEY);
    assert!(!stored.contains("tskey"));
    assert_eq!(app.core.decrypt_secret(Some(stored)).as_deref(), Some(KEY));
    assert_eq!(
        nodes.calls(),
        vec![NodeCall::Ensure(
            proxy.id,
            "mymcps".into(),
            Some(KEY.into())
        )]
    );
}

#[tokio::test]
async fn adds_a_proxy_for_the_page_script() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;

    let response = from_script(app.post("/proxies"))
        .login_as(&admin)
        .csrf()
        .form(&create_form(&[
            ("tailscaleHostname", "box-1"),
            ("tailscaleAuthKey", ""),
        ]))
        .send()
        .await;

    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert_eq!(response.header("x-location"), Some("/proxies"));
    assert_eq!(flash(&response, "success").as_deref(), Some("Proxy added"));
    let proxy = &proxies(&app).await[0];
    assert_eq!(proxy.tailscale_auth_key, None);
    // No key: the node asks Tailscale for a login link.
    assert_eq!(
        nodes.calls(),
        vec![NodeCall::Ensure(proxy.id, "box-1".into(), None)]
    );
}

#[tokio::test]
async fn keeps_the_proxy_when_its_node_does_not_start() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;
    nodes.fail_with(Some("the daemon exited"));

    let response = create(&app, &admin, &[]).await;

    assert_redirect(&response, "/proxies");
    assert_eq!(
        flash(&response, "error").as_deref(),
        Some(
            "Proxy added, but its Tailscale node did not start: tailscaled could not be started: the daemon exited"
        )
    );
    assert_eq!(flash(&response, "success"), None);
    assert_eq!(proxies(&app).await.len(), 1);
}

#[tokio::test]
async fn refuses_what_the_form_gets_wrong_and_draws_it_again() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;

    let hostname_hint = "Use lowercase letters, digits, and hyphens, starting and ending with a letter or a digit (63 characters at most)";
    let too_long = "x".repeat(101);
    for (field, value, message) in [
        ("name", "  ", "The name field must be defined"),
        (
            "name",
            too_long.as_str(),
            "The name field must not be greater than 100 characters",
        ),
        ("tailscaleHostname", "MyMCPs", hostname_hint),
        ("tailscaleHostname", "-box", hostname_hint),
        ("tailscaleHostname", "", "Enter a node name"),
        (
            "tailscaleAuthKey",
            "tskey-client-kXyZ-0123456789",
            "OAuth client secrets are not supported yet: create an auth key",
        ),
        (
            "tailscaleAuthKey",
            "hunter2",
            "Paste an auth key: it starts with tskey-auth-",
        ),
    ] {
        let fields = [(field, value)];

        // The page script gets the dialog back, with what was typed.
        let response = from_script(app.post("/proxies"))
            .login_as(&admin)
            .csrf()
            .form(&create_form(&fields))
            .send()
            .await;
        assert_eq!(
            response.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{message}"
        );
        let body = response.text();
        assert!(body.contains(message), "{message}");
        assert!(body.contains("aria-invalid=\"true\""), "{message}");
        assert!(body.contains("action=\"/proxies\" data-async"));

        // A plain post goes back with the first message.
        let response = create(&app, &admin, &fields).await;
        assert_redirect(&response, "/proxies");
        assert_eq!(flash(&response, "error").as_deref(), Some(message));
    }
    assert!(proxies(&app).await.is_empty());
    assert!(nodes.calls().is_empty());
}

#[tokio::test]
async fn gives_back_what_was_typed_when_the_form_is_refused() {
    let (app, _nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;

    let response = from_script(app.post("/proxies"))
        .login_as(&admin)
        .csrf()
        .form(&create_form(&[
            ("name", "My <home>"),
            ("tailscaleHostname", "Bad Name"),
        ]))
        .send()
        .await;

    let body = response.text();
    assert!(body.contains("value=\"My &lt;home&gt;\""));
    assert!(body.contains("value=\"Bad Name\""));
}

#[tokio::test]
async fn gives_a_team_at_most_five_proxies() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;
    for index in 0..MAX_PROXIES_PER_TEAM {
        create_proxy(&app, admin.id, |proxy| {
            proxy.name = format!("Proxy {index}")
        })
        .await;
    }

    // The button is there, greyed, with the reason.
    let shown = page(&app, &admin).await;
    assert!(shown.contains("aria-disabled=\"true\" aria-describedby=\"add-proxy-reason\""));
    assert!(shown.contains("id=\"add-proxy-reason\">A team has at most 5 proxies.</p>"));
    assert!(!shown.contains("data-dialog-open=\"#add-proxy\""));

    let response = create(&app, &admin, &[]).await;
    assert_redirect(&response, "/proxies");
    assert_eq!(
        flash(&response, "error").as_deref(),
        Some("A team has at most 5 proxies")
    );
    let dialog = from_script(app.post("/proxies"))
        .login_as(&admin)
        .csrf()
        .form(&create_form(&[]))
        .send()
        .await;
    assert_eq!(dialog.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(dialog.text().contains("A team has at most 5 proxies"));
    assert_eq!(proxies(&app).await.len(), MAX_PROXIES_PER_TEAM);
    assert!(nodes.calls().is_empty());

    // Another team has its own five.
    let outsider = create_account(&app, None).await;
    let team = create_team(&app, "Research").await;
    join_team(&app, team.id, &outsider, TeamRole::Owner).await;
    let response = app
        .post("/proxies")
        .login_as(&outsider)
        .in_team(team.id)
        .csrf()
        .form(&create_form(&[]))
        .send()
        .await;
    assert_eq!(flash(&response, "success").as_deref(), Some("Proxy added"));
}

#[tokio::test]
async fn gives_the_instance_as_many_proxies_as_it_is_set_to_run() {
    let nodes = FakeProxyNodes::available();
    let app = TestApp::with_proxy_nodes_and_config(nodes.clone(), |config| {
        config.tailscale_max_proxies = 2;
    })
    .await;
    let admin = create_admin(&app).await;
    // One of them is in another team: the limit is the instance's.
    let outsider = create_account(&app, None).await;
    let team = create_team(&app, "Research").await;
    join_team(&app, team.id, &outsider, TeamRole::Owner).await;
    create_proxy(&app, outsider.id, |proxy| proxy.team_id = team.id).await;

    let response = create(&app, &admin, &[("name", "Second")]).await;
    assert_eq!(flash(&response, "success").as_deref(), Some("Proxy added"));

    let shown = page(&app, &admin).await;
    assert!(shown.contains("aria-disabled=\"true\" aria-describedby=\"add-proxy-reason\""));
    assert!(shown.contains("This server runs at most 2 proxies: raise TAILSCALE_MAX_PROXIES."));
    let response = create(&app, &admin, &[("name", "Third")]).await;
    assert_redirect(&response, "/proxies");
    assert_eq!(
        flash(&response, "error").as_deref(),
        Some("This server runs at most 2 proxies: a super admin raises TAILSCALE_MAX_PROXIES")
    );
    assert_eq!(proxies(&app).await.len(), 2);
}

#[tokio::test]
async fn refuses_to_add_a_proxy_where_tailscale_cannot_run() {
    let nodes = FakeProxyNodes::with_availability(Availability::Unavailable {
        reason: "tailscaled was not found: install Tailscale or set TAILSCALED_BINARY".into(),
    });
    let app = TestApp::with_proxy_nodes(nodes.clone()).await;
    let admin = create_admin(&app).await;

    let shown = page(&app, &admin).await;
    assert!(shown.contains("Tailscale proxies cannot start"));
    assert!(shown.contains("tailscaled was not found: install Tailscale or set TAILSCALED_BINARY"));
    assert!(shown.contains("class=\"banner banner--warning\""));
    assert!(shown.contains("aria-disabled=\"true\" aria-describedby=\"add-proxy-reason\""));
    assert!(shown.contains("Tailscale proxies cannot start on this server."));
    assert!(!shown.contains("data-dialog-open=\"#add-proxy\""));
    let response = create(&app, &admin, &[]).await;
    assert_eq!(
        flash(&response, "error").as_deref(),
        Some(
            "Tailscale proxies cannot start: tailscaled was not found: install Tailscale or set TAILSCALED_BINARY"
        )
    );

    // Off: an informative banner, and the same refusal.
    nodes.set_availability(Availability::Disabled);
    let shown = page(&app, &admin).await;
    assert!(shown.contains("Tailscale proxies are off"));
    assert!(shown.contains("A super admin turns them on with TAILSCALE_MAX_PROXIES."));
    assert!(shown.contains("class=\"banner\""));
    assert!(shown.contains("Tailscale proxies are off on this server."));
    let response = create(&app, &admin, &[]).await;
    assert_eq!(
        flash(&response, "error").as_deref(),
        Some("Tailscale proxies are off: a super admin turns them on with TAILSCALE_MAX_PROXIES")
    );

    assert!(proxies(&app).await.is_empty());
    assert!(nodes.calls().is_empty());
}

// --- changing a proxy ---

#[tokio::test]
async fn changes_a_proxy_and_keeps_its_key_when_the_field_is_blank() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;
    let saved = create_proxy(&app, admin.id, |proxy| {
        proxy.tailscale_auth_key = app.core.encrypt_secret(Some(KEY));
    })
    .await;
    let stored_key = saved.tailscale_auth_key.clone();
    let path = format!("/proxies/{}?_method=PUT", saved.id);

    let response = app
        .post(&path)
        .login_as(&admin)
        .csrf()
        .form(&[
            ("name", " Office "),
            ("tailscaleHostname", "office"),
            ("tailscaleAuthKey", ""),
        ])
        .send()
        .await;

    assert_redirect(&response, "/proxies");
    assert_eq!(
        flash(&response, "success").as_deref(),
        Some("Proxy updated")
    );
    let updated = proxy(&app, saved.id).await;
    assert_eq!(
        (updated.name.as_str(), updated.tailscale_hostname.as_str()),
        ("Office", "office")
    );
    // The saved key is untouched, and the node still gets it.
    assert_eq!(updated.tailscale_auth_key, stored_key);
    assert_eq!(
        nodes.calls(),
        vec![NodeCall::Ensure(
            saved.id,
            "office".into(),
            Some(KEY.into())
        )]
    );

    // A key typed replaces it.
    nodes.clear_calls();
    let response = from_script(app.post(&path))
        .login_as(&admin)
        .csrf()
        .form(&[
            ("name", "Office"),
            ("tailscaleHostname", "office"),
            ("tailscaleAuthKey", OTHER_KEY),
        ])
        .send()
        .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert_eq!(response.header("x-location"), Some("/proxies"));
    let updated = proxy(&app, saved.id).await;
    assert_ne!(updated.tailscale_auth_key, stored_key);
    assert_eq!(
        app.core
            .decrypt_secret(updated.tailscale_auth_key.as_deref())
            .as_deref(),
        Some(OTHER_KEY)
    );
    assert_eq!(
        nodes.calls(),
        vec![NodeCall::Ensure(
            saved.id,
            "office".into(),
            Some(OTHER_KEY.into())
        )]
    );
}

#[tokio::test]
async fn refuses_a_change_and_draws_the_dialog_from_the_saved_proxy() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;
    let saved = create_proxy(&app, admin.id, |proxy| proxy.name = "Home".into()).await;
    let path = format!("/proxies/{}?_method=PUT", saved.id);

    let response = from_script(app.post(&path))
        .login_as(&admin)
        .csrf()
        .form(&[
            ("name", "Renamed"),
            ("tailscaleHostname", "NOT OK"),
            ("tailscaleAuthKey", ""),
        ])
        .send()
        .await;

    assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY);
    let body = response.text();
    assert!(body.contains("Use lowercase letters, digits, and hyphens"));
    // What was typed is kept in the form; the title is the saved name's.
    assert!(body.contains("value=\"Renamed\""));
    assert!(body.contains("Edit Home"));
    assert_eq!(proxy(&app, saved.id).await.name, "Home");
    assert!(nodes.calls().is_empty());

    let response = app
        .post(&path)
        .login_as(&admin)
        .csrf()
        .form(&[
            ("name", "Renamed"),
            ("tailscaleHostname", "mymcps"),
            ("tailscaleAuthKey", "tskey-client-secret"),
        ])
        .send()
        .await;
    assert_redirect(&response, "/proxies");
    assert_eq!(
        flash(&response, "error").as_deref(),
        Some("OAuth client secrets are not supported yet: create an auth key")
    );
}

#[tokio::test]
async fn saves_a_change_while_tailscale_cannot_run() {
    let nodes = FakeProxyNodes::with_availability(Availability::Disabled);
    let app = TestApp::with_proxy_nodes(nodes.clone()).await;
    let admin = create_admin(&app).await;
    let saved = create_proxy(&app, admin.id, |_| {}).await;

    let response = app
        .post(&format!("/proxies/{}?_method=PUT", saved.id))
        .login_as(&admin)
        .csrf()
        .form(&create_form(&[("name", "Renamed")]))
        .send()
        .await;

    assert_eq!(
        flash(&response, "success").as_deref(),
        Some("Proxy updated")
    );
    assert_eq!(proxy(&app, saved.id).await.name, "Renamed");
    assert!(nodes.calls().is_empty());
}

#[tokio::test]
async fn reconnects_a_node() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;
    let saved = create_proxy(&app, admin.id, |proxy| {
        proxy.name = "Home".into();
        proxy.tailscale_hostname = "box".into();
        proxy.tailscale_auth_key = app.core.encrypt_secret(Some(KEY));
    })
    .await;
    let path = format!("/proxies/{}/reconnect", saved.id);

    let response = app.post(&path).login_as(&admin).csrf().send().await;

    assert_redirect(&response, "/proxies");
    assert_eq!(
        flash(&response, "success").as_deref(),
        Some("Reconnecting Home")
    );
    assert_eq!(
        nodes.calls(),
        vec![NodeCall::Reconnect(
            saved.id,
            "box".into(),
            Some(KEY.into())
        )]
    );

    nodes.fail_with(Some("the daemon exited"));
    let response = app.post(&path).login_as(&admin).csrf().send().await;
    assert!(
        flash(&response, "error")
            .unwrap()
            .starts_with("Home did not restart: ")
    );

    nodes.set_availability(Availability::Disabled);
    nodes.clear_calls();
    let response = app.post(&path).login_as(&admin).csrf().send().await;
    assert_eq!(
        flash(&response, "error").as_deref(),
        Some("Tailscale proxies are off: a super admin turns them on with TAILSCALE_MAX_PROXIES")
    );
    assert!(nodes.calls().is_empty());
}

// --- deleting a proxy ---

#[tokio::test]
async fn deletes_a_proxy_and_removes_its_node() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;
    let saved = create_proxy(&app, admin.id, |_| {}).await;
    let kept = create_proxy(&app, admin.id, |_| {}).await;

    let response = app
        .post(&format!("/proxies/{}?_method=DELETE", saved.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;

    assert_redirect(&response, "/proxies");
    assert_eq!(
        flash(&response, "success").as_deref(),
        Some("Proxy deleted")
    );
    let left = proxies(&app).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, kept.id);
    assert_eq!(nodes.calls(), vec![NodeCall::Remove(saved.id)]);

    // Also for the page script, and when the node cannot be removed: the
    // row is gone, and the failure is for the log.
    nodes.fail_with(Some("the daemon exited"));
    let response = from_script(app.delete(&format!("/proxies/{}", kept.id)))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert_eq!(response.header("x-location"), Some("/proxies"));
    assert!(proxies(&app).await.is_empty());
}

#[tokio::test]
async fn refuses_to_delete_a_proxy_that_mcps_use() {
    let (app, nodes) = app_with_nodes().await;
    let admin = create_admin(&app).await;
    let saved = create_proxy(&app, admin.id, |_| {}).await;
    let first = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "T3 Code".into();
        mcp.proxy_id = Some(saved.id);
    })
    .await;
    create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Docs".into();
        mcp.proxy_id = Some(saved.id);
    })
    .await;
    let path = format!("/proxies/{}?_method=DELETE", saved.id);

    let response = app.post(&path).login_as(&admin).csrf().send().await;

    assert_redirect(&response, "/proxies");
    assert_eq!(
        flash(&response, "error").as_deref(),
        Some(
            "Proxy is used by 2 MCPs: Docs, T3 Code. Set their Network to Direct or another proxy first."
        )
    );
    assert_eq!(proxies(&app).await.len(), 1);
    assert!(nodes.calls().is_empty());

    // With one left, in the singular.
    sqlx::query("delete from `mcps` where `name` = 'Docs'")
        .execute(&*app.core.db)
        .await
        .unwrap();
    let response = app.post(&path).login_as(&admin).csrf().send().await;
    assert_eq!(
        flash(&response, "error").as_deref(),
        Some("Proxy is used by 1 MCP: T3 Code. Set its Network to Direct or another proxy first.")
    );

    // Once the MCP goes direct, the proxy goes.
    sqlx::query("update `mcps` set `proxy_id` = null where `id` = ?")
        .bind(first.id)
        .execute(&*app.core.db)
        .await
        .unwrap();
    let response = app.post(&path).login_as(&admin).csrf().send().await;
    assert_eq!(
        flash(&response, "success").as_deref(),
        Some("Proxy deleted")
    );
    assert!(proxies(&app).await.is_empty());
    assert_eq!(nodes.calls(), vec![NodeCall::Remove(saved.id)]);
}

// --- what deletes a team's proxies ---

#[tokio::test]
async fn removes_the_nodes_of_a_team_that_is_deleted() {
    let (app, nodes) = app_with_nodes().await;
    create_owner(&app).await;
    let owner = create_account(&app, None).await;
    let team = create_team(&app, "Research").await;
    join_team(&app, team.id, &owner, TeamRole::Owner).await;
    let first = create_proxy(&app, owner.id, |proxy| proxy.team_id = team.id).await;
    let second = create_proxy(&app, owner.id, |proxy| proxy.team_id = team.id).await;
    let elsewhere = create_proxy(&app, owner.id, |_| {}).await;

    let response = app
        .post("/team?_method=DELETE")
        .login_as(&owner)
        .in_team(team.id)
        .csrf()
        .send()
        .await;

    assert_redirect(&response, "/");
    assert_eq!(flash(&response, "success").as_deref(), Some("Team deleted"));
    let mut removed: Vec<NodeCall> = nodes.calls();
    removed.sort_by_key(|call| format!("{call:?}"));
    assert_eq!(
        removed,
        vec![NodeCall::Remove(first.id), NodeCall::Remove(second.id)]
    );
    let left = proxies(&app).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, elsewhere.id);
}

#[tokio::test]
async fn removes_the_nodes_of_the_teams_an_account_takes_with_it() {
    let (app, nodes) = app_with_nodes().await;
    let mut super_admin = create_owner(&app).await;
    mymcps_web::testing::factories::make_super_admin(&app, &mut super_admin).await;
    // An account alone in its team, which has two proxies...
    let outsider = create_account(&app, None).await;
    let alone = create_team(&app, "Alone").await;
    join_team(&app, alone.id, &outsider, TeamRole::Owner).await;
    let first = create_proxy(&app, outsider.id, |proxy| proxy.team_id = alone.id).await;
    let second = create_proxy(&app, outsider.id, |proxy| proxy.team_id = alone.id).await;
    // ...and in the Root Team, which has one that stays.
    join_team(&app, Team::ROOT_ID, &outsider, TeamRole::Member).await;
    let root = create_proxy(&app, super_admin.id, |_| {}).await;

    let response = app
        .post(&format!("/users/{}?_method=DELETE", outsider.id))
        .login_as(&super_admin)
        .csrf()
        .form(&[("currentPassword", mymcps_web::testing::factories::PASSWORD)])
        .send()
        .await;

    assert_redirect(&response, "/users");
    assert_eq!(flash(&response, "success").as_deref(), Some("User deleted"));
    let mut removed = nodes.calls();
    removed.sort_by_key(|call| format!("{call:?}"));
    assert_eq!(
        removed,
        vec![NodeCall::Remove(first.id), NodeCall::Remove(second.id)]
    );
    let left = proxies(&app).await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, root.id);
}

#[tokio::test]
async fn reconciles_the_nodes_with_the_rows_of_an_imported_backup() {
    let (app, nodes) = app_with_nodes().await;
    let owner = create_owner(&app).await;
    let first = create_proxy(&app, owner.id, |proxy| {
        proxy.tailscale_auth_key = app.core.encrypt_secret(Some(KEY));
    })
    .await;
    let second = create_proxy(&app, owner.id, |_| {}).await;

    app.state.database_replaced().await.unwrap();

    assert_eq!(
        nodes.calls(),
        vec![NodeCall::Reconcile(vec![first.id, second.id])]
    );
    let _ = json!({});
}
