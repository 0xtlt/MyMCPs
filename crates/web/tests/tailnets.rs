//! The Tailnets page and the Network field of the MCP form: listing, connecting
//! with either sign-in method (against the fake `tailscaled` and LocalAPI of
//! `mymcps-tailnet`), what is refused, signing in again, deleting, and what
//! deleting a team or a user does to the daemons of its tailnets.
#![cfg(unix)]

mod mcps_support;

use std::path::PathBuf;

use http::StatusCode;
use mcps_support::*;
use mymcps_core::config::TailnetControl;
use mymcps_core::models::{Mcp, McpAuthType, McpTransport, Tailnet, Team, TeamRole, User};
use mymcps_tailnet::test_support::{FakeLocalApi, fake_tailscaled, running_status, status_in};
use mymcps_tailnet::{Tailnets, TailnetsConfig};
use mymcps_web::testing::factories::{
    PASSWORD, create_account, create_admin, create_mcp, create_member, create_super_admin,
    create_team, join_team,
};
use mymcps_web::testing::{TestApp, TestResponse};
use serde_json::{Value, json};
use tempfile::TempDir;

const KEY: &str = "tskey-auth-kS3cr3tK3y-abcdef";
const LOGIN_URL: &str = "https://login.tailscale.com/a/abc123";

/// An app whose tailnets are run by a fake `tailscaled`: each tailnet that
/// a test connects needs a fake LocalAPI on its socket, which [`Lab::daemon`]
/// binds before the request that starts it.
struct Lab {
    app: TestApp,
    tailnets: Tailnets,
    dir: TempDir,
}

impl Lab {
    /// A server that allows any control server, as the tests of Headscale need.
    async fn new() -> Self {
        Self::with_control(TailnetControl::Any).await
    }

    async fn with_control(control: TailnetControl) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let binary = fake_tailscaled(&dir.path().join("bin"), 0);
        let tailnets = Tailnets::new(TailnetsConfig {
            binary,
            data_dir: dir.path().join("data"),
        });
        let (fetcher, _calls) = mock_fetch(|call| mcp_answer(call, &json!([])));
        let app = TestApp::with_state(|config| config.tailnet_control = control, {
            let tailnets = tailnets.clone();
            |core| {
                let upstream =
                    mymcps_upstream::Upstream::builder(core.clone(), mymcps_web::state::builtins())
                        .fetcher(fetcher)
                        .address_guard(public_names())
                        .tailnets(Some(tailnets))
                        .build();
                mymcps_web::state::AppState::with_upstream(core, upstream)
            }
        })
        .await;
        Self { app, tailnets, dir }
    }

    fn tailnet_dir(&self, id: i64) -> PathBuf {
        self.dir.path().join("data/tailnets").join(id.to_string())
    }

    /// A fake LocalAPI where the daemon of tailnet `id` will have its socket.
    fn daemon(&self, id: i64, status: Value) -> FakeLocalApi {
        let directory = self.tailnet_dir(id);
        std::fs::create_dir_all(&directory).unwrap();
        FakeLocalApi::bind(&directory.join("ts.sock"), status)
    }

    /// How many times the fake `tailscaled` was started.
    fn runs(&self) -> u32 {
        std::fs::read_to_string(self.dir.path().join("bin/runs"))
            .ok()
            .and_then(|runs| runs.trim().parse().ok())
            .unwrap_or(0)
    }

    /// A tailnet row on a Headscale, with the state of a node that joined it
    /// before this process started, and a daemon that would answer if one
    /// were started.
    async fn joined_row(
        &self,
        team_id: i64,
        created_by: i64,
        name: &str,
    ) -> (Tailnet, FakeLocalApi) {
        let tailnet = self.row(team_id, created_by, name).await;
        sqlx::query("update `tailnets` set `control_url` = ? where `id` = ?")
            .bind("https://hs.example.com/")
            .bind(tailnet.id)
            .execute(&*self.app.core.db)
            .await
            .unwrap();
        let api = self.daemon(tailnet.id, running_status());
        std::fs::write(self.tailnet_dir(tailnet.id).join("tailscaled.state"), "{}").unwrap();
        (tailnet, api)
    }

    /// A tailnet row, as if connected earlier.
    async fn row(&self, team_id: i64, created_by: i64, name: &str) -> Tailnet {
        let mut tailnet = Tailnet {
            team_id,
            name: name.into(),
            hostname: format!("mymcps-{}", name.to_lowercase()),
            created_by,
            ..Default::default()
        };
        tailnet.insert(&*self.app.core.db).await.unwrap();
        tailnet
    }
}

fn needs_login() -> Value {
    json!({ "Version": "1.104.2", "BackendState": "NeedsLogin", "AuthURL": LOGIN_URL })
}

/// The rows of the table, one piece of the page for each.
fn table_rows(page: &str) -> Vec<&str> {
    between(page, "<tbody>", "</tbody>")
        .split("<tr>")
        .skip(1)
        .collect()
}

fn flash(response: &TestResponse, key: &str) -> Option<String> {
    response
        .flashed(key)
        .and_then(|value| value.as_str().map(str::to_string))
}

async fn tailnets_of(app: &TestApp, team_id: i64) -> Vec<Tailnet> {
    Tailnet::of_team(&*app.core.db, team_id).await.unwrap()
}

async fn get(app: &TestApp, user: &User, path: &str) -> String {
    let response = app.get(path).login_as(user).send().await;
    assert_eq!(response.status, StatusCode::OK, "{path}");
    response.text()
}

fn connect<'a>(
    app: &'a TestApp,
    user: &User,
    form: &[(&str, &str)],
) -> mymcps_web::testing::TestRequest<'a> {
    app.post("/tailnets").login_as(user).csrf().form(form)
}

// --- who sees the page ---

#[tokio::test]
async fn is_for_the_admins_of_a_team() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let member = create_member(&lab.app).await;

    assert_redirect(&lab.app.get("/tailnets").send().await, "/login");
    assert_redirect(&lab.app.get("/tailnets/new").send().await, "/login");
    for path in ["/tailnets", "/tailnets/new"] {
        let refused = lab.app.get(path).login_as(&member).send().await;
        assert_redirect(&refused, "/");
    }
    let refused = connect(&lab.app, &member, &[("name", "pro"), ("method", "link")])
        .send()
        .await;
    assert_redirect(&refused, "/");
    let refused = lab
        .app
        .post("/tailnets/1?_method=DELETE")
        .login_as(&member)
        .csrf()
        .send()
        .await;
    assert_redirect(&refused, "/");
    assert!(tailnets_of(&lab.app, Team::ROOT_ID).await.is_empty());

    // The navigation offers it to those who may use it.
    let nav = r#"<a class="nav-item" href="/tailnets""#;
    assert!(get(&lab.app, &admin, "/mcps").await.contains(nav));
    assert!(!get(&lab.app, &member, "/mcps").await.contains(nav));
    let page = get(&lab.app, &admin, "/tailnets").await;
    assert!(page.contains(r#"aria-current="page"><svg"#));
}

// --- listing ---

#[tokio::test]
async fn invites_to_connect_a_first_tailnet() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;

    let page = get(&lab.app, &admin, "/tailnets").await;
    assert!(page.contains("<h1 class=\"page-header__title\">Tailnets</h1>"));
    assert!(page.contains("No tailnet yet"));
    assert!(page.contains(
        "Connect a Tailscale network, then choose it on an HTTP MCP to reach its devices."
    ));
    assert!(!page.contains("<table"));
    assert!(!page.contains("http-equiv=\"refresh\""));
    assert!(!page.contains("Tailscale is off"));
    // The button, and the dialog it opens.
    assert!(page.contains(r#"href="/tailnets/new" data-dialog-open="#));
    assert!(page.contains(r#"<dialog class="dialog" id="connect-tailnet""#));
    assert!(!page.contains("id=\"connect-tailnet\" aria-labelledby=\"connect-tailnet-title\" data-dialog-static data-dialog-reset data-dialog-return=\"/tailnets\" data-open"));

    // The address that opens the dialog has it open on load, and no refresh.
    let opened = get(&lab.app, &admin, "/tailnets/new").await;
    assert!(opened.contains("data-dialog-return=\"/tailnets\" data-open>"));
    assert!(opened.contains("name=\"method\" value=\"link\" checked"));
    assert!(opened.contains("name=\"authKey\" type=\"password\""));
    assert!(!opened.contains("value=\"tskey"));
}

#[tokio::test]
async fn warns_that_tailscale_is_off_and_connects_nothing() {
    let app = TestApp::with_upstream(|upstream| upstream.tailnets(None)).await;
    let admin = create_admin(&app).await;

    let page = get(&app, &admin, "/tailnets").await;
    assert!(page.contains("Tailscale is off on this server"));
    assert!(page.contains("TAILSCALED_BINARY"));
    assert!(page.contains("aria-disabled=\"true\""));
    assert!(!page.contains("<dialog class=\"dialog\" id=\"connect-tailnet\""));

    let refused = from_script(connect(
        &app,
        &admin,
        &[("name", "pro"), ("method", "link")],
    ))
    .send()
    .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(refused.text().contains("Tailscale is off on this server"));
    assert!(tailnets_of(&app, Team::ROOT_ID).await.is_empty());

    // A row that exists is still listed, and can be deleted.
    let mut tailnet = Tailnet {
        team_id: Team::ROOT_ID,
        name: "pro".into(),
        hostname: "mymcps-pro".into(),
        created_by: admin.id,
        ..Default::default()
    };
    tailnet.insert(&*app.core.db).await.unwrap();
    let page = get(&app, &admin, "/tailnets").await;
    assert!(page.contains("badge--neutral\">Unavailable"));
    let deleted = app
        .post(&format!("/tailnets/{}?_method=DELETE", tailnet.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_eq!(
        flash(&deleted, "success").as_deref(),
        Some("Tailnet deleted")
    );
    assert!(tailnets_of(&app, Team::ROOT_ID).await.is_empty());
}

#[tokio::test]
async fn lists_the_tailnets_of_the_team_with_where_each_stands() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let other_team = create_team(&lab.app, "Other").await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let perso = lab.row(Team::ROOT_ID, admin.id, "perso").await;
    let elsewhere = lab.row(other_team.id, admin.id, "elsewhere").await;
    let _pro = lab.daemon(pro.id, running_status());
    let _perso = lab.daemon(perso.id, needs_login());
    let _elsewhere = lab.daemon(elsewhere.id, running_status());
    for name in ["Grafana", "Home"] {
        create_mcp(&lab.app, admin.id, |mcp| {
            mcp.name = name.into();
            mcp.slug = name.to_lowercase();
            mcp.tailnet_id = Some(pro.id);
        })
        .await;
    }
    create_mcp(&lab.app, admin.id, |mcp| mcp.tailnet_id = Some(perso.id)).await;

    let page = get(&lab.app, &admin, "/tailnets").await;
    // Tailnets of another team are not shown.
    assert!(!page.contains("elsewhere"));
    let rows = table_rows(&page);
    assert_eq!(rows.len(), 2);
    // By name: perso, pro.
    let (perso_row, pro_row) = (rows[0], rows[1]);
    assert!(pro_row.contains("cell-title\">pro<"));
    assert!(pro_row.contains("badge badge--success\">Connected"));
    assert!(pro_row.contains("<td class=\"cell-code\">mymcps.tail1234.ts.net</td>"));
    assert!(pro_row.contains("title=\"Grafana, Home\">2 MCPs"));
    assert!(!pro_row.contains("Sign in"));
    assert!(pro_row.contains(&format!("action=\"/tailnets/{}?_method=DELETE\"", pro.id)));

    assert!(perso_row.contains("badge badge--warning\">Needs sign-in"));
    assert!(perso_row.contains("<td class=\"cell-tertiary\">—</td>"));
    assert!(perso_row.contains("1 MCP<"));
    assert!(perso_row.contains(&format!(
        "<a class=\"button button--secondary\" href=\"{LOGIN_URL}\" target=\"_blank\" rel=\"noopener noreferrer\">Sign in</a>"
    )));
    // A tailnet that waits for its sign-in asks for the page again.
    assert!(page.contains("<meta http-equiv=\"refresh\" content=\"5\">"));
}

#[tokio::test]
async fn refreshes_while_a_tailnet_starts_and_stops_when_all_are_settled() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let api = lab.daemon(pro.id, status_in("Starting"));

    let page = get(&lab.app, &admin, "/tailnets").await;
    assert!(page.contains("badge badge--neutral\">Starting"));
    assert!(page.contains("http-equiv=\"refresh\""));
    // Never with the dialog open: what is typed would be lost.
    assert!(
        !get(&lab.app, &admin, "/tailnets/new")
            .await
            .contains("http-equiv")
    );

    api.set_status(running_status());
    // The status is cached for a moment: wait for it to be read again.
    tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
    let page = get(&lab.app, &admin, "/tailnets").await;
    assert!(page.contains("badge badge--success\">Connected"));
    assert!(!page.contains("http-equiv"));

    api.set_status(json!({ "Version": "1.104.2", "BackendState": "NeedsMachineAuth" }));
    tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
    let page = get(&lab.app, &admin, "/tailnets").await;
    assert!(page.contains("Needs approval"));
    assert!(page.contains("http-equiv"));
}

#[tokio::test]
async fn connect_is_a_plain_link_on_a_page_that_refreshes() {
    // The script would open the dialog on a page that reloads itself every
    // few seconds, and the reload would empty it while it is being filled.
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let api = lab.daemon(pro.id, status_in("Starting"));

    let page = get(&lab.app, &admin, "/tailnets").await;
    assert!(page.contains("http-equiv=\"refresh\""));
    let buttons: Vec<&str> = page
        .split("<a ")
        .filter(|link| link.contains("Connect a tailnet"))
        .collect();
    assert!(!buttons.is_empty());
    for button in &buttons {
        let link = between(button, "", "</a>");
        assert!(link.contains("href=\"/tailnets/new\""), "{link}");
        assert!(!link.contains("data-dialog-open"), "{link}");
    }

    // The page it leads to has the dialog open, and no refresh.
    let new = get(&lab.app, &admin, "/tailnets/new").await;
    assert!(!new.contains("http-equiv"));
    assert!(between(&new, "id=\"connect-tailnet\"", ">").contains("data-open"));

    // Once everything is settled the page stays, and the script opens the dialog.
    api.set_status(running_status());
    tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
    let page = get(&lab.app, &admin, "/tailnets").await;
    assert!(!page.contains("http-equiv"));
    assert!(page.contains("href=\"/tailnets/new\" data-dialog-open=\"#connect-tailnet\""));
}

#[tokio::test]
async fn links_only_a_web_address_to_sign_in() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let _api = lab.daemon(
        pro.id,
        json!({ "BackendState": "NeedsLogin", "AuthURL": "javascript:alert(1)" }),
    );

    let page = get(&lab.app, &admin, "/tailnets").await;
    assert!(page.contains("Needs sign-in"));
    assert!(page.contains("Waiting for the sign-in link"));
    assert!(!page.contains("javascript:"));
}

// --- connecting ---

#[tokio::test]
async fn connects_a_tailnet_with_a_sign_in_link() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let api = lab.daemon(1, needs_login());

    let response = connect(
        &lab.app,
        &admin,
        &[("name", "  pro "), ("method", "link"), ("controlUrl", "")],
    )
    .send()
    .await;
    assert_redirect(&response, "/tailnets");
    assert_eq!(
        flash(&response, "success").as_deref(),
        Some("Tailnet added: use its Sign in link to finish")
    );

    let saved = tailnets_of(&lab.app, Team::ROOT_ID).await;
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].id, 1);
    assert_eq!(saved[0].name, "pro");
    assert_eq!(saved[0].hostname, "mymcps-pro");
    assert_eq!(saved[0].control_url, None);
    assert_eq!(saved[0].created_by, admin.id);

    // The node was asked to join as `mymcps-pro`, with no key, and to log in.
    let start = &api.requests_to("/localapi/v0/start")[0].json();
    assert_eq!(start["UpdatePrefs"]["Hostname"], "mymcps-pro");
    assert_eq!(start["UpdatePrefs"]["ControlURL"], "");
    assert!(start.get("AuthKey").is_none());
    assert_eq!(api.requests_to("/localapi/v0/login-interactive").len(), 1);

    // The page then offers the link.
    let page = lab
        .app
        .get("/tailnets")
        .login_as(&admin)
        .session(response.session())
        .send()
        .await
        .text();
    assert!(page.contains(&format!("href=\"{LOGIN_URL}\" target=\"_blank\"")));
}

#[tokio::test]
async fn connects_a_headscale_tailnet_with_an_auth_key_that_is_kept_nowhere() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    // A new node: tailscaled waits for a login, which the key answers.
    let api = lab.daemon(1, status_in("NeedsLogin"));

    let response = from_script(connect(
        &lab.app,
        &admin,
        &[
            ("name", "Home lab"),
            ("method", "key"),
            ("authKey", KEY),
            ("controlUrl", "https://headscale.example.com"),
        ],
    ))
    .send()
    .await;
    // The script is sent to the list, with the confirmation flashed.
    assert_eq!(response.status, StatusCode::NO_CONTENT);
    assert_eq!(response.header("x-location"), Some("/tailnets"));
    assert_eq!(
        flash(&response, "success").as_deref(),
        Some("Tailnet connected")
    );
    assert!(!format!("{:?}", response.session()).contains(KEY));

    let saved = &tailnets_of(&lab.app, Team::ROOT_ID).await[0];
    assert_eq!(saved.name, "Home lab");
    assert_eq!(saved.hostname, "mymcps-home-lab");
    assert_eq!(
        saved.control_url.as_deref(),
        Some("https://headscale.example.com/")
    );

    // The key went to the daemon, once, and the login that uses it was started.
    let start = &api.requests_to("/localapi/v0/start")[0].json();
    assert_eq!(start["AuthKey"], KEY);
    assert_eq!(start["UpdatePrefs"]["Hostname"], "mymcps-home-lab");
    assert_eq!(
        start["UpdatePrefs"]["ControlURL"],
        "https://headscale.example.com/"
    );
    assert_eq!(api.requests_to("/localapi/v0/login-interactive").len(), 1);

    // Nothing the server shows or stores holds it.
    let page = get(&lab.app, &admin, "/tailnets").await;
    assert!(page.contains("Headscale · headscale.example.com"));
    assert!(!page.contains(KEY));
    let dump: Vec<String> = sqlx::query_scalar(
        "select `name` || `hostname` || ifnull(`control_url`, '') from `tailnets`",
    )
    .fetch_all(&*lab.app.core.db)
    .await
    .unwrap();
    assert!(dump.iter().all(|row| !row.contains(KEY)));
}

#[tokio::test]
async fn gives_a_refused_form_back_with_what_is_wrong_and_never_the_key() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;

    let refuse = async |form: &[(&str, &str)]| {
        let scripted = from_script(connect(&lab.app, &admin, form)).send().await;
        assert_eq!(
            scripted.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{form:?}"
        );
        let text = scripted.text();
        // The dialog comes back with what was typed, and without the key.
        assert!(!text.contains(KEY), "{text}");
        text
    };

    let text = refuse(&[("name", ""), ("method", "key"), ("authKey", KEY)]).await;
    assert!(text.contains("aria-invalid=\"true\""));
    assert!(text.contains("field__error"));
    assert!(text.contains("name=\"method\" value=\"key\" checked"));

    let text = refuse(&[("name", "-pro"), ("method", "link")]).await;
    assert!(text.contains("id=\"connect-tailnet-name-error\""));
    assert!(text.contains("value=\"-pro\""));

    let text = refuse(&[("name", "pro"), ("method", "key")]).await;
    assert!(text.contains("id=\"connect-tailnet-key-error\""));

    // What no field says is said above the form.
    let text = refuse(&[("name", "pro"), ("method", "carrier-pigeon")]).await;
    assert!(text.contains("This tailnet could not be connected"));

    let text = refuse(&[
        ("name", "pro"),
        ("method", "key"),
        ("authKey", KEY),
        ("controlUrl", "ftp://headscale.example.com"),
    ])
    .await;
    assert!(text.contains("Control server URL must use HTTP or HTTPS"));
    assert!(text.contains("value=\"ftp://headscale.example.com\""));

    let text = refuse(&[
        ("name", "pro"),
        ("method", "key"),
        ("authKey", KEY),
        ("controlUrl", "https://user:pw@headscale.example.com"),
    ])
    .await;
    assert!(text.contains("must not include a user name or a password"));
    assert!(!text.contains("pw@"));

    // A plain post goes back to the list with the first error, the key is
    // not kept in the session.
    let plain = connect(
        &lab.app,
        &admin,
        &[
            ("name", "pro"),
            ("method", "key"),
            ("authKey", &"k".repeat(2000)),
        ],
    )
    .send()
    .await;
    assert_redirect(&plain, "/tailnets");
    assert!(flash(&plain, "error").is_some());
    assert!(!format!("{:?}", plain.session()).contains("kkkkkkkk"));

    // Without a token, nothing happens.
    let forged = lab
        .app
        .post("/tailnets")
        .login_as(&admin)
        .form(&[("name", "pro"), ("method", "link")])
        .send()
        .await;
    assert_ne!(forged.status, StatusCode::NO_CONTENT);
    assert!(tailnets_of(&lab.app, Team::ROOT_ID).await.is_empty());
}

#[tokio::test]
async fn refuses_a_name_the_team_already_uses_whatever_its_case() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let other_team = create_team(&lab.app, "Other").await;
    lab.row(Team::ROOT_ID, admin.id, "Pro").await;
    // Another team's tailnet of that name is no obstacle.
    lab.row(other_team.id, admin.id, "work").await;

    let duplicate = from_script(connect(
        &lab.app,
        &admin,
        &[("name", "pro"), ("method", "link")],
    ))
    .send()
    .await;
    assert_eq!(duplicate.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        duplicate
            .text()
            .contains("This team already has a tailnet with this name")
    );
    assert_eq!(tailnets_of(&lab.app, Team::ROOT_ID).await.len(), 1);

    let api = lab.daemon(3, needs_login());
    let allowed = connect(&lab.app, &admin, &[("name", "work"), ("method", "link")])
        .send()
        .await;
    assert_redirect(&allowed, "/tailnets");
    drop(api);
    assert_eq!(tailnets_of(&lab.app, Team::ROOT_ID).await.len(), 2);
}

#[tokio::test]
async fn lets_one_of_two_creates_whose_names_differ_by_case_win() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    // Both may pass the check of the names and meet at the unique index:
    // the one that loses gets the usual field error, never a failure.
    let _first = lab.daemon(1, needs_login());
    let _second = lab.daemon(2, needs_login());
    let sent = |name: &'static str| {
        from_script(connect(
            &lab.app,
            &admin,
            &[("name", name), ("method", "link")],
        ))
        .send()
    };
    let (lower, upper) = tokio::join!(sent("pro"), sent("PRO"));

    let (won, lost) = if lower.status == StatusCode::UNPROCESSABLE_ENTITY {
        (&upper, &lower)
    } else {
        (&lower, &upper)
    };
    assert_ne!(
        won.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        won.text()
    );
    assert_eq!(lost.status, StatusCode::UNPROCESSABLE_ENTITY);
    let text = lost.text();
    assert!(
        text.contains("This team already has a tailnet with this name"),
        "{text}"
    );
    assert_eq!(tailnets_of(&lab.app, Team::ROOT_ID).await.len(), 1);
}

#[tokio::test]
async fn connects_at_most_five_tailnets_to_a_team() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let other_team = create_team(&lab.app, "Other").await;
    join_team(&lab.app, other_team.id, &admin, TeamRole::Owner).await;
    for name in ["a", "b", "c", "d", "e"] {
        lab.row(Team::ROOT_ID, admin.id, name).await;
    }

    let page = get(&lab.app, &admin, "/tailnets").await;
    assert!(page.contains("title=\"A team can connect up to 5 tailnets\""));
    assert!(!page.contains("id=\"connect-tailnet\""));

    let refused = from_script(connect(
        &lab.app,
        &admin,
        &[("name", "f"), ("method", "link")],
    ))
    .send()
    .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        refused
            .text()
            .contains("A team can connect up to 5 tailnets")
    );
    assert_eq!(tailnets_of(&lab.app, Team::ROOT_ID).await.len(), 5);

    // The limit is the team's: another team of the same person connects its own.
    let _api = lab.daemon(6, needs_login());
    let allowed = lab
        .app
        .post("/tailnets")
        .login_as(&admin)
        .in_team(other_team.id)
        .csrf()
        .form(&[("name", "f"), ("method", "link")])
        .send()
        .await;
    assert_redirect(&allowed, "/tailnets");
    assert_eq!(tailnets_of(&lab.app, other_team.id).await.len(), 1);
}

#[tokio::test]
async fn takes_the_tailnet_back_when_the_daemon_refuses_to_join() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let api = lab.daemon(1, status_in("NoState"));
    api.force("/localapi/v0/start", 500, &format!("invalid key {KEY}"));

    let refused = from_script(connect(
        &lab.app,
        &admin,
        &[("name", "pro"), ("method", "key"), ("authKey", KEY)],
    ))
    .send()
    .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    let text = refused.text();
    assert!(text.contains("Could not connect"), "{text}");
    assert!(!text.contains(KEY), "{text}");
    // No row is left, and nothing of the daemon.
    assert!(tailnets_of(&lab.app, Team::ROOT_ID).await.is_empty());
    assert!(!lab.tailnet_dir(1).exists());
}

#[tokio::test]
async fn does_not_start_a_tailnet_that_a_deletion_took_while_it_was_created() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    // The deletion of the team that holds the new row landed between its
    // insert and the start of its daemon: its id is tombstoned. The next
    // id is the one the form is about to get; ids are never reused.
    lab.tailnets.remove(1).await.unwrap();
    // A daemon that would answer, were one started.
    let api = lab.daemon(1, needs_login());

    let refused = from_script(connect(
        &lab.app,
        &admin,
        &[("name", "pro"), ("method", "link")],
    ))
    .send()
    .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        refused.text().contains("Could not connect"),
        "{}",
        refused.text()
    );
    // No orphan: no row, no daemon, no directory.
    assert!(tailnets_of(&lab.app, Team::ROOT_ID).await.is_empty());
    assert!(lab.tailnets.handle(1).is_none());
    assert!(api.requests().is_empty(), "nothing was asked of a daemon");
    assert!(!lab.tailnet_dir(1).exists());
}

#[tokio::test]
async fn a_tailnet_created_next_to_a_team_deleted_leaves_no_daemon_of_that_team() {
    let lab = Lab::new().await;
    let root_owner = create_super_admin(&lab.app).await;
    let team = create_team(&lab.app, "Research").await;
    let owner = create_account(&lab.app, None).await;
    join_team(&lab.app, team.id, &owner, TeamRole::Owner).await;
    let theirs = lab.row(team.id, owner.id, "theirs").await;
    let _theirs_api = lab.daemon(theirs.id, running_status());
    lab.tailnets.ensure_running(theirs.id).await.unwrap();
    let _new_api = lab.daemon(theirs.id + 1, needs_login());

    let deletion = lab
        .app
        .post("/team?_method=DELETE")
        .login_as(&owner)
        .in_team(team.id)
        .csrf()
        .send();
    let creation = connect(
        &lab.app,
        &root_owner,
        &[("name", "pro"), ("method", "link")],
    )
    .send();
    let (deleted, created) = tokio::join!(deletion, creation);
    assert_redirect(&deleted, "/");
    assert_redirect(&created, "/tailnets");

    // The tailnet of the deleted team is gone for good, the new one of the
    // other team is alive.
    assert!(lab.tailnets.handle(theirs.id).is_none());
    assert!(!lab.tailnet_dir(theirs.id).exists());
    assert!(matches!(
        lab.tailnets.ensure_running(theirs.id).await,
        Err(mymcps_tailnet::Error::Removed(_))
    ));
    assert_eq!(tailnets_of(&lab.app, Team::ROOT_ID).await.len(), 1);
    assert!(lab.tailnets.handle(theirs.id + 1).is_some());
}

#[tokio::test]
async fn forgets_the_removed_ids_when_a_backup_replaces_the_database() {
    let lab = Lab::new().await;
    let _api = lab.daemon(1, running_status());
    lab.tailnets.remove(1).await.unwrap();
    assert!(matches!(
        lab.tailnets.ensure_running(1).await,
        Err(mymcps_tailnet::Error::Removed(1))
    ));

    // The backup brings back a tailnet with an id this process removed.
    lab.app.state.database_replaced().await.unwrap();
    let _api = lab.daemon(1, running_status());
    lab.tailnets.ensure_running(1).await.unwrap();
    assert!(lab.tailnets.handle(1).is_some());
}

// --- which control servers the operator allows ---

async fn dialog_of(lab: &Lab, admin: &User) -> String {
    get(&lab.app, admin, "/tailnets/new").await
}

#[tokio::test]
async fn offers_the_control_server_only_when_the_operator_allows_other_servers() {
    // Unset: Tailscale's own, and no field at all.
    let official = Lab::with_control(TailnetControl::Official).await;
    let admin = create_admin(&official.app).await;
    let page = dialog_of(&official, &admin).await;
    assert!(page.contains("id=\"connect-tailnet-name\""));
    assert!(!page.contains("name=\"controlUrl\""), "{page}");
    assert!(!page.contains("Control server"));

    // `any`: a field to type the address in.
    let any = Lab::with_control(TailnetControl::Any).await;
    let admin = create_admin(&any.app).await;
    let page = dialog_of(&any, &admin).await;
    assert!(
        page.contains("<input class=\"input\" id=\"connect-tailnet-control\" name=\"controlUrl\"")
    );

    // A list: a select, Tailscale first, with the origins and nothing else.
    let listed = Lab::with_control(
        TailnetControl::parse(Some("https://hs.example.com,http://10.0.0.2:8080")).unwrap(),
    )
    .await;
    let admin = create_admin(&listed.app).await;
    let page = dialog_of(&listed, &admin).await;
    let select = between(
        &page,
        "<select class=\"input\" id=\"connect-tailnet-control\"",
        "</select>",
    );
    assert!(select.contains("name=\"controlUrl\""));
    assert!(
        select.contains("<option value=\"\" selected>Tailscale</option>"),
        "{select}"
    );
    assert!(
        select.contains("<option value=\"https://hs.example.com\">https://hs.example.com</option>")
    );
    assert!(
        select.contains("<option value=\"http://10.0.0.2:8080\">http://10.0.0.2:8080</option>")
    );
    assert_eq!(select.matches("<option").count(), 3);
}

#[tokio::test]
async fn refuses_a_control_server_the_operator_does_not_allow() {
    let lab =
        Lab::with_control(TailnetControl::parse(Some("https://hs.example.com")).unwrap()).await;
    let admin = create_admin(&lab.app).await;

    for url in [
        "https://evil.example.com",
        "http://hs.example.com",
        "http://127.0.0.1:8080",
        "http://169.254.169.254",
    ] {
        let refused = from_script(connect(
            &lab.app,
            &admin,
            &[("name", "pro"), ("method", "link"), ("controlUrl", url)],
        ))
        .send()
        .await;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{url}");
        let text = refused.text();
        assert!(text.contains("TAILNET_CONTROL_URLS"), "{url}: {text}");
        assert!(
            text.contains("id=\"connect-tailnet-control-error\""),
            "{url}: {text}"
        );
    }
    // Nothing was saved, and no daemon was started.
    assert!(tailnets_of(&lab.app, Team::ROOT_ID).await.is_empty());
    assert!(lab.tailnets.handle(1).is_none());

    // The allowed one connects, and Tailscale's own needs no permission.
    let api = lab.daemon(1, needs_login());
    let allowed = connect(
        &lab.app,
        &admin,
        &[
            ("name", "hs"),
            ("method", "link"),
            ("controlUrl", "https://hs.example.com/"),
        ],
    )
    .send()
    .await;
    assert_redirect(&allowed, "/tailnets");
    assert_eq!(
        api.requests_to("/localapi/v0/start")[0].json()["UpdatePrefs"]["ControlURL"],
        "https://hs.example.com/"
    );
    let _api = lab.daemon(2, needs_login());
    let official = connect(&lab.app, &admin, &[("name", "ts"), ("method", "link")])
        .send()
        .await;
    assert_redirect(&official, "/tailnets");
    assert_eq!(tailnets_of(&lab.app, Team::ROOT_ID).await.len(), 2);

    // Only Tailscale's own: the field is not there, and a post that names one is refused.
    let strict = Lab::with_control(TailnetControl::Official).await;
    let admin = create_admin(&strict.app).await;
    let refused = from_script(connect(
        &strict.app,
        &admin,
        &[
            ("name", "pro"),
            ("method", "link"),
            ("controlUrl", "https://hs.example.com"),
        ],
    ))
    .send()
    .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    let text = refused.text();
    assert!(
        text.contains("This tailnet could not be connected"),
        "{text}"
    );
    assert!(text.contains("TAILNET_CONTROL_URLS"), "{text}");
    assert!(tailnets_of(&strict.app, Team::ROOT_ID).await.is_empty());
    let plain = connect(
        &strict.app,
        &admin,
        &[
            ("name", "pro"),
            ("method", "link"),
            ("controlUrl", "https://hs.example.com"),
        ],
    )
    .send()
    .await;
    assert_redirect(&plain, "/tailnets");
    assert!(
        flash(&plain, "error")
            .unwrap()
            .contains("TAILNET_CONTROL_URLS")
    );
}

#[tokio::test]
async fn does_not_start_nor_sign_in_a_tailnet_whose_control_server_is_no_longer_allowed() {
    // Connected while the operator allowed it; the operator no longer does.
    let lab = Lab::with_control(TailnetControl::Official).await;
    let admin = create_admin(&lab.app).await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    sqlx::query("update `tailnets` set `control_url` = ? where `id` = ?")
        .bind("https://hs.example.com/")
        .bind(pro.id)
        .execute(&*lab.app.core.db)
        .await
        .unwrap();
    let api = lab.daemon(pro.id, needs_login());
    // Tailscale's own control server keeps working.
    let own = lab.row(Team::ROOT_ID, admin.id, "own").await;
    let _own_api = lab.daemon(own.id, running_status());

    // The page says why, and starts no daemon for it.
    let page = get(&lab.app, &admin, "/tailnets").await;
    let row = table_rows(&page)
        .into_iter()
        .find(|row| row.contains(">pro<"))
        .unwrap();
    assert!(row.contains("badge badge--critical"), "{row}");
    assert!(
        row.contains("https://hs.example.com is not allowed"),
        "{row}"
    );
    assert!(row.contains("TAILNET_CONTROL_URLS"), "{row}");
    assert!(lab.tailnets.handle(pro.id).is_none());
    assert!(lab.tailnets.handle(own.id).is_some());

    // The way back is refused with the same words, before any daemon is asked.
    let refused = sign_in_again(&lab.app, &admin, pro.id).send().await;
    assert_redirect(&refused, "/tailnets");
    let message = flash(&refused, "error").unwrap();
    assert!(message.starts_with("Could not sign in again:"), "{message}");
    assert!(message.contains("TAILNET_CONTROL_URLS"), "{message}");
    assert!(api.requests().is_empty());
    assert!(lab.tailnets.handle(pro.id).is_none());

    // So does the start of every tailnet when the server boots.
    let starts = lab.app.state.upstream.start_tailnets().await.unwrap();
    let blocked = starts
        .iter()
        .find(|start| start.tailnet.id == pro.id)
        .unwrap();
    assert!(matches!(
        &blocked.state,
        Ok(mymcps_tailnet::TailnetState::Failed(message)) if message.contains("TAILNET_CONTROL_URLS")
    ));
    assert!(lab.tailnets.handle(pro.id).is_none());

    // Allowed again (the operator restarts the server with the variable),
    // it starts as before.
    let allowed =
        Lab::with_control(TailnetControl::parse(Some("https://hs.example.com")).unwrap()).await;
    let admin = create_admin(&allowed.app).await;
    let hs = allowed.row(Team::ROOT_ID, admin.id, "hs").await;
    sqlx::query("update `tailnets` set `control_url` = ? where `id` = ?")
        .bind("https://hs.example.com/")
        .bind(hs.id)
        .execute(&*allowed.app.core.db)
        .await
        .unwrap();
    let _api = allowed.daemon(hs.id, needs_login());
    get(&allowed.app, &admin, "/tailnets").await;
    assert!(allowed.tailnets.handle(hs.id).is_some());
}

// --- deleting ---

#[tokio::test]
async fn refuses_to_delete_a_tailnet_that_mcps_use_and_names_them() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let api = lab.daemon(pro.id, running_status());
    let grafana = create_mcp(&lab.app, admin.id, |mcp| {
        mcp.name = "Grafana".into();
        mcp.slug = "grafana".into();
        mcp.tailnet_id = Some(pro.id);
    })
    .await;
    create_mcp(&lab.app, admin.id, |mcp| {
        mcp.name = "Home".into();
        mcp.slug = "home".into();
        mcp.tailnet_id = Some(pro.id);
    })
    .await;
    let delete = async || {
        lab.app
            .post(&format!("/tailnets/{}?_method=DELETE", pro.id))
            .login_as(&admin)
            .csrf()
            .send()
            .await
    };

    let refused = delete().await;
    assert_redirect(&refused, "/tailnets");
    assert_eq!(
        flash(&refused, "error").as_deref(),
        Some("pro is used by Grafana and Home. Move them to another network first.")
    );
    assert_eq!(tailnets_of(&lab.app, Team::ROOT_ID).await.len(), 1);
    assert!(api.requests_to("/localapi/v0/logout").is_empty());

    // Once they use another network, the tailnet goes: the node leaves it,
    // its daemon stops, its directory is gone.
    lab.tailnets.ensure_running(pro.id).await.unwrap();
    sqlx::query("update `mcps` set `tailnet_id` = null where `id` = ?")
        .bind(grafana.id)
        .execute(&*lab.app.core.db)
        .await
        .unwrap();
    let still = delete().await;
    assert_eq!(
        flash(&still, "error").as_deref(),
        Some("pro is used by Home. Move it to another network first.")
    );
    sqlx::query("update `mcps` set `tailnet_id` = null")
        .execute(&*lab.app.core.db)
        .await
        .unwrap();

    let deleted = delete().await;
    assert_redirect(&deleted, "/tailnets");
    assert_eq!(
        flash(&deleted, "success").as_deref(),
        Some("Tailnet deleted")
    );
    assert!(tailnets_of(&lab.app, Team::ROOT_ID).await.is_empty());
    assert_eq!(api.requests_to("/localapi/v0/logout").len(), 1);
    assert!(!lab.tailnet_dir(pro.id).exists());
    // The MCPs are still there, on the internet.
    assert_eq!(count_mcps(&lab.app).await, 2);
}

#[tokio::test]
async fn deletes_only_a_tailnet_of_the_team() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let other_team = create_team(&lab.app, "Other").await;
    let theirs = lab.row(other_team.id, admin.id, "theirs").await;

    for id in [theirs.id.to_string(), "999".into(), "abc".into()] {
        let refused = lab
            .app
            .post(&format!("/tailnets/{id}?_method=DELETE"))
            .login_as(&admin)
            .csrf()
            .send()
            .await;
        assert_redirect(&refused, "/tailnets");
        assert_eq!(
            flash(&refused, "error").as_deref(),
            Some("Tailnet not found"),
            "{id}"
        );
    }
    assert_eq!(tailnets_of(&lab.app, other_team.id).await.len(), 1);
}

/// Deletes a tailnet of the root team through the page.
async fn delete_tailnet(lab: &Lab, admin: &User, id: i64) -> TestResponse {
    let deleted = lab
        .app
        .post(&format!("/tailnets/{id}?_method=DELETE"))
        .login_as(admin)
        .csrf()
        .send()
        .await;
    assert_redirect(&deleted, "/tailnets");
    assert_eq!(
        flash(&deleted, "success").as_deref(),
        Some("Tailnet deleted")
    );
    deleted
}

#[tokio::test]
async fn deleting_a_tailnet_whose_control_server_is_no_longer_allowed_starts_no_daemon() {
    // Connected to a Headscale the operator allowed; the operator no longer does.
    let lab = Lab::with_control(TailnetControl::Official).await;
    let admin = create_admin(&lab.app).await;
    let (pro, api) = lab.joined_row(Team::ROOT_ID, admin.id, "pro").await;

    delete_tailnet(&lab, &admin, pro.id).await;

    assert_eq!(lab.runs(), 0, "no daemon was started to log out");
    assert!(api.requests().is_empty());
    assert!(!lab.tailnet_dir(pro.id).exists());
    assert!(tailnets_of(&lab.app, Team::ROOT_ID).await.is_empty());
    assert!(lab.tailnets.handle(pro.id).is_none());
    assert!(matches!(
        lab.tailnets.ensure_running(pro.id).await,
        Err(mymcps_tailnet::Error::Removed(_))
    ));
}

#[tokio::test]
async fn deleting_a_running_tailnet_whose_control_server_is_no_longer_allowed_stops_it_without_a_logout()
 {
    let lab = Lab::with_control(TailnetControl::Official).await;
    let admin = create_admin(&lab.app).await;
    let (pro, api) = lab.joined_row(Team::ROOT_ID, admin.id, "pro").await;
    // A daemon that was already running (started before the setting changed).
    lab.tailnets.ensure_running(pro.id).await.unwrap();
    assert!(lab.tailnets.handle(pro.id).is_some());
    let runs = lab.runs();

    delete_tailnet(&lab, &admin, pro.id).await;

    assert_eq!(lab.runs(), runs);
    assert!(api.requests_to("/localapi/v0/logout").is_empty());
    assert!(lab.tailnets.handle(pro.id).is_none());
    assert!(!lab.tailnet_dir(pro.id).exists());
}

#[tokio::test]
async fn deleting_a_tailnet_whose_control_server_is_allowed_still_logs_the_node_out() {
    let lab =
        Lab::with_control(TailnetControl::parse(Some("https://hs.example.com")).unwrap()).await;
    let admin = create_admin(&lab.app).await;
    let (pro, api) = lab.joined_row(Team::ROOT_ID, admin.id, "pro").await;

    delete_tailnet(&lab, &admin, pro.id).await;

    // A daemon was started, for the logout.
    assert_eq!(api.requests_to("/localapi/v0/logout").len(), 1);
    assert!(!lab.tailnet_dir(pro.id).exists());
}

// --- signing in again ---

fn sign_in_again<'a>(
    app: &'a TestApp,
    user: &User,
    id: impl std::fmt::Display,
) -> mymcps_web::testing::TestRequest<'a> {
    app.post(&format!("/tailnets/{id}/sign-in"))
        .login_as(user)
        .csrf()
}

#[tokio::test]
async fn signs_a_tailnet_in_again_whatever_it_holds_and_shows_the_new_link() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let api = lab.daemon(pro.id, status_in("Stopped"));

    // A tailnet that is not connected offers the way back, as a plain form.
    let page = get(&lab.app, &admin, "/tailnets").await;
    let row = table_rows(&page)[0];
    assert!(row.contains("badge badge--neutral\">Stopped"));
    assert!(row.contains(&format!(
        "<form method=\"post\" action=\"/tailnets/{}/sign-in\">",
        pro.id
    )));
    assert!(row.contains("Sign in again</button>"));

    let response = sign_in_again(&lab.app, &admin, pro.id).send().await;
    assert_redirect(&response, "/tailnets");
    assert_eq!(
        flash(&response, "success").as_deref(),
        Some("Use the Sign in link of the tailnet to finish")
    );
    // The node was asked, with its own name and no key, for a new login.
    let starts = api.requests_to("/localapi/v0/start");
    assert_eq!(starts.len(), 1);
    let start = starts[0].json();
    assert_eq!(start["UpdatePrefs"]["Hostname"], "mymcps-pro");
    assert_eq!(start["UpdatePrefs"]["ControlURL"], "");
    assert!(start.get("AuthKey").is_none());
    assert_eq!(api.requests_to("/localapi/v0/login-interactive").len(), 1);
    assert!(api.requests_to("/localapi/v0/logout").is_empty());

    // The daemon then has a link: the row offers it, and still the way back,
    // since a link that is shown may be one that expired.
    api.set_status(needs_login());
    let page = get(&lab.app, &admin, "/tailnets").await;
    let row = table_rows(&page)[0];
    assert!(row.contains(&format!("href=\"{LOGIN_URL}\" target=\"_blank\"")));
    assert!(row.contains("Sign in again</button>"));
    assert_eq!(tailnets_of(&lab.app, Team::ROOT_ID).await.len(), 1);
}

#[tokio::test]
async fn keeps_the_headscale_of_a_tailnet_when_it_signs_in_again() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    sqlx::query("update `tailnets` set `control_url` = ? where `id` = ?")
        .bind("https://headscale.example.com/")
        .bind(pro.id)
        .execute(&*lab.app.core.db)
        .await
        .unwrap();
    let api = lab.daemon(pro.id, status_in("NeedsLogin"));

    let response = sign_in_again(&lab.app, &admin, pro.id).send().await;

    assert_redirect(&response, "/tailnets");
    let start = api.requests_to("/localapi/v0/start")[0].json();
    assert_eq!(
        start["UpdatePrefs"]["ControlURL"],
        "https://headscale.example.com/"
    );
    assert_eq!(api.requests_to("/localapi/v0/login-interactive").len(), 1);
}

#[tokio::test]
async fn does_not_sign_in_again_a_tailnet_that_is_connected_or_waits_for_an_admin() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let api = lab.daemon(pro.id, running_status());

    let page = get(&lab.app, &admin, "/tailnets").await;
    assert!(!page.contains("Sign in again"));
    assert!(!page.contains("/sign-in"));
    for status in [
        running_status(),
        json!({ "Version": "1.104.2", "BackendState": "NeedsMachineAuth" }),
    ] {
        api.set_status(status);
        let refused = sign_in_again(&lab.app, &admin, pro.id).send().await;
        assert_redirect(&refused, "/tailnets");
        assert_eq!(
            flash(&refused, "error").as_deref(),
            Some("pro does not need to sign in again")
        );
    }
    assert!(api.requests_to("/localapi/v0/start").is_empty());
    assert!(api.requests_to("/localapi/v0/login-interactive").is_empty());
}

#[tokio::test]
async fn reports_a_daemon_that_cannot_sign_in_again() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let api = lab.daemon(pro.id, status_in("NeedsLogin"));
    api.force("/localapi/v0/login-interactive", 500, "control is down");

    let response = sign_in_again(&lab.app, &admin, pro.id).send().await;

    assert_redirect(&response, "/tailnets");
    let message = flash(&response, "error").unwrap();
    assert!(
        message.starts_with("Could not sign in again: "),
        "{message}"
    );
    assert!(flash(&response, "success").is_none());
}

#[tokio::test]
async fn signs_in_again_only_for_the_admins_of_the_team() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let member = create_member(&lab.app).await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let api = lab.daemon(pro.id, status_in("Stopped"));

    let refused = lab
        .app
        .post(&format!("/tailnets/{}/sign-in", pro.id))
        .csrf()
        .send()
        .await;
    assert_redirect(&refused, "/login");
    let refused = sign_in_again(&lab.app, &member, pro.id).send().await;
    assert_redirect(&refused, "/");
    // Nor without the token of the page.
    let forged = lab
        .app
        .post(&format!("/tailnets/{}/sign-in", pro.id))
        .login_as(&admin)
        .send()
        .await;
    assert_ne!(forged.status, StatusCode::OK);
    assert!(forged.flashed("success").is_none());
    assert!(api.requests().is_empty());
}

#[tokio::test]
async fn signs_in_again_only_a_tailnet_of_the_team() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let other_team = create_team(&lab.app, "Other").await;
    let theirs = lab.row(other_team.id, admin.id, "theirs").await;
    let api = lab.daemon(theirs.id, status_in("Stopped"));

    for id in [theirs.id.to_string(), "999".into(), "abc".into()] {
        let refused = sign_in_again(&lab.app, &admin, &id).send().await;
        assert_redirect(&refused, "/tailnets");
        assert_eq!(
            flash(&refused, "error").as_deref(),
            Some("Tailnet not found"),
            "{id}"
        );
    }
    assert!(api.requests().is_empty());
}

#[tokio::test]
async fn does_not_sign_in_again_when_tailscale_is_off() {
    let app = TestApp::with_upstream(|upstream| upstream.tailnets(None)).await;
    let admin = create_admin(&app).await;
    let mut pro = Tailnet {
        team_id: Team::ROOT_ID,
        name: "pro".into(),
        hostname: "mymcps-pro".into(),
        created_by: admin.id,
        ..Default::default()
    };
    pro.insert(&*app.core.db).await.unwrap();

    let refused = sign_in_again(&app, &admin, pro.id).send().await;

    assert_redirect(&refused, "/tailnets");
    assert!(
        flash(&refused, "error")
            .unwrap()
            .contains("Tailscale is off on this server")
    );
}

// --- the Network field of the MCP form ---

fn mcp_form<'a>(fields: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
    let mut form = vec![
        ("name", "Grafana"),
        ("description", ""),
        ("transport", "http"),
        ("httpUrl", "https://grafana.tail1234.ts.net/mcp"),
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
async fn offers_the_tailnets_of_the_team_on_an_http_mcp() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let other_team = create_team(&lab.app, "Other").await;
    lab.row(other_team.id, admin.id, "theirs").await;

    // No tailnet in the team: no field.
    assert!(
        !get(&lab.app, &admin, "/mcps/new?template=custom")
            .await
            .contains("tailnetId")
    );

    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    lab.row(Team::ROOT_ID, admin.id, "Lab").await;
    let form = get(&lab.app, &admin, "/mcps/new?template=custom").await;
    let network = between(&form, "for=\"new-mcp-tailnet\"", "</select>");
    assert!(network.contains(">Network</label>"));
    assert!(network.contains("<option value=\"\" selected>Internet</option>"));
    // By name, and no tailnet of another team.
    assert!(network.find("Tailnet: Lab").unwrap() < network.find("Tailnet: pro").unwrap());
    assert!(!network.contains("theirs"));
    assert!(form.contains("data-show-when=\"transport=http\""));

    // An MCP that has one is shown with it selected, and in the list.
    let mcp = create_mcp(&lab.app, admin.id, |mcp| mcp.tailnet_id = Some(pro.id)).await;
    let edit = get(&lab.app, &admin, &format!("/mcps/{}/edit", mcp.id)).await;
    assert!(edit.contains(&format!(
        "<option value=\"{}\" selected>Tailnet: pro</option>",
        pro.id
    )));
    let list = get(&lab.app, &admin, "/mcps").await;
    assert!(list.contains("via tailnet pro"));
    assert!(!between(&list, "<tbody>", "</tbody>").contains("theirs"));
    // A search finds it by its tailnet.
    let found = get(&lab.app, &admin, "/mcps?q=pro").await;
    assert!(found.contains("via tailnet pro"));
}

#[tokio::test]
async fn saves_the_network_of_an_http_mcp_and_clears_it_for_the_others() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let id = pro.id.to_string();

    let created = connect_mcp(&lab, &admin, &mcp_form(&[("tailnetId", &id)])).await;
    assert_redirect(&created, "/mcps");
    let mcp = mcp_by_slug(&lab.app, "grafana").await.unwrap();
    assert_eq!(mcp.tailnet_id, Some(pro.id));

    // Back to the internet with the blank option.
    let updated = lab
        .app
        .post(&format!("/mcps/{}?_method=PUT", mcp.id))
        .login_as(&admin)
        .csrf()
        .form(&mcp_form(&[("tailnetId", "")]))
        .send()
        .await;
    assert_redirect(&updated, "/mcps");
    assert_eq!(find_mcp(&lab.app, mcp.id).await.tailnet_id, None);

    // An npm MCP is not reached over a network: whatever the form says, it has none.
    let npm = lab
        .app
        .post("/mcps")
        .login_as(&admin)
        .csrf()
        .form(&[
            ("name", "Package"),
            ("transport", "npm"),
            ("npmPackage", "@example/mcp"),
            ("authType", "auto"),
            ("tailnetId", &id),
        ])
        .send()
        .await;
    assert_redirect(&npm, "/mcps");
    assert_eq!(
        mcp_by_slug(&lab.app, "package").await.unwrap().tailnet_id,
        None
    );
    assert_eq!(
        mcp_by_slug(&lab.app, "package").await.unwrap().transport,
        McpTransport::Npm
    );

    // And one that moves from HTTP to npm forgets it.
    let again = connect_mcp(
        &lab,
        &admin,
        &mcp_form(&[("name", "Second"), ("tailnetId", &id)]),
    )
    .await;
    assert_redirect(&again, "/mcps");
    let second = mcp_by_slug(&lab.app, "second").await.unwrap();
    assert_eq!(second.tailnet_id, Some(pro.id));
    let moved = lab
        .app
        .post(&format!("/mcps/{}?_method=PUT", second.id))
        .login_as(&admin)
        .csrf()
        .form(&[
            ("name", "Second"),
            ("transport", "npm"),
            ("npmPackage", "@example/mcp"),
            ("authType", "auto"),
            ("tailnetId", &id),
        ])
        .send()
        .await;
    assert_redirect(&moved, "/mcps");
    assert_eq!(find_mcp(&lab.app, second.id).await.tailnet_id, None);
}

async fn connect_mcp(lab: &Lab, user: &User, form: &[(&str, &str)]) -> TestResponse {
    lab.app
        .post("/mcps")
        .login_as(user)
        .csrf()
        .form(form)
        .send()
        .await
}

#[tokio::test]
async fn refuses_the_tailnet_of_another_team_or_one_that_does_not_exist() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let other_team = create_team(&lab.app, "Other").await;
    let theirs = lab.row(other_team.id, admin.id, "theirs").await;
    lab.row(Team::ROOT_ID, admin.id, "pro").await;

    for id in [theirs.id.to_string(), "999".to_string()] {
        let refused = from_script(
            lab.app
                .post("/mcps")
                .login_as(&admin)
                .csrf()
                .form(&mcp_form(&[("tailnetId", &id)])),
        )
        .send()
        .await;
        assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY, "{id}");
        let text = refused.text();
        assert!(text.contains("Choose a tailnet of this team, or the internet"));
        assert!(text.contains("id=\"new-mcp-tailnet-error\""));
        // The field comes back with the choices of the team only.
        assert!(!text.contains("theirs"));
    }
    assert_eq!(count_mcps(&lab.app).await, 0);

    // The same on an edit: the MCP stays as it was.
    let mcp = create_mcp(&lab.app, admin.id, |_| {}).await;
    let refused = lab
        .app
        .post(&format!("/mcps/{}?_method=PUT", mcp.id))
        .login_as(&admin)
        .csrf()
        .form(&mcp_form(&[("tailnetId", &theirs.id.to_string())]))
        .send()
        .await;
    assert_redirect(&refused, "/mcps");
    assert!(
        flash(&refused, "error")
            .unwrap()
            .contains("Choose a tailnet of this team")
    );
    assert_eq!(find_mcp(&lab.app, mcp.id).await.tailnet_id, None);
    assert_ne!(find_mcp(&lab.app, mcp.id).await.name, "Grafana");

    // A value that is no id at all.
    let refused = from_script(
        lab.app
            .post("/mcps")
            .login_as(&admin)
            .csrf()
            .form(&mcp_form(&[("tailnetId", "pro")])),
    )
    .send()
    .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(count_mcps(&lab.app).await, 1);
}

// --- what deleting a team or a user takes with it ---

#[tokio::test]
async fn stops_the_daemons_of_a_team_that_is_deleted() {
    let lab = Lab::new().await;
    let root_owner = create_super_admin(&lab.app).await;
    let team = create_team(&lab.app, "Research").await;
    let owner = create_account(&lab.app, None).await;
    join_team(&lab.app, team.id, &owner, TeamRole::Owner).await;
    let theirs = lab.row(team.id, owner.id, "theirs").await;
    let kept = lab.row(Team::ROOT_ID, root_owner.id, "kept").await;
    let theirs_api = lab.daemon(theirs.id, running_status());
    let kept_api = lab.daemon(kept.id, running_status());
    lab.tailnets.ensure_running(theirs.id).await.unwrap();
    lab.tailnets.ensure_running(kept.id).await.unwrap();

    let deleted = lab
        .app
        .post("/team?_method=DELETE")
        .login_as(&owner)
        .in_team(team.id)
        .csrf()
        .send()
        .await;
    assert_redirect(&deleted, "/");
    assert_eq!(flash(&deleted, "success").as_deref(), Some("Team deleted"));

    // Its tailnet is gone with the team, and its node left the tailnet.
    assert!(
        Tailnet::find(&*lab.app.core.db, theirs.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(theirs_api.requests_to("/localapi/v0/logout").len(), 1);
    assert!(!lab.tailnet_dir(theirs.id).exists());
    assert!(lab.tailnets.handle(theirs.id).is_none());
    // Another team's is not touched.
    assert!(
        Tailnet::find(&*lab.app.core.db, kept.id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(kept_api.requests_to("/localapi/v0/logout").is_empty());
    assert!(lab.tailnets.handle(kept.id).is_some());
}

#[tokio::test]
async fn stops_the_daemons_of_a_user_deleted_with_their_personal_team() {
    let lab = Lab::new().await;
    let actor = create_super_admin(&lab.app).await;
    let outsider = create_account(&lab.app, None).await;
    let mut transaction = lab.app.core.db.begin().await.unwrap();
    let personal = Team::create_personal(&mut transaction, &outsider)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    // A team they leave behind keeps its tailnet.
    let shared = create_team(&lab.app, "Shared").await;
    join_team(&lab.app, shared.id, &outsider, TeamRole::Admin).await;
    join_team(&lab.app, shared.id, &actor, TeamRole::Owner).await;

    let mine = lab.row(personal.id, outsider.id, "mine").await;
    let common = lab.row(shared.id, outsider.id, "common").await;
    let mine_api = lab.daemon(mine.id, running_status());
    let common_api = lab.daemon(common.id, running_status());
    lab.tailnets.ensure_running(mine.id).await.unwrap();
    lab.tailnets.ensure_running(common.id).await.unwrap();

    let deleted = lab
        .app
        .post(&format!("/users/{}?_method=DELETE", outsider.id))
        .login_as(&actor)
        .csrf()
        .form(&[("currentPassword", PASSWORD)])
        .send()
        .await;
    assert_redirect(&deleted, "/users");
    assert_eq!(flash(&deleted, "success").as_deref(), Some("User deleted"));

    assert!(
        Tailnet::find(&*lab.app.core.db, mine.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(mine_api.requests_to("/localapi/v0/logout").len(), 1);
    assert!(!lab.tailnet_dir(mine.id).exists());
    // The shared team's tailnet stays, and goes to an owner of the team.
    let common = Tailnet::find(&*lab.app.core.db, common.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(common.created_by, actor.id);
    assert!(common_api.requests_to("/localapi/v0/logout").is_empty());
    assert!(lab.tailnets.handle(common.id).is_some());
}

#[tokio::test]
async fn deleting_a_team_starts_no_daemon_for_a_control_server_that_is_no_longer_allowed() {
    let lab = Lab::with_control(TailnetControl::Official).await;
    let team = create_team(&lab.app, "Research").await;
    let owner = create_account(&lab.app, None).await;
    join_team(&lab.app, team.id, &owner, TeamRole::Owner).await;
    let (forbidden, forbidden_api) = lab.joined_row(team.id, owner.id, "forbidden").await;
    // Tailscale's own control server is always allowed, and still logs out.
    let own = lab.row(team.id, owner.id, "own").await;
    let own_api = lab.daemon(own.id, running_status());
    std::fs::write(lab.tailnet_dir(own.id).join("tailscaled.state"), "{}").unwrap();

    let deleted = lab
        .app
        .post("/team?_method=DELETE")
        .login_as(&owner)
        .in_team(team.id)
        .csrf()
        .send()
        .await;
    assert_redirect(&deleted, "/");
    assert_eq!(flash(&deleted, "success").as_deref(), Some("Team deleted"));

    assert!(forbidden_api.requests().is_empty());
    assert!(!lab.tailnet_dir(forbidden.id).exists());
    assert_eq!(own_api.requests_to("/localapi/v0/logout").len(), 1);
    assert!(!lab.tailnet_dir(own.id).exists());
}

#[tokio::test]
async fn deleting_a_team_still_logs_out_the_nodes_of_an_allowed_control_server() {
    let lab =
        Lab::with_control(TailnetControl::parse(Some("https://hs.example.com")).unwrap()).await;
    let team = create_team(&lab.app, "Research").await;
    let owner = create_account(&lab.app, None).await;
    join_team(&lab.app, team.id, &owner, TeamRole::Owner).await;
    let (hs, api) = lab.joined_row(team.id, owner.id, "hs").await;

    let deleted = lab
        .app
        .post("/team?_method=DELETE")
        .login_as(&owner)
        .in_team(team.id)
        .csrf()
        .send()
        .await;
    assert_redirect(&deleted, "/");

    assert_eq!(api.requests_to("/localapi/v0/logout").len(), 1);
    assert!(!lab.tailnet_dir(hs.id).exists());
}

#[tokio::test]
async fn deleting_a_user_starts_no_daemon_for_a_control_server_that_is_no_longer_allowed() {
    let lab = Lab::with_control(TailnetControl::Official).await;
    let actor = create_super_admin(&lab.app).await;
    let outsider = create_account(&lab.app, None).await;
    let mut transaction = lab.app.core.db.begin().await.unwrap();
    let personal = Team::create_personal(&mut transaction, &outsider)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    let (mine, api) = lab.joined_row(personal.id, outsider.id, "mine").await;

    let deleted = lab
        .app
        .post(&format!("/users/{}?_method=DELETE", outsider.id))
        .login_as(&actor)
        .csrf()
        .form(&[("currentPassword", PASSWORD)])
        .send()
        .await;
    assert_redirect(&deleted, "/users");
    assert_eq!(flash(&deleted, "success").as_deref(), Some("User deleted"));

    assert!(
        Tailnet::find(&*lab.app.core.db, mine.id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(lab.runs(), 0, "no daemon was started to log out");
    assert!(api.requests().is_empty());
    assert!(!lab.tailnet_dir(mine.id).exists());
}

#[tokio::test]
async fn keeps_the_mcp_a_team_had_through_a_tailnet_on_the_internet_when_it_goes() {
    // `on delete set null`: deleting the row never leaves an MCP pointing at nothing.
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let mcp: Mcp = create_mcp(&lab.app, admin.id, |mcp| mcp.tailnet_id = Some(pro.id)).await;
    pro.delete(&*lab.app.core.db).await.unwrap();
    assert_eq!(find_mcp(&lab.app, mcp.id).await.tailnet_id, None);
}

/// An HTTP MCP that holds a bearer token, a header value and an OAuth
/// connection, on `network`.
async fn mcp_with_secrets(lab: &Lab, user: &User, name: &str, network: Option<i64>) -> Mcp {
    create_mcp(&lab.app, user.id, |mcp| {
        mcp.name = name.into();
        mcp.slug = name.to_lowercase();
        mcp.http_url = Some("https://grafana.tail1234.ts.net/mcp".into());
        mcp.tailnet_id = network;
        mcp.auth_type = McpAuthType::Bearer;
        mcp.auth_bearer = encrypt(&lab.app, "bearer-secret");
        mcp.auth_header_value = encrypt(&lab.app, "header-secret");
        mcp.oauth_access_token = encrypt(&lab.app, "access-token");
        mcp.oauth_refresh_token = encrypt(&lab.app, "refresh-token");
        mcp.oauth_client_id = Some("registered-client".into());
        mcp.oauth_client_secret = encrypt(&lab.app, "client-secret");
        mcp.oauth_token_url = Some("https://grafana.tail1234.ts.net/token".into());
        mcp.oauth_authorize_url = Some("https://grafana.tail1234.ts.net/authorize".into());
    })
    .await
}

/// Saves the MCP again with the same URL and sign-in, on `network`.
async fn save_on(lab: &Lab, user: &User, mcp: &Mcp, network: Option<i64>) {
    save_with(lab, user, mcp, "bearer", network).await;
}

async fn save_with(lab: &Lab, user: &User, mcp: &Mcp, auth_type: &str, network: Option<i64>) {
    let id = network.map(|id| id.to_string()).unwrap_or_default();
    let response = lab
        .app
        .post(&format!("/mcps/{}?_method=PUT", mcp.id))
        .login_as(user)
        .csrf()
        .form(&[
            ("name", mcp.name.as_str()),
            ("transport", "http"),
            ("httpUrl", "https://grafana.tail1234.ts.net/mcp"),
            ("authType", auth_type),
            ("tailnetId", id.as_str()),
            ("enabled", "on"),
        ])
        .send()
        .await;
    assert_redirect(&response, "/mcps");
}

#[tokio::test]
async fn forgets_the_secrets_and_the_sign_in_of_an_mcp_when_only_its_network_changes() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let home = lab.row(Team::ROOT_ID, admin.id, "home").await;

    // The same URL is another machine on another network.
    for (from, to) in [
        (None, Some(pro.id)),
        (Some(pro.id), None),
        (Some(pro.id), Some(home.id)),
    ] {
        let name = format!("Moves {}-{}", from.unwrap_or(0), to.unwrap_or(0));
        let mcp = mcp_with_secrets(&lab, &admin, &name, from).await;

        save_on(&lab, &admin, &mcp, to).await;

        let saved = find_mcp(&lab.app, mcp.id).await;
        assert_eq!(saved.tailnet_id, to, "{name}");
        assert_eq!(saved.auth_bearer, None, "{name}");
        assert_eq!(saved.auth_header_value, None, "{name}");
        assert_eq!(saved.oauth_access_token, None, "{name}");
        assert_eq!(saved.oauth_refresh_token, None, "{name}");
        assert_eq!(saved.oauth_client_id, None, "{name}");
        assert_eq!(saved.oauth_client_secret, None, "{name}");
        assert_eq!(saved.oauth_token_url, None, "{name}");
        assert_eq!(saved.oauth_authorize_url, None, "{name}");
    }
}

#[tokio::test]
async fn keeps_the_secrets_and_the_sign_in_of_an_mcp_that_stays_on_its_network() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    for (name, network) in [("Stays on pro", Some(pro.id)), ("Stays online", None)] {
        let mcp = mcp_with_secrets(&lab, &admin, name, network).await;
        save_on(&lab, &admin, &mcp, network).await;

        let saved = find_mcp(&lab.app, mcp.id).await;
        assert_eq!(saved.tailnet_id, network, "{name}");
        assert_eq!(saved.auth_bearer, mcp.auth_bearer, "{name}");
        assert_eq!(saved.auth_header_value, None, "{name}: unused by a bearer");
        assert_eq!(saved.oauth_access_token, None, "{name}: unused by a bearer");
    }
}

#[tokio::test]
async fn keeps_the_oauth_connection_of_an_mcp_that_stays_on_its_network_and_drops_it_on_leaving() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let mcp = create_mcp(&lab.app, admin.id, |mcp| {
        mcp.name = "Signed in".into();
        mcp.http_url = Some("https://grafana.tail1234.ts.net/mcp".into());
        mcp.tailnet_id = Some(pro.id);
        mcp.auth_type = McpAuthType::Auto;
        mcp.oauth_access_token = encrypt(&lab.app, "access-token");
        mcp.oauth_client_id = Some("registered-client".into());
    })
    .await;
    save_with(&lab, &admin, &mcp, "auto", Some(pro.id)).await;
    let saved = find_mcp(&lab.app, mcp.id).await;
    assert_eq!(saved.oauth_access_token, mcp.oauth_access_token);
    assert_eq!(saved.oauth_client_id.as_deref(), Some("registered-client"));

    // Moving it to the internet signs it out.
    save_with(&lab, &admin, &mcp, "auto", None).await;
    let saved = find_mcp(&lab.app, mcp.id).await;
    assert_eq!(saved.tailnet_id, None);
    assert_eq!(saved.oauth_access_token, None);
    assert_eq!(saved.oauth_client_id, None);
}

#[tokio::test]
async fn the_form_stops_offering_to_keep_the_saved_secrets_for_another_network() {
    let lab = Lab::new().await;
    let admin = create_admin(&lab.app).await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let internet = mcp_with_secrets(&lab, &admin, "Online", None).await;
    let tailnet = mcp_with_secrets(&lab, &admin, "Inside", Some(pro.id)).await;

    // The page script follows the Network field with this condition.
    let page = get(&lab.app, &admin, &format!("/mcps/{}/edit", internet.id)).await;
    assert!(
        page.contains("data-show-when=\"transport=http&amp;httpUrl:origin=https://grafana.tail1234.ts.net&amp;tailnetId=\""),
        "{page}"
    );
    let page = get(&lab.app, &admin, &format!("/mcps/{}/edit", tailnet.id)).await;
    assert!(
        page.contains(&format!("&amp;tailnetId={}\"", pro.id)),
        "{page}"
    );
}
