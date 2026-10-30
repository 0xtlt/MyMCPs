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
use mymcps_tailnet::{Error as TailnetError, TailnetState, Tailnets, TailnetsConfig};
use mymcps_web::testing::factories::{
    PASSWORD, create_account, create_admin, create_mcp, create_member, create_super_admin,
    create_team, join_team,
};
use mymcps_web::testing::{TestApp, TestRequest, TestResponse};
use serde_json::{Value, json};
use tempfile::TempDir;

const KEY: &str = "tskey-auth-kS3cr3tK3y-abcdef";
const LOGIN_URL: &str = "https://login.tailscale.com/a/abc123";
const START: &str = "/localapi/v0/start";
const LOGIN: &str = "/localapi/v0/login-interactive";
const LOGOUT: &str = "/localapi/v0/logout";
const HEADSCALE: &str = "https://hs.example.com/";
const LINK: &[(&str, &str)] = &[("name", "pro"), ("method", "link")];

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

    /// [`new`](Self::new), and an admin of its root team.
    async fn with_admin() -> (Self, User) {
        let lab = Self::new().await;
        let admin = create_admin(&lab.app).await;
        (lab, admin)
    }

    /// A server that allows the control servers in `urls` (a list as the
    /// operator writes it) and Tailscale's own.
    async fn allowing(urls: &str) -> Self {
        Self::with_control(TailnetControl::parse(Some(urls)).unwrap()).await
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

    /// A tailnet row, as if connected earlier.
    async fn row(&self, team_id: i64, created_by: i64, name: &str) -> Tailnet {
        Tailnet::create_for_tests(&*self.app.core.db, team_id, created_by, name).await
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
        set_control(&self.app, tailnet.id, HEADSCALE).await;
        let api = self.daemon(tailnet.id, running_status());
        std::fs::write(self.tailnet_dir(tailnet.id).join("tailscaled.state"), "{}").unwrap();
        (tailnet, api)
    }

    /// The tailnet is gone with its daemon, its directory and its id.
    async fn assert_removed(&self, id: i64) {
        assert!(!tailnet_exists(&self.app, id).await);
        assert!(self.tailnets.handle(id).is_none());
        assert!(!self.tailnet_dir(id).exists());
        assert!(matches!(
            self.tailnets.ensure_running(id).await,
            Err(TailnetError::Removed(_))
        ));
    }
}

async fn set_control(app: &TestApp, id: i64, url: &str) {
    sqlx::query("update `tailnets` set `control_url` = ? where `id` = ?")
        .bind(url)
        .bind(id)
        .execute(&*app.core.db)
        .await
        .unwrap();
}

async fn tailnet_exists(app: &TestApp, id: i64) -> bool {
    Tailnet::find(&*app.core.db, id).await.unwrap().is_some()
}

/// The personal team of `user`, as an account has one when it is created.
async fn personal_team(app: &TestApp, user: &User) -> Team {
    let mut transaction = app.core.db.begin().await.unwrap();
    let team = Team::create_personal(&mut transaction, user).await.unwrap();
    transaction.commit().await.unwrap();
    team
}

/// A team and its owner, an account that is no one else's.
async fn team_with_owner(app: &TestApp, name: &str) -> (Team, User) {
    let team = create_team(app, name).await;
    let owner = create_account(app, None).await;
    join_team(app, team.id, &owner, TeamRole::Owner).await;
    (team, owner)
}

fn needs_login() -> Value {
    json!({ "Version": "1.104.2", "BackendState": "NeedsLogin", "AuthURL": LOGIN_URL })
}

fn logouts(api: &FakeLocalApi) -> usize {
    api.requests_to(LOGOUT).len()
}

/// The node was asked to join as `hostname` on `control`, with `key` or
/// without, and to log in.
fn assert_joined(api: &FakeLocalApi, hostname: &str, control: &str, key: Option<&str>) {
    let [start] = api.requests_to(START).try_into().unwrap();
    let start = start.json();
    assert_eq!(start["UpdatePrefs"]["Hostname"], hostname);
    assert_eq!(start["UpdatePrefs"]["ControlURL"], control);
    match key {
        Some(key) => assert_eq!(start["AuthKey"], key),
        None => assert!(start.get("AuthKey").is_none(), "{start}"),
    }
    assert_eq!(api.requests_to(LOGIN).len(), 1);
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

/// A redirect to `to` that flashes `message` as a success.
fn assert_done(response: &TestResponse, to: &str, message: &str) {
    assert_redirect(response, to);
    assert_eq!(flash(response, "success").as_deref(), Some(message));
}

/// A redirect to `/tailnets` that flashes `message` as an error.
fn assert_refused(response: &TestResponse, message: &str) {
    assert_redirect(response, "/tailnets");
    assert_eq!(flash(response, "error").as_deref(), Some(message));
}

async fn tailnets_of(app: &TestApp, team_id: i64) -> Vec<Tailnet> {
    Tailnet::of_team(&*app.core.db, team_id).await.unwrap()
}

async fn get(app: &TestApp, user: &User, path: &str) -> String {
    let response = app.get(path).login_as(user).send().await;
    assert_eq!(response.status, StatusCode::OK, "{path}");
    response.text()
}

/// A post of `user` with the token of the page.
fn post_as<'a>(app: &'a TestApp, user: &User, path: &str) -> TestRequest<'a> {
    app.post(path).login_as(user).csrf()
}

fn connect<'a>(app: &'a TestApp, user: &User, form: &[(&str, &str)]) -> TestRequest<'a> {
    post_as(app, user, "/tailnets").form(form)
}

/// [`connect`], sent as the script of the page does.
async fn connect_scripted(app: &TestApp, user: &User, form: &[(&str, &str)]) -> TestResponse {
    from_script(connect(app, user, form)).send().await
}

async fn delete_tailnet(app: &TestApp, user: &User, id: impl std::fmt::Display) -> TestResponse {
    post_as(app, user, &format!("/tailnets/{id}?_method=DELETE"))
        .send()
        .await
}

async fn delete_team(app: &TestApp, owner: &User, team_id: i64) -> TestResponse {
    post_as(app, owner, "/team?_method=DELETE")
        .in_team(team_id)
        .send()
        .await
}

async fn delete_user(app: &TestApp, actor: &User, id: i64) -> TestResponse {
    post_as(app, actor, &format!("/users/{id}?_method=DELETE"))
        .form(&[("currentPassword", PASSWORD)])
        .send()
        .await
}

fn sign_in_again<'a>(app: &'a TestApp, user: &User, id: impl std::fmt::Display) -> TestRequest<'a> {
    post_as(app, user, &format!("/tailnets/{id}/sign-in"))
}

/// The status is cached for a moment: waits for it to be read again.
async fn status_becomes(api: &FakeLocalApi, status: Value) {
    api.set_status(status);
    tokio::time::sleep(std::time::Duration::from_millis(2200)).await;
}

// --- who sees the page ---

#[tokio::test]
async fn is_for_the_admins_of_a_team() {
    let (lab, admin) = Lab::with_admin().await;
    let member = create_member(&lab.app).await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let api = lab.daemon(pro.id, status_in("Stopped"));
    let sign_in = format!("/tailnets/{}/sign-in", pro.id);

    assert_redirect(&lab.app.get("/tailnets").send().await, "/login");
    assert_redirect(&lab.app.get("/tailnets/new").send().await, "/login");
    assert_redirect(&lab.app.post(&sign_in).csrf().send().await, "/login");
    for path in ["/tailnets", "/tailnets/new"] {
        let refused = lab.app.get(path).login_as(&member).send().await;
        assert_redirect(&refused, "/");
    }
    assert_redirect(&connect(&lab.app, &member, LINK).send().await, "/");
    assert_redirect(&delete_tailnet(&lab.app, &member, pro.id).await, "/");
    assert_redirect(&sign_in_again(&lab.app, &member, pro.id).send().await, "/");
    // Nor without the token of the page.
    let forged = lab.app.post(&sign_in).login_as(&admin).send().await;
    assert_ne!(forged.status, StatusCode::OK);
    assert!(forged.flashed("success").is_none());
    assert_eq!(tailnets_of(&lab.app, Team::ROOT_ID).await.len(), 1);
    assert!(api.requests().is_empty());

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
    let (lab, admin) = Lab::with_admin().await;

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

    let refused = connect_scripted(&app, &admin, LINK).await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(refused.text().contains("Tailscale is off on this server"));
    assert!(tailnets_of(&app, Team::ROOT_ID).await.is_empty());

    // A row that exists is still listed, and can be deleted, but not signed in again.
    let tailnet = Tailnet::create_for_tests(&*app.core.db, Team::ROOT_ID, admin.id, "pro").await;
    let page = get(&app, &admin, "/tailnets").await;
    assert!(page.contains("badge--neutral\">Unavailable"));
    let refused = sign_in_again(&app, &admin, tailnet.id).send().await;
    assert_redirect(&refused, "/tailnets");
    assert!(
        flash(&refused, "error")
            .unwrap()
            .contains("Tailscale is off on this server")
    );
    let deleted = delete_tailnet(&app, &admin, tailnet.id).await;
    assert_eq!(
        flash(&deleted, "success").as_deref(),
        Some("Tailnet deleted")
    );
    assert!(tailnets_of(&app, Team::ROOT_ID).await.is_empty());
}

#[tokio::test]
async fn lists_the_tailnets_of_the_team_with_where_each_stands() {
    let (lab, admin) = Lab::with_admin().await;
    let other_team = create_team(&lab.app, "Other").await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let perso = lab.row(Team::ROOT_ID, admin.id, "perso").await;
    let elsewhere = lab.row(other_team.id, admin.id, "elsewhere").await;
    let _pro = lab.daemon(pro.id, running_status());
    let _perso = lab.daemon(perso.id, needs_login());
    let _elsewhere = lab.daemon(elsewhere.id, running_status());
    let script = lab.row(Team::ROOT_ID, admin.id, "script").await;
    let _script = lab.daemon(
        script.id,
        json!({ "BackendState": "NeedsLogin", "AuthURL": "javascript:alert(1)" }),
    );
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
    assert_eq!(rows.len(), 3);
    // By name: perso, pro, script.
    let (perso_row, pro_row, script_row) = (rows[0], rows[1], rows[2]);
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
    // Only a web address is a link to sign in.
    assert!(script_row.contains("Needs sign-in"));
    assert!(script_row.contains("Waiting for the sign-in link"));
    assert!(!page.contains("javascript:"));
    // A tailnet that waits for its sign-in asks for the page again.
    assert!(page.contains("<meta http-equiv=\"refresh\" content=\"5\">"));
}

#[tokio::test]
async fn refreshes_while_a_tailnet_is_not_settled_and_connect_stays_a_plain_link_meanwhile() {
    let (lab, admin) = Lab::with_admin().await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let api = lab.daemon(pro.id, status_in("Starting"));

    let page = get(&lab.app, &admin, "/tailnets").await;
    assert!(page.contains("badge badge--neutral\">Starting"));
    assert!(page.contains("http-equiv=\"refresh\""));
    // The script would open the dialog on a page that reloads itself every
    // few seconds, and the reload would empty it while it is being filled.
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
    // The page it leads to has the dialog open, and never a refresh: what is
    // typed would be lost.
    let new = get(&lab.app, &admin, "/tailnets/new").await;
    assert!(!new.contains("http-equiv"));
    assert!(between(&new, "id=\"connect-tailnet\"", ">").contains("data-open"));

    // Once everything is settled the page stays, and the script opens the dialog.
    status_becomes(&api, running_status()).await;
    let page = get(&lab.app, &admin, "/tailnets").await;
    assert!(page.contains("badge badge--success\">Connected"));
    assert!(!page.contains("http-equiv"));
    assert!(page.contains("href=\"/tailnets/new\" data-dialog-open=\"#connect-tailnet\""));

    status_becomes(
        &api,
        json!({ "Version": "1.104.2", "BackendState": "NeedsMachineAuth" }),
    )
    .await;
    let page = get(&lab.app, &admin, "/tailnets").await;
    assert!(page.contains("Needs approval"));
    assert!(page.contains("http-equiv"));
}

// --- connecting ---

#[tokio::test]
async fn connects_a_tailnet_with_a_sign_in_link() {
    let (lab, admin) = Lab::with_admin().await;
    let api = lab.daemon(1, needs_login());

    let form = [("name", "  pro "), ("method", "link"), ("controlUrl", "")];
    let response = connect(&lab.app, &admin, &form).send().await;
    assert_done(
        &response,
        "/tailnets",
        "Tailnet added: use its Sign in link to finish",
    );

    let saved = tailnets_of(&lab.app, Team::ROOT_ID).await;
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].id, 1);
    assert_eq!(saved[0].name, "pro");
    assert_eq!(saved[0].hostname, "mymcps-pro");
    assert_eq!(saved[0].control_url, None);
    assert_eq!(saved[0].created_by, admin.id);
    // The node was asked to join as `mymcps-pro`, with no key, and to log in.
    assert_joined(&api, "mymcps-pro", "", None);

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
    let (lab, admin) = Lab::with_admin().await;
    // A new node: tailscaled waits for a login, which the key answers.
    let api = lab.daemon(1, status_in("NeedsLogin"));

    let response = connect_scripted(
        &lab.app,
        &admin,
        &[
            ("name", "Home lab"),
            ("method", "key"),
            ("authKey", KEY),
            ("controlUrl", "https://headscale.example.com"),
        ],
    )
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
    assert_joined(
        &api,
        "mymcps-home-lab",
        "https://headscale.example.com/",
        Some(KEY),
    );

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
    let (lab, admin) = Lab::with_admin().await;
    let long_key = "k".repeat(2000);

    // The form, what the dialog that comes back says, and what it must not.
    type Case<'a> = (&'a [(&'a str, &'a str)], &'a [&'a str], &'a [&'a str]);
    let cases: [Case<'_>; 6] = [
        (
            &[("name", ""), ("method", "key"), ("authKey", KEY)],
            &[
                "aria-invalid=\"true\"",
                "field__error",
                "name=\"method\" value=\"key\" checked",
            ],
            &[],
        ),
        (
            &[("name", "-pro"), ("method", "link")],
            &["id=\"connect-tailnet-name-error\"", "value=\"-pro\""],
            &[],
        ),
        (
            &[("name", "pro"), ("method", "key")],
            &["id=\"connect-tailnet-key-error\""],
            &[],
        ),
        // What no field says is said above the form.
        (
            &[("name", "pro"), ("method", "carrier-pigeon")],
            &["This tailnet could not be connected"],
            &[],
        ),
        (
            &[
                ("name", "pro"),
                ("method", "key"),
                ("authKey", KEY),
                ("controlUrl", "ftp://headscale.example.com"),
            ],
            &[
                "Control server URL must use HTTP or HTTPS",
                "value=\"ftp://headscale.example.com\"",
            ],
            &[],
        ),
        (
            &[
                ("name", "pro"),
                ("method", "key"),
                ("authKey", KEY),
                ("controlUrl", "https://user:pw@headscale.example.com"),
            ],
            &["must not include a user name or a password"],
            &["pw@"],
        ),
    ];
    for (form, contains, lacks) in cases {
        let scripted = connect_scripted(&lab.app, &admin, form).await;
        assert_eq!(
            scripted.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{form:?}"
        );
        // The dialog comes back with what was typed, and without the key.
        let text = scripted.text();
        assert!(!text.contains(KEY), "{text}");
        for expected in contains {
            assert!(text.contains(expected), "{expected}: {text}");
        }
        for unexpected in lacks {
            assert!(!text.contains(unexpected), "{unexpected}: {text}");
        }
    }

    // A plain post goes back to the list with the first error, the key is
    // not kept in the session.
    let form = [("name", "pro"), ("method", "key"), ("authKey", &*long_key)];
    let plain = connect(&lab.app, &admin, &form).send().await;
    assert_redirect(&plain, "/tailnets");
    assert!(flash(&plain, "error").is_some());
    assert!(!format!("{:?}", plain.session()).contains("kkkkkkkk"));

    // Without a token, nothing happens.
    let forged = lab
        .app
        .post("/tailnets")
        .login_as(&admin)
        .form(LINK)
        .send()
        .await;
    assert_ne!(forged.status, StatusCode::NO_CONTENT);
    assert!(tailnets_of(&lab.app, Team::ROOT_ID).await.is_empty());
}

#[tokio::test]
async fn refuses_a_name_the_team_already_uses_whatever_its_case() {
    let (lab, admin) = Lab::with_admin().await;
    let other_team = create_team(&lab.app, "Other").await;
    lab.row(Team::ROOT_ID, admin.id, "Pro").await;
    // Another team's tailnet of that name is no obstacle.
    lab.row(other_team.id, admin.id, "work").await;

    let duplicate = connect_scripted(&lab.app, &admin, LINK).await;
    assert_eq!(duplicate.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        duplicate
            .text()
            .contains("This team already has a tailnet with this name")
    );
    assert_eq!(tailnets_of(&lab.app, Team::ROOT_ID).await.len(), 1);

    let _api = lab.daemon(3, needs_login());
    let allowed = connect(&lab.app, &admin, &[("name", "work"), ("method", "link")]);
    assert_redirect(&allowed.send().await, "/tailnets");
    assert_eq!(tailnets_of(&lab.app, Team::ROOT_ID).await.len(), 2);
}

#[tokio::test]
async fn lets_one_of_two_creates_whose_names_differ_by_case_win() {
    let (lab, admin) = Lab::with_admin().await;
    // Both may pass the check of the names and meet at the unique index:
    // the one that loses gets the usual field error, never a failure.
    let _first = lab.daemon(1, needs_login());
    let _second = lab.daemon(2, needs_login());
    let sent = async |name| {
        connect_scripted(&lab.app, &admin, &[("name", name), ("method", "link")]).await
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
    let (lab, admin) = Lab::with_admin().await;
    let other_team = create_team(&lab.app, "Other").await;
    join_team(&lab.app, other_team.id, &admin, TeamRole::Owner).await;
    for name in ["a", "b", "c", "d", "e"] {
        lab.row(Team::ROOT_ID, admin.id, name).await;
    }

    let page = get(&lab.app, &admin, "/tailnets").await;
    assert!(page.contains("title=\"A team can connect up to 5 tailnets\""));
    assert!(!page.contains("id=\"connect-tailnet\""));

    let refused = connect_scripted(&lab.app, &admin, &[("name", "f"), ("method", "link")]).await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        refused
            .text()
            .contains("A team can connect up to 5 tailnets")
    );
    assert_eq!(tailnets_of(&lab.app, Team::ROOT_ID).await.len(), 5);

    // The limit is the team's: another team of the same person connects its own.
    let _api = lab.daemon(6, needs_login());
    let allowed = connect(&lab.app, &admin, &[("name", "f"), ("method", "link")])
        .in_team(other_team.id)
        .send()
        .await;
    assert_redirect(&allowed, "/tailnets");
    assert_eq!(tailnets_of(&lab.app, other_team.id).await.len(), 1);
}

#[tokio::test]
async fn takes_the_tailnet_back_when_the_daemon_refuses_to_join() {
    let (lab, admin) = Lab::with_admin().await;
    let api = lab.daemon(1, status_in("NoState"));
    api.force(START, 500, &format!("invalid key {KEY}"));

    let form = [("name", "pro"), ("method", "key"), ("authKey", KEY)];
    let refused = from_script(connect(&lab.app, &admin, &form)).send().await;
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
    let (lab, admin) = Lab::with_admin().await;
    // The deletion of the team that holds the new row landed between its
    // insert and the start of its daemon: its id is tombstoned. The next
    // id is the one the form is about to get; ids are never reused.
    lab.tailnets.remove(1).await.unwrap();
    // A daemon that would answer, were one started.
    let api = lab.daemon(1, needs_login());

    let refused = connect_scripted(&lab.app, &admin, LINK).await;
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
    let (team, owner) = team_with_owner(&lab.app, "Research").await;
    let theirs = lab.row(team.id, owner.id, "theirs").await;
    let _theirs_api = lab.daemon(theirs.id, running_status());
    lab.tailnets.ensure_running(theirs.id).await.unwrap();
    let _new_api = lab.daemon(theirs.id + 1, needs_login());

    let (deleted, created) = tokio::join!(
        delete_team(&lab.app, &owner, team.id),
        connect(&lab.app, &root_owner, LINK).send()
    );
    assert_redirect(&deleted, "/");
    assert_redirect(&created, "/tailnets");

    // The tailnet of the deleted team is gone for good, the new one of the
    // other team is alive.
    assert!(lab.tailnets.handle(theirs.id).is_none());
    assert!(!lab.tailnet_dir(theirs.id).exists());
    assert!(matches!(
        lab.tailnets.ensure_running(theirs.id).await,
        Err(TailnetError::Removed(_))
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
        Err(TailnetError::Removed(1))
    ));

    // The backup brings back a tailnet with an id this process removed.
    lab.app.state.database_replaced().await.unwrap();
    let _api = lab.daemon(1, running_status());
    lab.tailnets.ensure_running(1).await.unwrap();
    assert!(lab.tailnets.handle(1).is_some());
}

// --- which control servers the operator allows ---

#[tokio::test]
async fn offers_the_control_server_only_when_the_operator_allows_other_servers() {
    async fn dialog(control: Lab) -> String {
        let admin = create_admin(&control.app).await;
        get(&control.app, &admin, "/tailnets/new").await
    }

    // Unset: Tailscale's own, and no field at all.
    let page = dialog(Lab::with_control(TailnetControl::Official).await).await;
    assert!(page.contains("id=\"connect-tailnet-name\""));
    assert!(!page.contains("name=\"controlUrl\""), "{page}");
    assert!(!page.contains("Control server"));

    // `any`: a field to type the address in.
    let page = dialog(Lab::new().await).await;
    assert!(
        page.contains("<input class=\"input\" id=\"connect-tailnet-control\" name=\"controlUrl\"")
    );

    // A list: a select, Tailscale first, with the origins and nothing else.
    let page = dialog(Lab::allowing("https://hs.example.com,http://10.0.0.2:8080").await).await;
    let select = between(
        &page,
        "<select class=\"input\" id=\"connect-tailnet-control\"",
        "</select>",
    );
    assert!(select.contains("name=\"controlUrl\""));
    for option in [
        "<option value=\"\" selected>Tailscale</option>",
        "<option value=\"https://hs.example.com\">https://hs.example.com</option>",
        "<option value=\"http://10.0.0.2:8080\">http://10.0.0.2:8080</option>",
    ] {
        assert!(select.contains(option), "{select}");
    }
    assert_eq!(select.matches("<option").count(), 3);
}

#[tokio::test]
async fn refuses_a_control_server_the_operator_does_not_allow() {
    let lab = Lab::allowing("https://hs.example.com").await;
    let admin = create_admin(&lab.app).await;
    let with_control = |name, url| [("name", name), ("method", "link"), ("controlUrl", url)];

    for url in [
        "https://evil.example.com",
        "http://hs.example.com",
        "http://127.0.0.1:8080",
        "http://169.254.169.254",
    ] {
        let refused = connect_scripted(&lab.app, &admin, &with_control("pro", url)).await;
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
    let allowed = connect(&lab.app, &admin, &with_control("hs", HEADSCALE))
        .send()
        .await;
    assert_redirect(&allowed, "/tailnets");
    assert_joined(&api, "mymcps-hs", HEADSCALE, None);
    let _api = lab.daemon(2, needs_login());
    let official = connect(&lab.app, &admin, &[("name", "ts"), ("method", "link")]);
    assert_redirect(&official.send().await, "/tailnets");
    assert_eq!(tailnets_of(&lab.app, Team::ROOT_ID).await.len(), 2);

    // Only Tailscale's own: the field is not there, and a post that names one is refused.
    let strict = Lab::with_control(TailnetControl::Official).await;
    let admin = create_admin(&strict.app).await;
    let form = with_control("pro", "https://hs.example.com");
    let refused = connect_scripted(&strict.app, &admin, &form).await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    let text = refused.text();
    assert!(
        text.contains("This tailnet could not be connected"),
        "{text}"
    );
    assert!(text.contains("TAILNET_CONTROL_URLS"), "{text}");
    assert!(tailnets_of(&strict.app, Team::ROOT_ID).await.is_empty());
    let plain = connect(&strict.app, &admin, &form).send().await;
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
    set_control(&lab.app, pro.id, HEADSCALE).await;
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
        Ok(TailnetState::Failed(message)) if message.contains("TAILNET_CONTROL_URLS")
    ));
    assert!(lab.tailnets.handle(pro.id).is_none());

    // Allowed again (the operator restarts the server with the variable),
    // it starts as before.
    let allowed = Lab::allowing("https://hs.example.com").await;
    let admin = create_admin(&allowed.app).await;
    let hs = allowed.row(Team::ROOT_ID, admin.id, "hs").await;
    set_control(&allowed.app, hs.id, HEADSCALE).await;
    let _api = allowed.daemon(hs.id, needs_login());
    get(&allowed.app, &admin, "/tailnets").await;
    assert!(allowed.tailnets.handle(hs.id).is_some());
}

// --- deleting ---

#[tokio::test]
async fn refuses_to_delete_a_tailnet_that_mcps_use_and_names_them() {
    let (lab, admin) = Lab::with_admin().await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let api = lab.daemon(pro.id, running_status());
    let mut users = Vec::new();
    for name in ["Grafana", "Home"] {
        users.push(
            create_mcp(&lab.app, admin.id, |mcp| {
                mcp.name = name.into();
                mcp.slug = name.to_lowercase();
                mcp.tailnet_id = Some(pro.id);
            })
            .await,
        );
    }

    let refused = delete_tailnet(&lab.app, &admin, pro.id).await;
    assert_refused(
        &refused,
        "pro is used by Grafana and Home. Move them to another network first.",
    );
    assert_eq!(tailnets_of(&lab.app, Team::ROOT_ID).await.len(), 1);
    assert_eq!(logouts(&api), 0);

    // Once they use another network, the tailnet goes: the node leaves it,
    // its daemon stops, its directory is gone.
    lab.tailnets.ensure_running(pro.id).await.unwrap();
    sqlx::query("update `mcps` set `tailnet_id` = null where `id` = ?")
        .bind(users[0].id)
        .execute(&*lab.app.core.db)
        .await
        .unwrap();
    let still = delete_tailnet(&lab.app, &admin, pro.id).await;
    assert_refused(
        &still,
        "pro is used by Home. Move it to another network first.",
    );
    sqlx::query("update `mcps` set `tailnet_id` = null")
        .execute(&*lab.app.core.db)
        .await
        .unwrap();

    let deleted = delete_tailnet(&lab.app, &admin, pro.id).await;
    assert_done(&deleted, "/tailnets", "Tailnet deleted");
    assert!(tailnets_of(&lab.app, Team::ROOT_ID).await.is_empty());
    assert_eq!(logouts(&api), 1);
    assert!(!lab.tailnet_dir(pro.id).exists());
    // The MCPs are still there, on the internet.
    assert_eq!(count_mcps(&lab.app).await, 2);
}

#[tokio::test]
async fn deleting_a_tailnet_whose_control_server_is_no_longer_allowed_starts_no_daemon_nor_logout()
{
    // Joined to a Headscale the operator allowed, which the operator no
    // longer does: the node cannot be logged out, so no daemon is asked to.
    // The last case is the control server that is still allowed.
    for (control, running, logs_out) in [
        (TailnetControl::Official, false, false),
        (TailnetControl::Official, true, false),
        (
            TailnetControl::parse(Some("https://hs.example.com")).unwrap(),
            false,
            true,
        ),
    ] {
        let lab = Lab::with_control(control).await;
        let admin = create_admin(&lab.app).await;
        let (pro, api) = lab.joined_row(Team::ROOT_ID, admin.id, "pro").await;
        if running {
            // A daemon that was already running (started before the setting changed).
            lab.tailnets.ensure_running(pro.id).await.unwrap();
            assert!(lab.tailnets.handle(pro.id).is_some());
        }
        let runs = lab.runs();

        let deleted = delete_tailnet(&lab.app, &admin, pro.id).await;

        assert_done(&deleted, "/tailnets", "Tailnet deleted");
        assert_eq!(logouts(&api), usize::from(logs_out), "{running}");
        if !logs_out {
            assert_eq!(lab.runs(), runs, "no daemon was started to log out");
        }
        if !running && !logs_out {
            assert!(api.requests().is_empty());
        }
        lab.assert_removed(pro.id).await;
    }
}

// --- signing in again ---

#[tokio::test]
async fn signs_a_tailnet_in_again_whatever_it_holds_and_keeps_its_headscale() {
    let (lab, admin) = Lab::with_admin().await;
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
    assert_done(
        &response,
        "/tailnets",
        "Use the Sign in link of the tailnet to finish",
    );
    // The node was asked, with its own name and no key, for a new login.
    assert_joined(&api, "mymcps-pro", "", None);
    assert_eq!(logouts(&api), 0);

    // The daemon then has a link: the row offers it, and still the way back,
    // since a link that is shown may be one that expired.
    api.set_status(needs_login());
    let page = get(&lab.app, &admin, "/tailnets").await;
    let row = table_rows(&page)[0];
    assert!(row.contains(&format!("href=\"{LOGIN_URL}\" target=\"_blank\"")));
    assert!(row.contains("Sign in again</button>"));
    assert_eq!(tailnets_of(&lab.app, Team::ROOT_ID).await.len(), 1);

    // A tailnet on a Headscale keeps it.
    let hs = lab.row(Team::ROOT_ID, admin.id, "hs").await;
    set_control(&lab.app, hs.id, "https://headscale.example.com/").await;
    let api = lab.daemon(hs.id, status_in("NeedsLogin"));
    let response = sign_in_again(&lab.app, &admin, hs.id).send().await;
    assert_redirect(&response, "/tailnets");
    assert_joined(&api, "mymcps-hs", "https://headscale.example.com/", None);
}

#[tokio::test]
async fn does_not_sign_in_again_a_tailnet_that_is_connected_or_waits_and_reports_a_daemon_that_cannot()
 {
    let (lab, admin) = Lab::with_admin().await;
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
        assert_refused(&refused, "pro does not need to sign in again");
    }
    assert!(api.requests_to(START).is_empty());
    assert!(api.requests_to(LOGIN).is_empty());

    // A daemon that cannot start the login is reported.
    let broken = lab.row(Team::ROOT_ID, admin.id, "broken").await;
    let api = lab.daemon(broken.id, status_in("NeedsLogin"));
    api.force(LOGIN, 500, "control is down");
    let response = sign_in_again(&lab.app, &admin, broken.id).send().await;
    assert_redirect(&response, "/tailnets");
    let message = flash(&response, "error").unwrap();
    assert!(
        message.starts_with("Could not sign in again: "),
        "{message}"
    );
    assert!(flash(&response, "success").is_none());
}

#[tokio::test]
async fn deletes_and_signs_in_again_only_a_tailnet_of_the_team() {
    let (lab, admin) = Lab::with_admin().await;
    let other_team = create_team(&lab.app, "Other").await;
    let theirs = lab.row(other_team.id, admin.id, "theirs").await;
    let api = lab.daemon(theirs.id, status_in("Stopped"));

    for id in [theirs.id.to_string(), "999".into(), "abc".into()] {
        let deleted = delete_tailnet(&lab.app, &admin, &id).await;
        assert_refused(&deleted, "Tailnet not found");
        let signed_in = sign_in_again(&lab.app, &admin, &id).send().await;
        assert_refused(&signed_in, "Tailnet not found");
    }
    assert_eq!(tailnets_of(&lab.app, other_team.id).await.len(), 1);
    assert!(api.requests().is_empty());
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

/// The form of an npm MCP, which a network does not apply to.
fn npm_form<'a>(name: &'a str, tailnet_id: &'a str) -> [(&'a str, &'a str); 5] {
    [
        ("name", name),
        ("transport", "npm"),
        ("npmPackage", "@example/mcp"),
        ("authType", "auto"),
        ("tailnetId", tailnet_id),
    ]
}

fn create_mcp_request<'a>(app: &'a TestApp, user: &User, form: &[(&str, &str)]) -> TestRequest<'a> {
    post_as(app, user, "/mcps").form(form)
}

fn update_mcp_request<'a>(
    app: &'a TestApp,
    user: &User,
    id: i64,
    form: &[(&str, &str)],
) -> TestRequest<'a> {
    post_as(app, user, &format!("/mcps/{id}?_method=PUT")).form(form)
}

#[tokio::test]
async fn offers_the_tailnets_of_the_team_on_an_http_mcp() {
    let (lab, admin) = Lab::with_admin().await;
    let other_team = create_team(&lab.app, "Other").await;
    lab.row(other_team.id, admin.id, "theirs").await;

    // No tailnet in the team: no field.
    let form = get(&lab.app, &admin, "/mcps/new?template=custom").await;
    assert!(!form.contains("tailnetId"));

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
    let (lab, admin) = Lab::with_admin().await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let id = pro.id.to_string();

    let created = create_mcp_request(&lab.app, &admin, &mcp_form(&[("tailnetId", &id)]));
    assert_redirect(&created.send().await, "/mcps");
    let mcp = mcp_by_slug(&lab.app, "grafana").await.unwrap();
    assert_eq!(mcp.tailnet_id, Some(pro.id));

    // Back to the internet with the blank option.
    let updated = update_mcp_request(&lab.app, &admin, mcp.id, &mcp_form(&[("tailnetId", "")]));
    assert_redirect(&updated.send().await, "/mcps");
    assert_eq!(find_mcp(&lab.app, mcp.id).await.tailnet_id, None);

    // An npm MCP is not reached over a network: whatever the form says, it has none.
    let npm = create_mcp_request(&lab.app, &admin, &npm_form("Package", &id));
    assert_redirect(&npm.send().await, "/mcps");
    let package = mcp_by_slug(&lab.app, "package").await.unwrap();
    assert_eq!(package.tailnet_id, None);
    assert_eq!(package.transport, McpTransport::Npm);

    // And one that moves from HTTP to npm forgets it.
    let again = create_mcp_request(
        &lab.app,
        &admin,
        &mcp_form(&[("name", "Second"), ("tailnetId", &id)]),
    );
    assert_redirect(&again.send().await, "/mcps");
    let second = mcp_by_slug(&lab.app, "second").await.unwrap();
    assert_eq!(second.tailnet_id, Some(pro.id));
    let moved = update_mcp_request(&lab.app, &admin, second.id, &npm_form("Second", &id));
    assert_redirect(&moved.send().await, "/mcps");
    assert_eq!(find_mcp(&lab.app, second.id).await.tailnet_id, None);
}

#[tokio::test]
async fn refuses_the_tailnet_of_another_team_or_one_that_does_not_exist() {
    let (lab, admin) = Lab::with_admin().await;
    let other_team = create_team(&lab.app, "Other").await;
    let theirs = lab.row(other_team.id, admin.id, "theirs").await;
    lab.row(Team::ROOT_ID, admin.id, "pro").await;

    for id in [theirs.id.to_string(), "999".to_string()] {
        let form = mcp_form(&[("tailnetId", &id)]);
        let refused = from_script(create_mcp_request(&lab.app, &admin, &form))
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
    let theirs_id = theirs.id.to_string();
    let form = mcp_form(&[("tailnetId", &theirs_id)]);
    let refused = update_mcp_request(&lab.app, &admin, mcp.id, &form)
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
    let form = mcp_form(&[("tailnetId", "pro")]);
    let refused = from_script(create_mcp_request(&lab.app, &admin, &form))
        .send()
        .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(count_mcps(&lab.app).await, 1);
}

// --- what deleting a team or a user takes with it ---

/// The two deletions that take the tailnets of a team: the team, by its
/// owner, and an account, by a super admin, with its personal team.
#[derive(Clone, Copy, Debug)]
enum Deletion {
    Team,
    User,
}

impl Deletion {
    const BOTH: [Self; 2] = [Self::Team, Self::User];

    /// The team the deletion takes, and a team its member shares with `admin`.
    async fn prepare(self, app: &TestApp) -> Doomed {
        let admin = create_super_admin(app).await;
        let (team, member) = match self {
            Self::Team => team_with_owner(app, "Research").await,
            Self::User => {
                let outsider = create_account(app, None).await;
                (personal_team(app, &outsider).await, outsider)
            }
        };
        let shared = create_team(app, "Shared").await;
        join_team(app, shared.id, &member, TeamRole::Admin).await;
        join_team(app, shared.id, &admin, TeamRole::Owner).await;
        Doomed {
            deletion: self,
            team,
            member,
            admin,
            shared,
        }
    }
}

struct Doomed {
    deletion: Deletion,
    team: Team,
    /// The owner of `team`, or the account deleted with it.
    member: User,
    admin: User,
    shared: Team,
}

impl Doomed {
    async fn delete(&self, app: &TestApp) -> TestResponse {
        match self.deletion {
            Deletion::Team => delete_team(app, &self.member, self.team.id).await,
            Deletion::User => delete_user(app, &self.admin, self.member.id).await,
        }
    }

    /// Where the deletion leads, and what it deleted.
    fn done(&self) -> (&'static str, &'static str) {
        match self.deletion {
            Deletion::Team => ("/", "Team"),
            Deletion::User => ("/users", "User"),
        }
    }

    fn assert_done(&self, response: &TestResponse) {
        let (to, what) = self.done();
        assert_done(response, to, &format!("{what} deleted"));
    }
}

#[tokio::test]
async fn stops_the_daemons_of_the_tailnets_a_deletion_takes() {
    for deletion in Deletion::BOTH {
        let lab = Lab::new().await;
        let doomed = deletion.prepare(&lab.app).await;
        let gone = lab.row(doomed.team.id, doomed.member.id, "gone").await;
        let kept = lab.row(doomed.shared.id, doomed.member.id, "kept").await;
        let gone_api = lab.daemon(gone.id, running_status());
        let kept_api = lab.daemon(kept.id, running_status());
        lab.tailnets.ensure_running(gone.id).await.unwrap();
        lab.tailnets.ensure_running(kept.id).await.unwrap();

        doomed.assert_done(&doomed.delete(&lab.app).await);

        // Its tailnet is gone with the team, and its node left the tailnet.
        assert!(!tailnet_exists(&lab.app, gone.id).await, "{deletion:?}");
        assert_eq!(logouts(&gone_api), 1, "{deletion:?}");
        assert!(!lab.tailnet_dir(gone.id).exists(), "{deletion:?}");
        assert!(lab.tailnets.handle(gone.id).is_none(), "{deletion:?}");
        // The shared team's stays, and goes to an owner of that team when the
        // account that connected it is deleted.
        let kept = Tailnet::find(&*lab.app.core.db, kept.id)
            .await
            .unwrap()
            .unwrap();
        let owner = match deletion {
            Deletion::Team => &doomed.member,
            Deletion::User => &doomed.admin,
        };
        assert_eq!(kept.created_by, owner.id, "{deletion:?}");
        assert_eq!(logouts(&kept_api), 0, "{deletion:?}");
        assert!(lab.tailnets.handle(kept.id).is_some(), "{deletion:?}");
    }
}

/// Runs `deletion` while a tailnet is connected to `team_id` in the gap that
/// opened before the deletion took the write lock of the database: a Connect
/// request, which inserts the row and starts its daemon, is in flight when
/// the deletion request arrives, and its insert lands first.
///
/// The ordering is the test's own: it holds the write lock (as that request's
/// transaction does) until the deletion has had every chance to run, inserts
/// the row under it, then lets go. Where the deletion reads the tailnets it
/// takes down does not change what it must do, but a read made before the
/// lock misses this row, so nothing would remove its daemon. Returns the
/// response, the new tailnet and its (fake) LocalAPI.
async fn delete_while_a_tailnet_is_connected(
    lab: &Lab,
    team_id: i64,
    created_by: i64,
    deletion: impl std::future::Future<Output = TestResponse>,
) -> (TestResponse, Tailnet, FakeLocalApi) {
    let mut connecting = lab.app.core.db.begin().await.unwrap();
    let tailnet = Tailnet::create_for_tests(&mut *connecting, team_id, created_by, "late").await;
    // Its daemon is up, as the Connect request starts it right after.
    let api = lab.daemon(tailnet.id, running_status());
    lab.tailnets.ensure_running(tailnet.id).await.unwrap();

    let release = async {
        // The deletion is in flight, waiting for the lock: its connection
        // joins ours, and a pause lets it get as far as it can. Only the
        // regression this test guards against depends on that pause.
        let pool = lab.app.core.db.pool();
        while pool.size() as usize - pool.num_idle() < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        connecting.commit().await.unwrap();
    };
    let (response, ()) = tokio::join!(deletion, release);
    (response, tailnet, api)
}

#[tokio::test]
async fn removes_the_daemon_of_a_tailnet_connected_just_before_the_deletion() {
    for deletion in Deletion::BOTH {
        let lab = Lab::new().await;
        let doomed = deletion.prepare(&lab.app).await;

        let (deleted, late, api) = delete_while_a_tailnet_is_connected(
            &lab,
            doomed.team.id,
            doomed.member.id,
            doomed.delete(&lab.app),
        )
        .await;

        doomed.assert_done(&deleted);
        assert!(!tailnet_exists(&lab.app, late.id).await, "{deletion:?}");
        assert_eq!(logouts(&api), 1, "{deletion:?}: the node left its tailnet");
        assert!(lab.tailnets.handle(late.id).is_none(), "{deletion:?}");
        assert!(!lab.tailnet_dir(late.id).exists(), "{deletion:?}");
    }
}

#[tokio::test]
async fn a_deletion_starts_no_daemon_for_a_control_server_that_is_no_longer_allowed() {
    for deletion in Deletion::BOTH {
        let lab = Lab::with_control(TailnetControl::Official).await;
        let doomed = deletion.prepare(&lab.app).await;
        let (team, member) = (doomed.team.id, doomed.member.id);
        let (forbidden, forbidden_api) = lab.joined_row(team, member, "forbidden").await;
        // Tailscale's own control server is always allowed, and still logs out.
        let own = lab.row(team, member, "own").await;
        let own_api = lab.daemon(own.id, running_status());
        std::fs::write(lab.tailnet_dir(own.id).join("tailscaled.state"), "{}").unwrap();

        doomed.assert_done(&doomed.delete(&lab.app).await);

        assert!(
            !tailnet_exists(&lab.app, forbidden.id).await,
            "{deletion:?}"
        );
        assert!(forbidden_api.requests().is_empty(), "{deletion:?}");
        assert!(!lab.tailnet_dir(forbidden.id).exists(), "{deletion:?}");
        assert_eq!(logouts(&own_api), 1, "{deletion:?}");
        assert!(!lab.tailnet_dir(own.id).exists(), "{deletion:?}");

        // A control server that is allowed still has its nodes logged out.
        let lab = Lab::allowing("https://hs.example.com").await;
        let doomed = deletion.prepare(&lab.app).await;
        let (hs, api) = lab.joined_row(doomed.team.id, doomed.member.id, "hs").await;

        doomed.assert_done(&doomed.delete(&lab.app).await);

        assert_eq!(logouts(&api), 1, "{deletion:?}");
        assert!(!lab.tailnet_dir(hs.id).exists(), "{deletion:?}");
    }
}

// --- deleting while Tailscale is off ---

/// A server where Tailscale is off (`TAILSCALED_BINARY=off`), with tailnets
/// that were joined before it was turned off: their rows and the state of
/// their nodes.
struct Off {
    app: TestApp,
}

impl Off {
    async fn new() -> Self {
        Self {
            app: TestApp::with_upstream(|upstream| upstream.tailnets(None)).await,
        }
    }

    fn state_dir(&self, id: i64) -> PathBuf {
        self.app
            .core
            .config
            .data_dir
            .join("tailnets")
            .join(id.to_string())
    }

    /// A tailnet row, and the node key of the device it joined.
    async fn joined(&self, team_id: i64, created_by: i64, name: &str) -> Tailnet {
        let tailnet =
            Tailnet::create_for_tests(&*self.app.core.db, team_id, created_by, name).await;
        let state = self.state_dir(tailnet.id);
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("tailscaled.state"), "{\"node-key\":1}").unwrap();
        tailnet
    }
}

/// `response` flashes that `what` was deleted and that the devices stay.
fn assert_device_stays(response: &TestResponse, to: &str, what: &str) -> String {
    assert_redirect(response, to);
    let message = flash(response, "success").unwrap();
    assert!(
        message.starts_with(&format!("{what} deleted.")),
        "{message}"
    );
    assert!(message.contains("Tailscale is off"), "{message}");
    assert!(message.contains("could not be logged out"), "{message}");
    assert!(message.contains("admin console"), "{message}");
    message
}

#[tokio::test]
async fn deleting_a_tailnet_while_tailscale_is_off_deletes_its_state_and_says_the_device_stays_only_if_one_joined()
 {
    let off = Off::new().await;
    let admin = create_admin(&off.app).await;
    let pro = off.joined(Team::ROOT_ID, admin.id, "pro").await;
    let kept = off.joined(Team::ROOT_ID, admin.id, "kept").await;
    assert!(off.state_dir(pro.id).join("tailscaled.state").exists());

    let deleted = delete_tailnet(&off.app, &admin, pro.id).await;

    assert_device_stays(&deleted, "/tailnets", "Tailnet");
    assert!(!off.state_dir(pro.id).exists(), "the node key is gone");
    assert!(!tailnet_exists(&off.app, pro.id).await);
    // Another tailnet's state is not touched.
    assert!(off.state_dir(kept.id).join("tailscaled.state").exists());

    // A tailnet that no device joined here (restored from a backup) says
    // nothing of a logout.
    let restored =
        Tailnet::create_for_tests(&*off.app.core.db, Team::ROOT_ID, admin.id, "restored").await;
    assert!(!off.state_dir(restored.id).exists());
    let deleted = delete_tailnet(&off.app, &admin, restored.id).await;
    assert_eq!(
        flash(&deleted, "success").as_deref(),
        Some("Tailnet deleted")
    );
}

#[tokio::test]
async fn a_deletion_while_tailscale_is_off_deletes_the_state_of_the_tailnets_it_takes() {
    for deletion in Deletion::BOTH {
        let off = Off::new().await;
        let doomed = deletion.prepare(&off.app).await;
        let (team, member) = (doomed.team.id, doomed.member.id);
        let first = off.joined(team, member, "first").await;
        let second = off.joined(team, member, "second").await;
        // The shared team keeps its tailnet, and so its state.
        let kept = off.joined(doomed.shared.id, member, "kept").await;

        let deleted = doomed.delete(&off.app).await;

        let (to, what) = doomed.done();
        let message = assert_device_stays(&deleted, to, what);
        assert!(message.contains("2 tailnets"), "{message}");
        assert!(!off.state_dir(first.id).exists(), "{deletion:?}");
        assert!(!off.state_dir(second.id).exists(), "{deletion:?}");
        assert!(off.state_dir(kept.id).exists(), "{deletion:?}");
        assert!(tailnet_exists(&off.app, kept.id).await, "{deletion:?}");
    }
}

/// An HTTP MCP that signs in with `auth_type`, and holds a bearer token, a
/// header value and an OAuth connection, on `network`.
async fn mcp_with_secrets(
    lab: &Lab,
    user: &User,
    name: &str,
    auth_type: McpAuthType,
    network: Option<i64>,
) -> Mcp {
    create_mcp(&lab.app, user.id, |mcp| {
        mcp.name = name.into();
        mcp.slug = name.to_lowercase();
        mcp.http_url = Some("https://grafana.tail1234.ts.net/mcp".into());
        mcp.tailnet_id = network;
        mcp.auth_type = auth_type;
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

/// Saves the MCP again with the same URL and the sign-in `auth_type`, on `network`.
async fn save_with(lab: &Lab, user: &User, mcp: &Mcp, auth_type: &str, network: Option<i64>) {
    let id = network.map(|id| id.to_string()).unwrap_or_default();
    let form = [
        ("name", mcp.name.as_str()),
        ("transport", "http"),
        ("httpUrl", "https://grafana.tail1234.ts.net/mcp"),
        ("authType", auth_type),
        ("tailnetId", id.as_str()),
        ("enabled", "on"),
    ];
    let response = update_mcp_request(&lab.app, user, mcp.id, &form)
        .send()
        .await;
    assert_redirect(&response, "/mcps");
}

#[tokio::test]
async fn forgets_the_secrets_and_the_sign_in_of_an_mcp_only_when_its_network_changes() {
    let (lab, admin) = Lab::with_admin().await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let home = lab.row(Team::ROOT_ID, admin.id, "home").await;

    // The same URL is another machine on another network.
    for (auth_type, field) in [(McpAuthType::Bearer, "bearer"), (McpAuthType::Auto, "auto")] {
        for (from, to) in [
            (None, Some(pro.id)),
            (Some(pro.id), None),
            (Some(pro.id), Some(home.id)),
            (Some(pro.id), Some(pro.id)),
            (None, None),
        ] {
            let name = format!("{field} {}-{}", from.unwrap_or(0), to.unwrap_or(0));
            let mcp = mcp_with_secrets(&lab, &admin, &name, auth_type, from).await;

            save_with(&lab, &admin, &mcp, field, to).await;

            let saved = find_mcp(&lab.app, mcp.id).await;
            assert_eq!(saved.tailnet_id, to, "{name}");
            // The secret of the auth type stays on its network; what the auth
            // type does not use never stays.
            let kept = |of: McpAuthType, value: &Option<String>| {
                value.clone().filter(|_| from == to && auth_type == of)
            };
            assert_eq!(
                saved.auth_bearer,
                kept(McpAuthType::Bearer, &mcp.auth_bearer),
                "{name}"
            );
            assert_eq!(saved.auth_header_value, None, "{name}");
            for (saved, before) in [
                (&saved.oauth_access_token, &mcp.oauth_access_token),
                (&saved.oauth_refresh_token, &mcp.oauth_refresh_token),
                (&saved.oauth_client_id, &mcp.oauth_client_id),
                (&saved.oauth_client_secret, &mcp.oauth_client_secret),
                (&saved.oauth_token_url, &mcp.oauth_token_url),
                (&saved.oauth_authorize_url, &mcp.oauth_authorize_url),
            ] {
                assert_eq!(*saved, kept(McpAuthType::Auto, before), "{name}");
            }
        }
    }
}

#[tokio::test]
async fn the_form_stops_offering_to_keep_the_saved_secrets_for_another_network() {
    let (lab, admin) = Lab::with_admin().await;
    let pro = lab.row(Team::ROOT_ID, admin.id, "pro").await;
    let internet = mcp_with_secrets(&lab, &admin, "Online", McpAuthType::Bearer, None).await;
    let tailnet = mcp_with_secrets(&lab, &admin, "Inside", McpAuthType::Bearer, Some(pro.id)).await;

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
