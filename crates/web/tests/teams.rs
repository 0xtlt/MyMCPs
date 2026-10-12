//! Teams, roles and registration, as Coolify has them: what belongs to a
//! team is only shown to its members, and what someone may do there follows
//! their role. Unlike Coolify, no team runs the instance: its super admins
//! do, and that is said of their account. `invites.rs` has the invites of
//! the Root Team.

mod mcps_support;
mod support;

use http::StatusCode;
use mcps_support::app_with_mcp_servers;
use mymcps_core::models::{
    AccessToken, InstanceSetting, Invite, Mcp, ScopeMode, Team, TeamRole, User,
};
use mymcps_gateway::access_token::{self, NewAccessToken};
use mymcps_web::auth::REMEMBER_COOKIE;
use mymcps_web::testing::factories::{
    PASSWORD, create_account, create_admin, create_invite, create_mcp, create_member, create_owner,
    create_super_admin, create_team, create_user, join_team, make_super_admin,
};
use mymcps_web::testing::{TestApp, TestRequest, TestResponse};
use serde_json::json;
use support::*;

const ADMIN_ONLY: &str = "Admin access required";
const SUPER_ADMIN_ONLY: &str = "Super admin access required";

/// What the instance form posts besides the switch of registration.
const INSTANCE_FIELDS: [(&str, &str); 3] = [
    ("gatewayToolMode", "eager"),
    ("mcpLogLevel", "metadata"),
    ("mcpLogRetentionDays", "14"),
];

fn assert_redirect(response: &TestResponse, path: &str) {
    assert_eq!(response.status, StatusCode::FOUND, "{}", response.text());
    assert_eq!(response.redirect_path().as_deref(), Some(path));
}

fn flash(response: &TestResponse, key: &str) -> Option<String> {
    response
        .flashed(key)
        .and_then(|value| value.as_str().map(str::to_string))
}

/// Send the request as the page's script sends an async form.
fn from_script(request: TestRequest<'_>) -> TestRequest<'_> {
    request
        .header("x-requested-with", "fetch")
        .header("accept", "text/html")
}

async fn count(app: &TestApp, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(&*app.core.db)
        .await
        .unwrap()
}

async fn role_of(app: &TestApp, team_id: i64, user: &User) -> Option<TeamRole> {
    Team::role_of(&*app.core.db, team_id, user.id)
        .await
        .unwrap()
}

async fn registration_open(app: &TestApp) -> bool {
    InstanceSetting::registration_enabled(&*app.core.db)
        .await
        .unwrap()
}

/// Save the instance settings as this person, with registration on or off.
async fn set_registration(app: &TestApp, user: &User, open: bool) -> TestResponse {
    let mut fields = INSTANCE_FIELDS.to_vec();
    // A switch submits "on" when checked, and nothing when unchecked.
    if open {
        fields.push(("registrationEnabled", "on"));
    }
    app.post("/settings/mcp-logging?_method=PATCH")
        .login_as(user)
        .csrf()
        .form(&fields)
        .send()
        .await
}

async fn sign_up(app: &TestApp, name: &str, email: &str) -> TestResponse {
    app.post("/signup")
        .csrf()
        .form(&[
            ("fullName", name),
            ("email", email),
            ("password", "password123"),
            ("passwordConfirmation", "password123"),
        ])
        .send()
        .await
}

/// A team with an owner of its own, who is in no other team.
async fn team_with_owner(app: &TestApp, name: &str) -> (Team, User) {
    let team = create_team(app, name).await;
    let owner = create_account(app, None).await;
    join_team(app, team.id, &owner, TeamRole::Owner).await;
    (team, owner)
}

async fn token_in(app: &TestApp, team_id: i64, user: &User, name: &str) -> AccessToken {
    access_token::create(
        &app.core.db,
        NewAccessToken {
            name,
            scope_mode: ScopeMode::All,
            mcp_ids: &[],
            expires_at: None,
            created_by: user.id,
            team_id,
        },
    )
    .await
    .unwrap()
    .token
}

async fn find_token(app: &TestApp, id: i64) -> Option<AccessToken> {
    AccessToken::find(&*app.core.db, id).await.unwrap()
}

async fn find_mcp(app: &TestApp, id: i64) -> Option<Mcp> {
    Mcp::find(&*app.core.db, id).await.unwrap()
}

async fn page(app: &TestApp, user: &User, team_id: i64, path: &str) -> String {
    let response = app.get(path).login_as(user).in_team(team_id).send().await;
    assert_eq!(response.status, StatusCode::OK, "{path}");
    response.text()
}

// ------------------------------------------------------------ registration

#[tokio::test]
async fn registration_is_closed_until_someone_who_runs_the_instance_opens_it() {
    let app = TestApp::new().await;
    let owner = create_super_admin(&app).await;
    let member = create_member(&app).await;
    // An owner of a team runs that team, not the instance: of the Root Team
    // no more than of another.
    let root_owner = create_owner(&app).await;
    let (_, outsider) = team_with_owner(&app, "Elsewhere").await;

    assert!(!registration_open(&app).await);
    let login = app.get("/login").send().await.text();
    assert!(login.contains("New users join by invite only."));
    assert!(login.contains("Self-hosted · invite-only"));
    assert!(!login.contains("/signup"));
    assert_redirect(&app.get("/signup").send().await, "/login");
    let refused = sign_up(&app, "Nina New", "nina@example.com").await;
    assert_redirect(&refused, "/login");
    assert_eq!(
        flash(&refused, "error").as_deref(),
        Some("Registration is disabled on this instance")
    );
    assert!(refused.cookie(REMEMBER_COOKIE).is_none());
    assert_eq!(count(&app, "select count(*) from `users`").await, 4);

    // Opening it is for the super admins.
    for user in [&member, &root_owner, &outsider] {
        let response = set_registration(&app, user, true).await;
        assert_redirect(&response, "/");
        assert_eq!(flash(&response, "error").as_deref(), Some(SUPER_ADMIN_ONLY));
        let settings = app.get("/settings").login_as(user).send().await.text();
        assert!(!settings.contains("registrationEnabled"));
        assert!(!settings.contains("My Instance"));
    }
    assert!(!registration_open(&app).await);

    let settings = app.get("/settings").login_as(&owner).send().await.text();
    assert!(settings.contains("<span class=\"setting-row__title\">Allow registration</span>"));
    assert!(settings.contains(
        "<input type=\"checkbox\" class=\"switch\" role=\"switch\" name=\"registrationEnabled\">"
    ));
    let opened = set_registration(&app, &owner, true).await;
    assert_redirect(&opened, "/settings");
    assert_eq!(
        flash(&opened, "success").as_deref(),
        Some("Instance settings updated")
    );
    assert!(registration_open(&app).await);
    let settings = app.get("/settings").login_as(&owner).send().await.text();
    assert!(settings.contains("name=\"registrationEnabled\" checked>"));

    // The screens before sign-in say so, and lead to the sign-up page.
    let login = app.get("/login").send().await.text();
    assert!(
        login.contains("<a class=\"button button--block\" href=\"/signup\">Create an account</a>")
    );
    assert!(!login.contains("invite only"));
    assert!(login.contains("registration open"));
    let form = app.get("/signup").send().await;
    assert_eq!(form.status, StatusCode::OK);
    let form = form.text();
    assert!(form.contains("<form class=\"form\" method=\"post\" action=\"/signup\">"));
    for field in ["fullName", "email", "password", "passwordConfirmation"] {
        assert!(form.contains(&format!("name=\"{field}\"")), "{field}");
    }

    // Saving the settings with the switch off closes it again.
    set_registration(&app, &owner, false).await;
    assert!(!registration_open(&app).await);
    assert_redirect(&app.get("/signup").send().await, "/login");
}

#[tokio::test]
async fn a_new_account_gets_a_team_of_its_own_and_nothing_of_the_others() {
    let app = TestApp::new().await;
    let owner = create_super_admin(&app).await;
    create_mcp(&app, owner.id, |mcp| mcp.name = "Secret MCP".into()).await;
    token_in(&app, Team::ROOT_ID, &owner, "Secret token").await;
    set_registration(&app, &owner, true).await;

    let created = sign_up(&app, " Nina New ", "nina@example.com").await;
    assert_redirect(&created, "/");
    assert_eq!(
        flash(&created, "success").as_deref(),
        Some("Welcome to MyMCPs")
    );
    assert!(created.cookie(REMEMBER_COOKIE).is_some());

    let nina = User::find_by_email(&*app.core.db, "nina@example.com")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(nina.full_name.as_deref(), Some("Nina New"));
    assert!(nina.verify_password("password123").await.unwrap());
    // One team: her own, which she owns. She is not in the Root Team.
    let teams = Team::memberships(&*app.core.db, nina.id).await.unwrap();
    assert_eq!(teams.len(), 1);
    let personal = &teams[0];
    assert_eq!(personal.team.name, "Nina New's Team");
    assert!(personal.team.personal_team && !personal.team.is_root());
    assert_eq!(personal.role, TeamRole::Owner);
    assert!(!nina.is_super_admin);

    // She is signed in, in her team, where she may add MCPs.
    let session = created.session();
    let team = app
        .get("/team")
        .session(session.clone())
        .send()
        .await
        .text();
    assert!(team.contains("<h1 class=\"page-header__title\">Nina New's Team</h1>"));
    assert!(team.contains("You own this team"));
    let mcps = app
        .get("/mcps")
        .session(session.clone())
        .send()
        .await
        .text();
    assert!(mcps.contains("Add MCP"));
    assert!(!mcps.contains("Secret MCP"));
    let tokens = app
        .get("/tokens")
        .session(session.clone())
        .send()
        .await
        .text();
    assert!(!tokens.contains("Secret token"));
    // Nothing of the instance is hers to run.
    for path in ["/users", "/settings/backup"] {
        let request = if path == "/users" {
            app.get(path)
        } else {
            app.post(path).csrf()
        };
        let response = request.session(session.clone()).send().await;
        assert_redirect(&response, "/");
        assert_eq!(flash(&response, "error").as_deref(), Some(SUPER_ADMIN_ONLY));
    }
    let home = app.get("/").session(session.clone()).send().await.text();
    assert!(!home.contains("href=\"/users\""));
    // Someone who is signed in has no use for the sign-up page.
    assert_redirect(&app.get("/signup").session(session).send().await, "/");

    // An address makes one account.
    let again = sign_up(&app, "Someone Else", "nina@example.com").await;
    assert_redirect(&again, "/signup");
    assert_eq!(
        again.flashed("errors"),
        Some(json!({ "email": "The email has already been taken" }))
    );
    let mismatch = app
        .post("/signup")
        .csrf()
        .form(&[
            ("fullName", "Otto"),
            ("email", "otto@example.com"),
            ("password", "password123"),
            ("passwordConfirmation", "something else"),
        ])
        .send()
        .await;
    assert_redirect(&mismatch, "/signup");
    assert!(mismatch.flashed("errors").is_some());
    let shown = app
        .get("/signup")
        .session(mismatch.session())
        .send()
        .await
        .text();
    assert!(shown.contains("value=\"otto@example.com\""));
    assert!(!shown.contains("something else"));
    assert_eq!(count(&app, "select count(*) from `users`").await, 2);
}

#[tokio::test]
async fn limits_the_accounts_one_address_can_ask_for() {
    let app = TestApp::new().await;
    let owner = create_super_admin(&app).await;
    set_registration(&app, &owner, true).await;

    // Counted before anything is checked or hashed.
    for attempt in 0..10 {
        let response = sign_up(&app, "Someone", "not an email").await;
        assert_redirect(&response, "/signup");
        assert!(response.flashed("errors").is_some(), "{attempt}");
    }
    let limited = sign_up(&app, "Real Person", "real@example.com").await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.header("retry-after").is_some());
    assert_eq!(count(&app, "select count(*) from `users`").await, 1);
}

// ------------------------------------------------------- teams of a person

#[tokio::test]
async fn anyone_creates_a_team_and_goes_from_one_of_theirs_to_another() {
    let app = TestApp::new().await;
    create_owner(&app).await;
    let member = create_member(&app).await;
    let (foreign, _) = team_with_owner(&app, "Foreign").await;

    // The switcher of every page: the team they work in, their teams, and
    // the way to a new one.
    let home = app.get("/").login_as(&member).send().await.text();
    assert!(home.contains("aria-label=\"Team: Root Team. Switch team\""));
    assert!(home.contains("<form method=\"post\" action=\"/teams/switch\">"));
    assert!(home.contains("href=\"/teams/new\" data-dialog-open=\"#create-team\""));
    assert!(home.contains("<dialog class=\"dialog dialog--sm\" id=\"create-team\""));
    assert!(!home.contains("Foreign"));

    let created = app
        .post("/teams")
        .login_as(&member)
        .csrf()
        .form(&[("name", " Research "), ("description", "Papers and notes")])
        .send()
        .await;
    assert_redirect(&created, "/team");
    assert_eq!(flash(&created, "success").as_deref(), Some("Team created"));
    let research: Team = sqlx::query_as("select * from `teams` where `name` = 'Research'")
        .fetch_one(&*app.core.db)
        .await
        .unwrap();
    assert_eq!(research.description.as_deref(), Some("Papers and notes"));
    assert!(!research.personal_team);
    // Whoever creates a team owns it, and works in it from then on: in this
    // browser, and wherever they sign in next.
    assert_eq!(
        role_of(&app, research.id, &member).await,
        Some(TeamRole::Owner)
    );
    assert_eq!(
        created.session().get("currentTeam"),
        Some(&json!(research.id))
    );
    let stored = User::find(&*app.core.db, member.id).await.unwrap().unwrap();
    assert_eq!(stored.current_team_id, Some(research.id));
    let elsewhere = app.get("/team").login_as(&stored).send().await.text();
    assert!(elsewhere.contains("<h1 class=\"page-header__title\">Research</h1>"));
    assert!(elsewhere.contains("You own this team"));
    assert!(elsewhere.contains("<span class=\"nav-user__role\">owner</span>"));
    // Both teams are in the switcher, the current one marked.
    assert!(elsewhere.contains(&format!(
        "<input type=\"hidden\" name=\"teamId\" value=\"{}\"><button type=\"submit\" class=\"menu__item\" role=\"menuitemradio\" aria-checked=\"true\">",
        research.id
    )));
    assert!(elsewhere.contains(
        "<input type=\"hidden\" name=\"teamId\" value=\"1\"><button type=\"submit\" class=\"menu__item\" role=\"menuitemradio\" aria-checked=\"false\">"
    ));

    // Back to the Root Team, where they are a member.
    let switched = app
        .post("/teams/switch")
        .session(created.session())
        .csrf()
        .form(&[("teamId", "1")])
        .send()
        .await;
    assert_redirect(&switched, "/");
    assert_eq!(switched.session().get("currentTeam"), Some(&json!(1)));
    let root = app
        .get("/team")
        .session(switched.session())
        .send()
        .await
        .text();
    assert!(root.contains("<h1 class=\"page-header__title\">Root Team</h1>"));
    assert!(root.contains("<span class=\"nav-user__role\">member</span>"));

    // A team they are not in is answered like no team, and changes nothing.
    for team_id in [foreign.id.to_string(), "999999".into(), "abc".into()] {
        let refused = app
            .post("/teams/switch")
            .session(switched.session())
            .csrf()
            .form(&[("teamId", &team_id)])
            .send()
            .await;
        assert_redirect(&refused, "/");
        assert_eq!(flash(&refused, "error").as_deref(), Some("Team not found"));
        assert_eq!(refused.session().get("currentTeam"), Some(&json!(1)));
    }
    // Nor does a session that names such a team work in it.
    let forged = app
        .get("/team")
        .login_as(&member)
        .in_team(foreign.id)
        .send()
        .await
        .text();
    assert!(!forged.contains("Foreign"));
}

#[tokio::test]
async fn sends_the_create_team_form_back_with_what_is_wrong() {
    let app = TestApp::new().await;
    let member = create_member(&app).await;

    let refused = from_script(app.post("/teams"))
        .login_as(&member)
        .csrf()
        .form(&[("name", " ")])
        .send()
        .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    let form = refused.text();
    assert!(form.starts_with("<form method=\"post\" action=\"/teams\" data-async>"));
    assert!(form.contains("aria-invalid=\"true\" aria-describedby=\"create-team-name-error\""));

    // Without the script, the page behind the link of the switcher shows
    // the dialog open on what was refused.
    let plain = app
        .post("/teams")
        .login_as(&member)
        .csrf()
        .form(&[("name", ""), ("description", "Kept")])
        .send()
        .await;
    assert_redirect(&plain, "/teams/new");
    let page = app
        .get("/teams/new")
        .session(plain.session())
        .send()
        .await
        .text();
    assert!(page.contains("id=\"create-team\" aria-labelledby=\"create-team-title\" data-dialog-reset data-open data-dialog-return=\"/team\">"));
    assert!(page.contains("id=\"create-team-name-error\">"));
    assert!(page.contains("value=\"Kept\""));
    assert_eq!(page.matches("id=\"create-team\"").count(), 1);
    assert_eq!(count(&app, "select count(*) from `teams`").await, 1);
}

// --------------------------------------------------------------- isolation

#[tokio::test]
async fn what_a_team_has_is_only_shown_to_its_members() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let ours = create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Root MCP".into();
        mcp.slug = "root-mcp".into();
    })
    .await;
    let token = token_in(&app, Team::ROOT_ID, &admin, "Root token").await;
    let log = create_mcp_call_log(&app, &token, Some(&ours), |log| {
        log.requested_tool_name = "root-mcp__secret_tool".into();
    })
    .await;
    create_invite(&app, admin.id, |invite| {
        invite.email = "root-invite@example.com".into()
    })
    .await;

    // The owner of another team: every page of theirs, and none of it.
    let (team, outsider) = team_with_owner(&app, "Elsewhere").await;
    for path in [
        "/",
        "/mcps",
        "/tokens",
        "/logs",
        "/analytics",
        "/team",
        "/approvals",
    ] {
        let page = page(&app, &outsider, team.id, path).await;
        for hidden in [
            "Root MCP",
            "root-mcp",
            "Root token",
            "secret_tool",
            "root-invite@example.com",
            "Root Team",
        ] {
            assert!(!page.contains(hidden), "{path}: {hidden}");
        }
    }
    let details = app
        .get(&format!("/logs?range=all&logId={}", log.id))
        .login_as(&outsider)
        .send()
        .await
        .text();
    assert!(!details.contains("secret_tool"));

    // An MCP of another team is answered like one that does not exist,
    // whatever is asked of it, and nothing of it changes.
    let id = ours.id;
    let attempts: Vec<(&str, String)> = vec![
        ("get", format!("/mcps/{id}")),
        ("get", format!("/mcps/{id}/edit")),
        ("put", format!("/mcps/{id}")),
        ("delete", format!("/mcps/{id}")),
        ("post", format!("/mcps/{id}/toggle")),
        ("post", format!("/mcps/{id}/probe")),
        ("post", format!("/mcps/{id}/update")),
        ("get", format!("/mcps/{id}/oauth/start")),
        ("get", format!("/mcps/{id}/tools")),
        ("put", format!("/mcps/{id}/tools")),
    ];
    for (method, path) in attempts {
        let request = match method {
            "get" => app.get(&path),
            "post" => app.post(&path),
            "put" => app.put(&path).form(&[("name", "Taken over")]),
            _ => app.delete(&path),
        };
        let response = request.login_as(&outsider).csrf().send().await;
        assert_redirect(&response, "/mcps");
        assert_eq!(
            flash(&response, "error").as_deref(),
            Some("MCP not found"),
            "{method} {path}"
        );
    }
    let kept = find_mcp(&app, ours.id).await.unwrap();
    assert_eq!((kept.name.as_str(), kept.enabled), ("Root MCP", true));

    // Nor is a token of another team theirs to change, or to see.
    for (method, path) in [
        ("get", format!("/tokens/{}/edit", token.id)),
        ("put", format!("/tokens/{}", token.id)),
        ("post", format!("/tokens/{}/revoke", token.id)),
    ] {
        let request = match method {
            "get" => app.get(&path),
            "put" => app
                .put(&path)
                .form(&[("name", "Mine now"), ("scopeMode", "all")]),
            _ => app.post(&path),
        };
        let response = request.login_as(&outsider).csrf().send().await;
        assert_redirect(&response, "/tokens");
        assert_eq!(
            flash(&response, "error").as_deref(),
            Some("Token not found"),
            "{method} {path}"
        );
    }
    let deleted = app
        .delete("/tokens")
        .login_as(&outsider)
        .csrf()
        .form(&[("ids[]", &token.id.to_string())])
        .send()
        .await;
    assert!(deleted.flashed("error").is_some());
    let kept = find_token(&app, token.id).await.unwrap();
    assert!(!kept.is_revoked() && kept.name == "Root token");

    // A token is made in the team its maker works in, and is never limited
    // to the MCP of another team.
    let limited = app
        .post("/tokens")
        .login_as(&outsider)
        .csrf()
        .form(&[
            ("name", "Reaching out"),
            ("scopeMode", "selected"),
            ("mcpIds[]", &ours.id.to_string()),
        ])
        .send()
        .await;
    assert_eq!(
        flash(&limited, "error").as_deref(),
        Some("One or more selected MCPs do not exist")
    );
    let created = app
        .post("/tokens")
        .login_as(&outsider)
        .csrf()
        .form(&[("name", "Elsewhere token"), ("scopeMode", "all")])
        .send()
        .await;
    assert_eq!(
        flash(&created, "success").as_deref(),
        Some("Access token created — copy it now, it will not be shown again")
    );
    let elsewhere: AccessToken =
        sqlx::query_as("select * from `access_tokens` where `name` = 'Elsewhere token'")
            .fetch_one(&*app.core.db)
            .await
            .unwrap();
    assert_eq!(
        (elsewhere.team_id, elsewhere.created_by),
        (team.id, outsider.id)
    );
    assert!(
        access_token::resolve_allowed_mcps(&app.core.db, &elsewhere)
            .await
            .unwrap()
            .is_empty(),
        "it reaches no MCP of the Root Team"
    );

    // And the Root Team sees nothing of theirs.
    let theirs = create_mcp(&app, outsider.id, |mcp| {
        mcp.name = "Elsewhere MCP".into();
        mcp.team_id = team.id;
    })
    .await;
    for path in ["/mcps", "/tokens", "/"] {
        let page = page(&app, &admin, Team::ROOT_ID, path).await;
        assert!(!page.contains("Elsewhere"), "{path}");
    }
    let response = app
        .post(&format!("/mcps/{}/toggle", theirs.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_eq!(flash(&response, "error").as_deref(), Some("MCP not found"));
}

#[tokio::test]
async fn someone_in_two_teams_sees_each_in_turn_and_a_slug_is_taken_in_its_team_only() {
    let (app, _calls) = app_with_mcp_servers().await;
    let admin = create_admin(&app).await;
    let research = create_team(&app, "Research").await;
    join_team(&app, research.id, &admin, TeamRole::Owner).await;
    create_mcp(&app, admin.id, |mcp| {
        mcp.name = "Notes".into();
        mcp.slug = "notes".into();
    })
    .await;

    // The same name in the other team gets the same slug: no "-2".
    let created = app
        .post("/mcps")
        .login_as(&admin)
        .in_team(research.id)
        .csrf()
        .form(&[
            ("name", "Notes"),
            ("description", "The notes of research"),
            ("transport", "http"),
            ("httpUrl", "https://notes.example/mcp"),
            ("authType", "auto"),
        ])
        .send()
        .await;
    assert_redirect(&created, "/mcps");
    assert_eq!(flash(&created, "success").as_deref(), Some("MCP created"));
    let mcps: Vec<(i64, String)> =
        sqlx::query_as("select `team_id`, `slug` from `mcps` order by `team_id`")
            .fetch_all(&*app.core.db)
            .await
            .unwrap();
    assert_eq!(
        mcps,
        [
            (Team::ROOT_ID, "notes".to_string()),
            (research.id, "notes".to_string())
        ]
    );
    // In one team it is taken.
    let again = app
        .post("/mcps")
        .login_as(&admin)
        .in_team(research.id)
        .csrf()
        .form(&[
            ("name", "Notes"),
            ("description", ""),
            ("transport", "http"),
            ("httpUrl", "https://other.example/mcp"),
            ("authType", "auto"),
        ])
        .send()
        .await;
    assert_eq!(flash(&again, "success").as_deref(), Some("MCP created"));
    assert_eq!(
        count(
            &app,
            "select count(*) from `mcps` where `slug` = 'notes-2' and `team_id` <> 1"
        )
        .await,
        1
    );

    let root = page(&app, &admin, Team::ROOT_ID, "/mcps").await;
    assert!(!root.contains("The notes of research") && !root.contains("notes-2"));
    let other = page(&app, &admin, research.id, "/mcps").await;
    assert!(other.contains("notes-2"));
    assert_eq!(
        other
            .matches("<span class=\"cell-title\">Notes</span>")
            .count(),
        2
    );
}

// ------------------------------------------------------------------- roles

#[tokio::test]
async fn a_member_makes_and_manages_the_access_tokens_they_made_and_no_others() {
    let app = TestApp::new().await;
    let admin = create_admin(&app).await;
    let member = create_member(&app).await;
    let other = create_member(&app).await;
    let of_admin = token_in(&app, Team::ROOT_ID, &admin, "Admin token").await;
    let of_other = token_in(&app, Team::ROOT_ID, &other, "Other token").await;

    let created = app
        .post("/tokens")
        .login_as(&member)
        .csrf()
        .form(&[("name", "My agent"), ("scopeMode", "all")])
        .send()
        .await;
    assert_redirect(&created, "/tokens");
    let mine: AccessToken =
        sqlx::query_as("select * from `access_tokens` where `name` = 'My agent'")
            .fetch_one(&*app.core.db)
            .await
            .unwrap();
    assert_eq!((mine.team_id, mine.created_by), (Team::ROOT_ID, member.id));

    let listed = page(&app, &member, Team::ROOT_ID, "/tokens").await;
    assert!(listed.contains("My agent"));
    assert!(!listed.contains("Admin token") && !listed.contains("Other token"));
    for token in [&of_admin, &of_other] {
        let response = app
            .post(&format!("/tokens/{}/revoke", token.id))
            .login_as(&member)
            .csrf()
            .send()
            .await;
        assert_eq!(
            flash(&response, "error").as_deref(),
            Some("Token not found")
        );
        assert!(!find_token(&app, token.id).await.unwrap().is_revoked());
    }
    let revoked = app
        .post(&format!("/tokens/{}/revoke", mine.id))
        .login_as(&member)
        .csrf()
        .send()
        .await;
    assert_eq!(flash(&revoked, "success").as_deref(), Some("Token revoked"));

    // An administrator of the team sees and revokes every token of it.
    let listed = page(&app, &admin, Team::ROOT_ID, "/tokens").await;
    for name in ["My agent", "Admin token", "Other token"] {
        assert!(listed.contains(name), "{name}");
    }
    let revoked = app
        .post(&format!("/tokens/{}/revoke", of_other.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_eq!(flash(&revoked, "success").as_deref(), Some("Token revoked"));
}

#[tokio::test]
async fn nobody_changes_someone_ranked_above_them_nor_gives_a_role_above_their_own() {
    let app = TestApp::new().await;
    let owner = create_owner(&app).await;
    let second_owner = create_owner(&app).await;
    let admin = create_admin(&app).await;
    let member = create_member(&app).await;
    let (_, outsider) = team_with_owner(&app, "Elsewhere").await;

    let change = async |actor: &User, target: &User, role: &str| {
        app.post(&format!("/members/{}?_method=PATCH", target.id))
            .login_as(actor)
            .csrf()
            .form(&[("role", role)])
            .send()
            .await
    };
    let refused = async |actor: &User, target: &User, role: &str, message: &str| {
        let before = role_of(&app, Team::ROOT_ID, target).await;
        let response = change(actor, target, role).await;
        assert_redirect(&response, "/team");
        assert_eq!(flash(&response, "error").as_deref(), Some(message));
        assert_eq!(role_of(&app, Team::ROOT_ID, target).await, before);
    };

    // An administrator: members and administrators, up to administrator.
    let promoted = change(&admin, &member, "admin").await;
    assert_redirect(&promoted, "/team");
    assert_eq!(
        flash(&promoted, "success").as_deref(),
        Some("Test User is now an admin")
    );
    assert_eq!(
        role_of(&app, Team::ROOT_ID, &member).await,
        Some(TeamRole::Admin)
    );
    change(&admin, &member, "member").await;
    assert_eq!(
        role_of(&app, Team::ROOT_ID, &member).await,
        Some(TeamRole::Member)
    );
    refused(
        &admin,
        &member,
        "owner",
        "Only an owner can make someone an owner",
    )
    .await;
    refused(
        &admin,
        &owner,
        "member",
        "Only an owner can change an owner",
    )
    .await;
    refused(&admin, &admin, "member", "You cannot change your own role").await;
    refused(
        &admin,
        &member,
        "viewer",
        "Choose a role: member, admin or owner",
    )
    .await;
    // Someone who is not in the team is not a member of it to change.
    let response = change(&admin, &outsider, "admin").await;
    assert_eq!(
        flash(&response, "error").as_deref(),
        Some("Member not found")
    );
    assert_eq!(role_of(&app, Team::ROOT_ID, &outsider).await, None);
    let response = app
        .delete(&format!("/members/{}", owner.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_eq!(
        flash(&response, "error").as_deref(),
        Some("Only an owner can remove an owner")
    );
    assert_eq!(
        role_of(&app, Team::ROOT_ID, &owner).await,
        Some(TeamRole::Owner)
    );

    // The page offers an administrator what they may do, and no more.
    let page = page(&app, &admin, Team::ROOT_ID, "/team").await;
    assert!(page.contains(&format!("action=\"/members/{}?_method=PATCH\"", member.id)));
    assert!(!page.contains(&format!("action=\"/members/{}?_method=PATCH\"", owner.id)));
    assert!(!page.contains("name=\"role\" value=\"owner\""));
    assert!(page.contains("<span class=\"badge badge--brand\">owner</span>"));

    // An owner: anyone, any role, other owners included.
    let made = change(&owner, &admin, "owner").await;
    assert_eq!(
        flash(&made, "success").as_deref(),
        Some("Test User is now an owner")
    );
    change(&owner, &second_owner, "member").await;
    assert_eq!(
        role_of(&app, Team::ROOT_ID, &second_owner).await,
        Some(TeamRole::Member)
    );
    refused(&owner, &owner, "member", "You cannot change your own role").await;
    // Whoever lost the role lost what it allowed.
    let response = change(&second_owner, &member, "admin").await;
    assert_redirect(&response, "/");
    assert_eq!(flash(&response, "error").as_deref(), Some(ADMIN_ONLY));
    assert_eq!(
        role_of(&app, Team::ROOT_ID, &member).await,
        Some(TeamRole::Member)
    );
}

#[tokio::test]
async fn an_owner_renames_a_team_and_deletes_it_once_it_has_no_mcp() {
    let app = TestApp::new().await;
    let root_owner = create_owner(&app).await;
    let team = create_team(&app, "Research").await;
    let owner = create_account(&app, None).await;
    let admin = create_account(&app, None).await;
    join_team(&app, team.id, &owner, TeamRole::Owner).await;
    join_team(&app, team.id, &admin, TeamRole::Admin).await;
    let mcp = create_mcp(&app, owner.id, |mcp| mcp.team_id = team.id).await;
    let token = token_in(&app, team.id, &admin, "Research token").await;

    // An administrator renames the team.
    let renamed = app
        .post("/team?_method=PATCH")
        .login_as(&admin)
        .csrf()
        .form(&[("name", " Lab "), ("description", "")])
        .send()
        .await;
    assert_redirect(&renamed, "/team");
    assert_eq!(flash(&renamed, "success").as_deref(), Some("Team updated"));
    let stored = Team::find(&*app.core.db, team.id).await.unwrap().unwrap();
    assert_eq!((stored.name.as_str(), stored.description), ("Lab", None));
    let refused = app
        .post("/team?_method=PATCH")
        .login_as(&admin)
        .csrf()
        .form(&[("name", "")])
        .send()
        .await;
    assert_redirect(&refused, "/team");
    assert!(refused.flashed("errors").is_some());
    let shown = app
        .get("/team")
        .session(refused.session())
        .send()
        .await
        .text();
    assert!(shown.contains("id=\"team-name-error\">"));

    // Deleting it is for an owner, and the page only offers it to one.
    let delete = async |user: &User, team_id: i64| {
        app.post("/team?_method=DELETE")
            .login_as(user)
            .in_team(team_id)
            .csrf()
            .send()
            .await
    };
    let refused = delete(&admin, team.id).await;
    assert_redirect(&refused, "/team");
    assert_eq!(
        flash(&refused, "error").as_deref(),
        Some("Only an owner can delete a team")
    );
    assert!(
        !page(&app, &admin, team.id, "/team")
            .await
            .contains("Danger zone")
    );

    // Not while it has MCPs: those hold credentials someone set up.
    let danger = page(&app, &owner, team.id, "/team").await;
    assert!(danger.contains("Danger zone"));
    assert!(danger.contains("Delete the MCPs of this team before you delete it."));
    assert!(!danger.contains("action=\"/team?_method=DELETE\""));
    let refused = delete(&owner, team.id).await;
    assert_eq!(
        flash(&refused, "error").as_deref(),
        Some("Delete the MCPs of this team first")
    );
    // Never the Root Team, nor a personal team.
    let refused = delete(&root_owner, Team::ROOT_ID).await;
    assert_eq!(
        flash(&refused, "error").as_deref(),
        Some("The Root Team cannot be deleted")
    );
    let mut transaction = app.core.db.begin().await.unwrap();
    let personal = Team::create_personal(&mut transaction, &owner)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    let refused = delete(&owner, personal.id).await;
    assert_eq!(
        flash(&refused, "error").as_deref(),
        Some("A personal team cannot be deleted")
    );
    assert_eq!(count(&app, "select count(*) from `teams`").await, 3);

    mcp.delete(&*app.core.db).await.unwrap();
    let danger = page(&app, &owner, team.id, "/team").await;
    assert!(danger.contains("action=\"/team?_method=DELETE\""));
    let deleted = delete(&owner, team.id).await;
    assert_redirect(&deleted, "/");
    assert_eq!(flash(&deleted, "success").as_deref(), Some("Team deleted"));
    assert!(Team::find(&*app.core.db, team.id).await.unwrap().is_none());
    assert!(find_token(&app, token.id).await.is_none());
    // Its members keep their accounts. The owner goes on in their other
    // team; the administrator, who had none, gets a personal one.
    let next = app
        .get("/team")
        .session(deleted.session())
        .send()
        .await
        .text();
    assert!(next.contains("<h1 class=\"page-header__title\">Test User's Team</h1>"));
    let stranded = app.get("/team").login_as(&admin).send().await;
    assert_eq!(stranded.status, StatusCode::OK);
    assert_eq!(
        Team::memberships(&*app.core.db, admin.id)
            .await
            .unwrap()
            .iter()
            .map(|membership| (membership.team.personal_team, membership.role))
            .collect::<Vec<_>>(),
        [(true, TeamRole::Owner)]
    );
}

// ----------------------------------------------------------------- invites

#[tokio::test]
async fn an_invite_carries_a_role_and_nobody_invites_above_their_own() {
    let app = TestApp::new().await;
    let owner = create_owner(&app).await;
    let admin = create_admin(&app).await;
    let invite = async |actor: &User, email: &str, role: &str| {
        app.post("/invites")
            .login_as(actor)
            .csrf()
            .form(&[("email", email), ("role", role)])
            .send()
            .await
    };
    let stored = async |email: &'static str| -> Option<Invite> {
        sqlx::query_as("select * from `invites` where `email` = ?")
            .bind(email)
            .fetch_optional(&*app.core.db)
            .await
            .unwrap()
    };

    let created = invite(&admin, "new-admin@example.com", "admin").await;
    assert_eq!(
        flash(&created, "success").as_deref(),
        Some("Invite created")
    );
    let new_admin = stored("new-admin@example.com").await.unwrap();
    assert_eq!(
        (new_admin.role, new_admin.team_id),
        (TeamRole::Admin, Team::ROOT_ID)
    );

    // The dialog of an administrator offers no owner, and the server gives none.
    let dialog = page(&app, &admin, Team::ROOT_ID, "/team").await;
    assert!(dialog.contains("<option value=\"member\">Member</option>"));
    assert!(dialog.contains("<option value=\"admin\">Admin</option>"));
    assert!(!dialog.contains("<option value=\"owner\""));
    let refused = from_script(app.post("/invites"))
        .login_as(&admin)
        .csrf()
        .form(&[("email", "new-owner@example.com"), ("role", "owner")])
        .send()
        .await;
    assert_eq!(refused.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(refused.text().contains("Only an owner can invite an owner"));
    assert!(stored("new-owner@example.com").await.is_none());
    let unknown = invite(&admin, "new-owner@example.com", "root").await;
    assert!(unknown.flashed("error").is_some());
    assert!(stored("new-owner@example.com").await.is_none());

    assert!(
        page(&app, &owner, Team::ROOT_ID, "/team")
            .await
            .contains("<option value=\"owner\">Owner</option>")
    );
    invite(&owner, "new-owner@example.com", "owner").await;
    let new_owner = stored("new-owner@example.com").await.unwrap();
    assert_eq!(new_owner.role, TeamRole::Owner);

    // The link of an invite is all it takes to join with its role: an
    // administrator is not shown the invite of an owner, and does not
    // remove it either. They see and remove the invites they could make.
    let listed = page(&app, &admin, Team::ROOT_ID, "/team").await;
    assert!(listed.contains(&new_admin.token));
    assert!(!listed.contains(&new_owner.token));
    assert!(!listed.contains("new-owner@example.com"));
    let listed = page(&app, &owner, Team::ROOT_ID, "/team").await;
    assert!(listed.contains(&new_admin.token) && listed.contains(&new_owner.token));
    let response = app
        .delete(&format!("/invites/{}", new_owner.id))
        .login_as(&admin)
        .csrf()
        .send()
        .await;
    assert_eq!(
        flash(&response, "error").as_deref(),
        Some("Invite not found")
    );
    assert!(stored("new-owner@example.com").await.is_some());

    // The invite of a team is its administrators' to remove, and no one else's.
    let (_, outsider) = team_with_owner(&app, "Elsewhere").await;
    let response = app
        .delete(&format!("/invites/{}", new_admin.id))
        .login_as(&outsider)
        .csrf()
        .send()
        .await;
    assert_eq!(
        flash(&response, "error").as_deref(),
        Some("Invite not found")
    );
    assert!(stored("new-admin@example.com").await.is_some());

    // Accepted with a new account: it comes with its personal team, joins
    // the team it was invited to with the role of the invite, and starts there.
    let accepted = app
        .post(&format!("/invite/{}", new_admin.token))
        .csrf()
        .form(&[
            ("fullName", "Ada Admin"),
            ("password", "password123"),
            ("passwordConfirmation", "password123"),
        ])
        .send()
        .await;
    assert_redirect(&accepted, "/");
    let ada = User::find_by_email(&*app.core.db, "new-admin@example.com")
        .await
        .unwrap()
        .unwrap();
    let teams: Vec<(String, TeamRole)> = Team::memberships(&*app.core.db, ada.id)
        .await
        .unwrap()
        .into_iter()
        .map(|membership| (membership.team.name, membership.role))
        .collect();
    assert_eq!(
        teams,
        [
            ("Ada Admin's Team".to_string(), TeamRole::Owner),
            ("Root Team".to_string(), TeamRole::Admin),
        ]
    );
    assert_eq!(ada.current_team_id, Some(Team::ROOT_ID));
    let home = app
        .get("/team")
        .session(accepted.session())
        .send()
        .await
        .text();
    assert!(home.contains("<h1 class=\"page-header__title\">Root Team</h1>"));
    // An administrator of the Root Team runs that team, not the instance.
    assert!(!ada.is_super_admin);
    let users = app.get("/users").session(accepted.session()).send().await;
    assert_eq!(flash(&users, "error").as_deref(), Some(SUPER_ADMIN_ONLY));
}

#[tokio::test]
async fn an_address_is_one_account_whatever_its_letter_case() {
    let app = TestApp::new().await;
    let owner = create_super_admin(&app).await;
    set_registration(&app, &owner, true).await;
    let ada = create_account(&app, Some("ada@example.com")).await;
    let users = async || count(&app, "select count(*) from `users`").await;
    let before = users().await;

    // Nobody takes the address of an account in another case: not by
    // signing up, nor by changing their own.
    let refused = sign_up(&app, "Not Ada", "Ada@Example.com").await;
    assert_redirect(&refused, "/signup");
    assert_eq!(users().await, before);
    let other = create_account(&app, Some("other@example.com")).await;
    let refused = app
        .post("/settings/email?_method=PATCH")
        .login_as(&other)
        .csrf()
        .form(&[("email", "ADA@example.com"), ("currentPassword", PASSWORD)])
        .send()
        .await;
    assert!(refused.flashed("error").is_some());
    let stored = User::find(&*app.core.db, other.id).await.unwrap().unwrap();
    assert_eq!(stored.email, "other@example.com");
    // Their own, they may write as they like.
    let changed = app
        .post("/settings/email?_method=PATCH")
        .login_as(&other)
        .csrf()
        .form(&[
            ("email", "Other@Example.com"),
            ("currentPassword", PASSWORD),
        ])
        .send()
        .await;
    assert_eq!(flash(&changed, "success").as_deref(), Some("Email updated"));

    // And the account signs in however its address is typed.
    let signed_in = app
        .post("/login")
        .csrf()
        .form(&[("email", "ADA@EXAMPLE.COM"), ("password", PASSWORD)])
        .send()
        .await;
    assert_redirect(&signed_in, "/");
    let found = User::find_by_email(&*app.core.db, "Ada@Example.COM")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, ada.id);
}

#[tokio::test]
async fn an_invite_is_for_the_account_of_its_address_and_no_look_alike() {
    let app = TestApp::new().await;
    let owner = create_owner(&app).await;
    // Two accounts whose addresses differ by letter case alone, as an
    // earlier version let them be made. The older one is in no team of
    // the owner; the other was made to look like it.
    let invited = create_account(&app, Some("grace@example.com")).await;
    let look_alike = create_account(&app, Some("Grace@Example.com")).await;
    let invite = create_invite(&app, owner.id, |invite| {
        invite.email = "grace@example.com".into();
        invite.role = TeamRole::Owner;
    })
    .await;
    let path = format!("/invite/{}", invite.token);

    // The look-alike is told the invite is not theirs, and joins nothing.
    let shown = app.get(&path).login_as(&look_alike).send().await;
    assert_redirect(&shown, "/");
    assert_eq!(
        flash(&shown, "error").as_deref(),
        Some("This invite is for grace@example.com. Sign in with that address to accept it.")
    );
    let refused = app.post(&path).login_as(&look_alike).csrf().send().await;
    assert_redirect(&refused, "/");
    assert_eq!(role_of(&app, Team::ROOT_ID, &look_alike).await, None);
    let stored: Invite = sqlx::query_as("select * from `invites` where `id` = ?")
        .bind(invite.id)
        .fetch_one(&*app.core.db)
        .await
        .unwrap();
    assert!(stored.accepted_at.is_none());
    // A visitor cannot make a third account for the address either: it has one.
    assert_redirect(&app.get(&path).send().await, "/login");

    // The account that was invited joins, with the role of the invite.
    let accepted = app.post(&path).login_as(&invited).csrf().send().await;
    assert_redirect(&accepted, "/");
    assert_eq!(
        flash(&accepted, "success").as_deref(),
        Some("You joined the team")
    );
    assert_eq!(
        role_of(&app, Team::ROOT_ID, &invited).await,
        Some(TeamRole::Owner)
    );

    // An invite written in another case than the account is still theirs.
    let invite = create_invite(&app, owner.id, |invite| {
        invite.email = "LINUS@example.com".into();
    })
    .await;
    let linus = create_account(&app, Some("linus@example.com")).await;
    let accepted = app
        .post(&format!("/invite/{}", invite.token))
        .login_as(&linus)
        .csrf()
        .send()
        .await;
    assert_eq!(
        flash(&accepted, "success").as_deref(),
        Some("You joined the team")
    );
    assert_eq!(
        role_of(&app, Team::ROOT_ID, &linus).await,
        Some(TeamRole::Member)
    );
}

#[tokio::test]
async fn an_invite_lets_someone_in_while_registration_is_closed() {
    let app = TestApp::new().await;
    let owner = create_owner(&app).await;
    assert!(!registration_open(&app).await);
    let invite = create_invite(&app, owner.id, |invite| {
        invite.email = "guest@example.com".into()
    })
    .await;

    let form = app.get(&format!("/invite/{}", invite.token)).send().await;
    assert_eq!(form.status, StatusCode::OK);
    let accepted = app
        .post(&format!("/invite/{}", invite.token))
        .csrf()
        .form(&[
            ("fullName", "Invited Guest"),
            ("password", "password123"),
            ("passwordConfirmation", "password123"),
        ])
        .send()
        .await;
    assert_redirect(&accepted, "/");
    assert_eq!(count(&app, "select count(*) from `users`").await, 2);
}

// ------------------------------------------------------------------- users

#[tokio::test]
async fn those_who_run_the_instance_see_its_accounts_and_delete_them() {
    let app = TestApp::new().await;
    let mut owner = create_user(&app, Some("owner@example.com"), TeamRole::Owner).await;
    make_super_admin(&app, &mut owner).await;
    // An administrator of the Root Team, who runs that team and no more.
    let admin = create_user(&app, Some("admin@example.com"), TeamRole::Admin).await;
    let member = create_user(&app, Some("member@example.com"), TeamRole::Member).await;
    // Someone who registered: a team of their own, with an MCP and a token.
    let (team, outsider) = team_with_owner(&app, "Their Team").await;
    let mcp = create_mcp(&app, outsider.id, |mcp| mcp.team_id = team.id).await;
    let token = token_in(&app, team.id, &outsider, "Their token").await;
    // And something they made in a team they share with others.
    join_team(&app, Team::ROOT_ID, &outsider, TeamRole::Member).await;
    let shared = create_mcp(&app, outsider.id, |mcp| mcp.name = "Shared".into()).await;
    let shared_token = token_in(&app, Team::ROOT_ID, &outsider, "Shared token").await;

    for user in [&admin, &member, &outsider] {
        let response = app.get("/users").login_as(user).send().await;
        assert_redirect(&response, "/");
        assert_eq!(flash(&response, "error").as_deref(), Some(SUPER_ADMIN_ONLY));
        let response = app
            .delete(&format!("/users/{}", owner.id))
            .login_as(user)
            .csrf()
            .send()
            .await;
        assert_eq!(flash(&response, "error").as_deref(), Some(SUPER_ADMIN_ONLY));
    }

    let listed = page(&app, &owner, Team::ROOT_ID, "/users").await;
    assert!(listed.contains("<h1 class=\"page-header__title\">Users</h1>"));
    assert!(listed.contains("4 accounts can sign in to this instance"));
    for email in [
        "owner@example.com",
        "admin@example.com",
        "member@example.com",
    ] {
        assert!(listed.contains(email), "{email}");
    }
    // One of them runs the instance, whatever the others are in the Root
    // Team: said once in the table, once in the list of a narrow screen.
    let badge = "<span class=\"badge badge--info\">Super admin</span>";
    assert_eq!(listed.matches(badge).count(), 2);
    assert_eq!(
        listed.matches("<span class=\"badge\">User</span>").count(),
        6
    );
    assert!(listed.contains("<td class=\"cell-secondary\">Their Team, Root Team</td>"));
    // A super admin deletes any account but their own. A row leads to the
    // dialog that asks for their password: a link, which the page script
    // opens in place.
    let offered = |id: i64| {
        listed.contains(&format!(
            "href=\"/users/{id}/delete\" data-dialog-open=\"#delete-user\" data-dialog-fetch"
        ))
    };
    assert!(offered(admin.id) && offered(member.id) && offered(outsider.id));
    assert!(!offered(owner.id));
    assert!(listed.contains("<dialog class=\"dialog dialog--sm\" id=\"delete-user\" aria-labelledby=\"delete-user-title\" data-dialog-reset data-dialog-return=\"/users\"><div data-fragment></div></dialog>"));
    assert!(!listed.contains("_method=DELETE"));

    let delete_with = async |actor: &User, id: String, password: &str| {
        app.post(&format!("/users/{id}?_method=DELETE"))
            .login_as(actor)
            .csrf()
            .form(&[("currentPassword", password)])
            .send()
            .await
    };
    let delete = async |actor: &User, id: String| delete_with(actor, id, PASSWORD).await;

    // The dialog of an account: the page with it open, or its form alone
    // for the page script.
    let confirm_path = format!("/users/{}/delete", outsider.id);
    let confirm = page(&app, &owner, Team::ROOT_ID, &confirm_path).await;
    assert!(confirm.contains("data-dialog-return=\"/users\" data-open>"));
    assert!(confirm.contains(&format!(
        "<form method=\"post\" action=\"/users/{}?_method=DELETE\" data-async>",
        outsider.id
    )));
    assert!(
        confirm.contains(
            "<h2 class=\"dialog__title\" id=\"delete-user-title\">Delete Test User?</h2>"
        )
    );
    assert!(confirm.contains("id=\"delete-user-password\" name=\"currentPassword\" type=\"password\" autocomplete=\"current-password\" required"));
    assert!(confirm.contains(
        "<button type=\"submit\" class=\"button button--critical\">Delete user</button>"
    ));
    let fragment = from_script(app.get(&confirm_path))
        .header("x-fragment", "delete-user")
        .login_as(&owner)
        .send()
        .await;
    assert_eq!(fragment.status, StatusCode::OK);
    assert_eq!(fragment.header("cache-control"), Some("no-store"));
    assert!(fragment.text().starts_with(&format!(
        "<form method=\"post\" action=\"/users/{}?_method=DELETE\" data-async>",
        outsider.id
    )));
    // No dialog for their own account.
    let response = app
        .get(&format!("/users/{}/delete", owner.id))
        .login_as(&owner)
        .send()
        .await;
    assert_redirect(&response, "/users");
    assert_eq!(
        flash(&response, "error").as_deref(),
        Some("You cannot delete your own account")
    );

    // A signed-in browser is no proof that it is them: without their own
    // password, or with a wrong one, nothing is deleted.
    let missing = app
        .post(&format!("/users/{}?_method=DELETE", outsider.id))
        .login_as(&owner)
        .csrf()
        .send()
        .await;
    assert_redirect(&missing, &confirm_path);
    assert_eq!(
        missing.flashed("errors"),
        Some(json!({ "currentPassword": "The currentPassword field must be defined" }))
    );
    let wrong = delete_with(&owner, outsider.id.to_string(), "not-the-password").await;
    assert_redirect(&wrong, &confirm_path);
    assert_eq!(
        wrong.flashed("errors"),
        Some(json!({ "currentPassword": "The current password is incorrect" }))
    );
    let reopened = app
        .get(&confirm_path)
        .session(wrong.session())
        .send()
        .await
        .text();
    assert!(reopened.contains("data-open>"));
    assert!(reopened.contains(
        "<p class=\"field__error\" id=\"delete-user-password-error\">The current password is incorrect</p>"
    ));
    assert!(!reopened.contains("not-the-password"));
    let scripted = from_script(app.post(&format!("/users/{}?_method=DELETE", outsider.id)))
        .login_as(&owner)
        .csrf()
        .form(&[("currentPassword", "not-the-password")])
        .send()
        .await;
    assert_eq!(scripted.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        scripted
            .text()
            .contains("The current password is incorrect")
    );
    assert!(
        User::find(&*app.core.db, outsider.id)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(count(&app, "select count(*) from `users`").await, 4);
    for (id, message) in [
        (owner.id.to_string(), "You cannot delete your own account"),
        ("999999".to_string(), "User not found"),
        ("abc".to_string(), "User not found"),
    ] {
        let response = delete(&owner, id.clone()).await;
        assert_redirect(&response, "/users");
        assert_eq!(flash(&response, "error").as_deref(), Some(message), "{id}");
    }
    assert_eq!(count(&app, "select count(*) from `users`").await, 4);

    let deleted = delete(&owner, outsider.id.to_string()).await;
    assert_redirect(&deleted, "/users");
    assert_eq!(flash(&deleted, "success").as_deref(), Some("User deleted"));
    let db = &*app.core.db;
    assert!(User::find(db, outsider.id).await.unwrap().is_none());
    // The team they were alone in went with them, and what it had.
    assert!(Team::find(db, team.id).await.unwrap().is_none());
    assert!(find_mcp(&app, mcp.id).await.is_none());
    assert!(find_token(&app, token.id).await.is_none());
    // What they made in the team they shared stays, with an owner of it;
    // their token there no longer works.
    let kept = find_mcp(&app, shared.id).await.unwrap();
    assert_eq!((kept.team_id, kept.created_by), (Team::ROOT_ID, owner.id));
    let kept = find_token(&app, shared_token.id).await.unwrap();
    assert!(kept.is_revoked());
    assert_eq!(kept.created_by, owner.id);
    // Their session is worth nothing.
    assert_redirect(&app.get("/").login_as(&outsider).send().await, "/login");

    // What an account is in the Root Team protects it from nothing.
    let deleted = delete(&owner, admin.id.to_string()).await;
    assert_eq!(flash(&deleted, "success").as_deref(), Some("User deleted"));
    assert_eq!(count(&app, "select count(*) from `users`").await, 2);
}

#[tokio::test]
async fn a_super_admin_decides_who_else_runs_the_instance() {
    let app = TestApp::new().await;
    let mut owner = create_user(&app, Some("owner@example.com"), TeamRole::Owner).await;
    make_super_admin(&app, &mut owner).await;
    let admin = create_user(&app, Some("admin@example.com"), TeamRole::Admin).await;
    // Someone who is in no team of the super admin.
    let (team, outsider) = team_with_owner(&app, "Their Team").await;
    let is_super_admin = async |user: &User| {
        let user = User::find(&*app.core.db, user.id).await.unwrap().unwrap();
        user.is_super_admin
    };
    let path = |user: &User| format!("/users/{}/super-admin", user.id);
    let change_with = async |actor: &User, account: &User, fields: &[(&str, &str)]| {
        app.post(&format!("{}?_method=PATCH", path(account)))
            .login_as(actor)
            .csrf()
            .form(fields)
            .send()
            .await
    };
    let change = async |actor: &User, account: &User, super_admin: &str| {
        let fields = [("superAdmin", super_admin), ("currentPassword", PASSWORD)];
        change_with(actor, account, &fields).await
    };

    // An administrator of the Root Team decides nothing of it.
    let response = app.get(&path(&outsider)).login_as(&admin).send().await;
    assert_eq!(flash(&response, "error").as_deref(), Some(SUPER_ADMIN_ONLY));
    let response = change(&admin, &admin, "true").await;
    assert_redirect(&response, "/");
    assert_eq!(flash(&response, "error").as_deref(), Some(SUPER_ADMIN_ONLY));
    assert!(!is_super_admin(&admin).await);

    // A row leads to the dialog, which says what the account becomes.
    let listed = page(&app, &owner, Team::ROOT_ID, "/users").await;
    let offered = |user: &User| {
        listed.contains(&format!(
            "href=\"{}\" data-dialog-open=\"#super-admin\" data-dialog-fetch",
            path(user)
        ))
    };
    assert!(offered(&admin) && offered(&outsider) && !offered(&owner));
    assert!(listed.contains("Make super admin") && !listed.contains("Remove super admin"));
    assert!(listed.contains("<dialog class=\"dialog dialog--sm\" id=\"super-admin\" aria-labelledby=\"super-admin-title\" data-dialog-reset data-dialog-return=\"/users\"><div data-fragment></div></dialog>"));
    let form = format!(
        "<form method=\"post\" action=\"{}?_method=PATCH\" data-async>",
        path(&outsider)
    );
    let confirm = page(&app, &owner, Team::ROOT_ID, &path(&outsider)).await;
    assert!(confirm.contains("id=\"super-admin\" aria-labelledby=\"super-admin-title\" data-dialog-reset data-dialog-return=\"/users\" data-open>"));
    assert!(confirm.contains(&form));
    assert!(confirm.contains(
        "<h2 class=\"dialog__title\" id=\"super-admin-title\">Make Test User a super admin?</h2>"
    ));
    assert!(confirm.contains("<input type=\"hidden\" name=\"superAdmin\" value=\"true\">"));
    assert!(confirm.contains("id=\"super-admin-password\" name=\"currentPassword\" type=\"password\" autocomplete=\"current-password\" required"));
    assert!(confirm.contains(
        "<button type=\"submit\" class=\"button button--primary\">Make super admin</button>"
    ));
    let fragment = from_script(app.get(&path(&outsider)))
        .header("x-fragment", "super-admin")
        .login_as(&owner)
        .send()
        .await;
    assert_eq!(fragment.status, StatusCode::OK);
    assert_eq!(fragment.header("cache-control"), Some("no-store"));
    assert!(fragment.text().starts_with(&form));

    // Nobody decides it of their own account, nor of one that is not there.
    let response = app.get(&path(&owner)).login_as(&owner).send().await;
    assert_redirect(&response, "/users");
    for (account, message) in [
        (
            owner.id.to_string(),
            "You cannot change your own super admin access",
        ),
        ("999999".to_string(), "User not found"),
    ] {
        let response = app
            .post(&format!("/users/{account}/super-admin?_method=PATCH"))
            .login_as(&owner)
            .csrf()
            .form(&[("superAdmin", "false"), ("currentPassword", PASSWORD)])
            .send()
            .await;
        assert_redirect(&response, "/users");
        assert_eq!(flash(&response, "error").as_deref(), Some(message));
    }
    assert!(is_super_admin(&owner).await);

    // A signed-in browser is no proof that it is them: without their own
    // password, or with a wrong one, nothing changes.
    let missing = change_with(&owner, &outsider, &[("superAdmin", "true")]).await;
    assert_redirect(&missing, &path(&outsider));
    assert_eq!(
        missing.flashed("errors"),
        Some(json!({ "currentPassword": "The currentPassword field must be defined" }))
    );
    let wrong = change_with(
        &owner,
        &outsider,
        &[
            ("superAdmin", "true"),
            ("currentPassword", "not-the-password"),
        ],
    )
    .await;
    assert_redirect(&wrong, &path(&outsider));
    let reopened = app
        .get(&path(&outsider))
        .session(wrong.session())
        .send()
        .await
        .text();
    assert!(reopened.contains("data-open>"));
    assert!(reopened.contains(
        "<p class=\"field__error\" id=\"super-admin-password-error\">The current password is incorrect</p>"
    ));
    let scripted = from_script(app.post(&format!("{}?_method=PATCH", path(&outsider))))
        .login_as(&owner)
        .csrf()
        .form(&[
            ("superAdmin", "true"),
            ("currentPassword", "not-the-password"),
        ])
        .send()
        .await;
    assert_eq!(scripted.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(scripted.text().starts_with(&form));
    // And the form says what the account becomes.
    let unsaid = change_with(&owner, &outsider, &[("currentPassword", PASSWORD)]).await;
    assert_redirect(&unsaid, &path(&outsider));
    assert!(!is_super_admin(&outsider).await);

    let made = change(&owner, &outsider, "true").await;
    assert_redirect(&made, "/users");
    assert_eq!(
        flash(&made, "success").as_deref(),
        Some("Test User is now a super admin")
    );
    assert!(is_super_admin(&outsider).await);
    // They run the instance from the team they work in, and are in no more
    // teams than before: the Root Team stays closed to them.
    let memberships = Team::memberships(&*app.core.db, outsider.id).await.unwrap();
    assert_eq!(memberships.len(), 1);
    assert_eq!(memberships[0].team.id, team.id);
    let users = page(&app, &outsider, team.id, "/users").await;
    assert_eq!(
        users
            .matches("<span class=\"badge badge--info\">Super admin</span>")
            .count(),
        4
    );
    let settings = page(&app, &outsider, team.id, "/settings").await;
    assert!(settings.contains("My Instance") && settings.contains("registrationEnabled"));
    let switched = app
        .post("/teams/switch")
        .login_as(&outsider)
        .csrf()
        .form(&[("teamId", "1")])
        .send()
        .await;
    assert!(switched.flashed("error").is_some());

    // The dialog of a super admin takes it from them.
    let confirm = page(&app, &outsider, team.id, &path(&owner)).await;
    assert!(confirm.contains(
        "<h2 class=\"dialog__title\" id=\"super-admin-title\">Remove Test User from the super admins?</h2>"
    ));
    assert!(confirm.contains("<input type=\"hidden\" name=\"superAdmin\" value=\"false\">"));
    assert!(confirm.contains(
        "<button type=\"submit\" class=\"button button--critical\">Remove super admin</button>"
    ));
    let removed = change(&outsider, &owner, "false").await;
    assert_redirect(&removed, "/users");
    assert_eq!(
        flash(&removed, "success").as_deref(),
        Some("Test User is no longer a super admin")
    );
    assert!(!is_super_admin(&owner).await);
    // The first account keeps the Root Team it owns, and no longer runs
    // the instance.
    assert_eq!(
        role_of(&app, Team::ROOT_ID, &owner).await,
        Some(TeamRole::Owner)
    );
    let response = app.get("/users").login_as(&owner).send().await;
    assert_eq!(flash(&response, "error").as_deref(), Some(SUPER_ADMIN_ONLY));
    let team_page = page(&app, &owner, Team::ROOT_ID, "/team").await;
    assert!(team_page.contains("<h1 class=\"page-header__title\">Root Team</h1>"));
    // The only super admin left stays one: they cannot take it from
    // themselves, and nobody else can.
    let response = change(&outsider, &outsider, "false").await;
    assert_eq!(
        flash(&response, "error").as_deref(),
        Some("You cannot change your own super admin access")
    );
    assert!(is_super_admin(&outsider).await);
}

#[tokio::test]
async fn a_role_in_a_team_gives_no_right_over_the_instance() {
    let app = TestApp::new().await;
    let owner = create_super_admin(&app).await;
    let (team, outsider) = team_with_owner(&app, "Elsewhere").await;
    // Also a plain member of the Root Team: what they own elsewhere gives
    // them nothing here.
    join_team(&app, Team::ROOT_ID, &outsider, TeamRole::Member).await;
    // And an owner of the Root Team, who runs that team and no more.
    let root_owner = create_owner(&app).await;

    for (user, team_id) in [
        (&outsider, team.id),
        (&outsider, Team::ROOT_ID),
        (&root_owner, Team::ROOT_ID),
    ] {
        for (method, path) in [
            ("get", "/users"),
            ("delete", "/users/1"),
            ("get", "/users/1/super-admin"),
            ("patch", "/users/1/super-admin"),
            ("patch", "/settings/mcp-logging"),
            ("post", "/settings/backup"),
        ] {
            let request = match method {
                "get" => app.get(path),
                "patch" => app.patch(path).form(&INSTANCE_FIELDS),
                "post" => app.post(path),
                _ => app.delete(path),
            };
            let response = request.login_as(user).in_team(team_id).csrf().send().await;
            assert_redirect(&response, "/");
            assert_eq!(
                flash(&response, "error").as_deref(),
                Some(SUPER_ADMIN_ONLY),
                "{method} {path} in team {team_id}"
            );
        }
    }
    // The owner of the Root Team does run it: its logs, its members.
    let logs = app
        .get("/logs")
        .login_as(&root_owner)
        .in_team(Team::ROOT_ID)
        .send()
        .await;
    assert_eq!(logs.status, StatusCode::OK);
    let navigation = logs.text();
    assert!(!navigation.contains("href=\"/users\""));
    // The other is a member of the Root Team: no logs, no MCP to change.
    let logs = app
        .get("/logs")
        .login_as(&outsider)
        .in_team(Team::ROOT_ID)
        .send()
        .await;
    assert_eq!(flash(&logs, "error").as_deref(), Some(ADMIN_ONLY));
    // In their own team they are the owner.
    let logs = app
        .get("/logs")
        .login_as(&outsider)
        .in_team(team.id)
        .send()
        .await;
    assert_eq!(logs.status, StatusCode::OK);

    // Whoever runs the instance does, whichever team they work in.
    join_team(&app, team.id, &owner, TeamRole::Member).await;
    let users = app
        .get("/users")
        .login_as(&owner)
        .in_team(team.id)
        .send()
        .await;
    assert_eq!(users.status, StatusCode::OK);
    assert!(
        users
            .text()
            .contains("<span class=\"nav-user__role\">member</span>")
    );
}
