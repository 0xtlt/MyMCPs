use std::time::Duration;

use mymcps_core::limiter::{Limiter, LimiterError};
use mymcps_core::models::{
    AccessToken, ApprovalDecision, ApprovalRequest, ApprovalState, InstanceSetting, Invite, Mcp,
    McpLogLevel, McpStatus, McpTransport, OauthClient, SIGNED_OUT_FOR_ANOTHER_TAILNET, Tailnet,
    TailnetIdentity, Team, TeamRole, User,
};
use mymcps_core::{Config, Core, Db, Timestamp};

async fn core() -> (std::sync::Arc<Core>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::boot(Config::for_tests(dir.path())).await.unwrap();
    (core, dir)
}

async fn admin(db: &Db) -> User {
    let mut user =
        User::with_password("admin@example.com", Some("Ada Lovelace"), "secret-password")
            .await
            .unwrap();
    let mut transaction = db.begin().await.unwrap();
    user.create_first(&mut transaction).await.unwrap();
    transaction.commit().await.unwrap();
    user
}

#[tokio::test]
async fn users_sign_in_and_change_their_password() {
    let (core, _dir) = core().await;
    let db = &core.db;
    assert!(!User::setup_complete(&**db).await.unwrap());

    let mut user = admin(db).await;
    assert!(user.id > 0);
    assert!(user.is_super_admin, "the first account runs the instance");
    assert_eq!(user.initials(), "AL");
    assert_eq!(user.session_version, 1);
    assert!(User::setup_complete(&**db).await.unwrap());

    assert!(
        User::verify_credentials(db, "admin@example.com", "secret-password")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        User::verify_credentials(db, "admin@example.com", "wrong")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        User::verify_credentials(db, "nobody@example.com", "secret-password")
            .await
            .unwrap()
            .is_none()
    );

    sqlx::query("insert into `remember_me_tokens` (`tokenable_id`, `hash`, `created_at`, `updated_at`, `expires_at`) values (?, 'h', ?, ?, ?)")
        .bind(user.id)
        .bind(Timestamp::now())
        .bind(Timestamp::now())
        .bind(Timestamp::now())
        .execute(&**db)
        .await
        .unwrap();
    user.change_password(db, "another-password").await.unwrap();
    assert_eq!(user.session_version, 2);
    let reloaded = User::find(&**db, user.id).await.unwrap().unwrap();
    assert_eq!(reloaded.session_version, 2);
    assert!(reloaded.verify_password("another-password").await.unwrap());
    let remembered: i64 = sqlx::query_scalar("select count(*) from `remember_me_tokens`")
        .fetch_one(&**db)
        .await
        .unwrap();
    assert_eq!(remembered, 0);

    let member = User {
        email: "grace@example.com".into(),
        ..Default::default()
    };
    assert_eq!(member.initials(), "GE");
    let member = User {
        email: "x".into(),
        full_name: Some("Plato".into()),
        ..Default::default()
    };
    assert_eq!(member.initials(), "PL");
}

#[tokio::test]
async fn save_writes_only_the_columns_that_changed() {
    let (core, _dir) = core().await;
    let db = &core.db;
    let user = admin(db).await;

    let mut mcp = Mcp {
        name: "Notion".into(),
        slug: Mcp::slugify("Notion"),
        transport: McpTransport::Http,
        http_url: Some("https://mcp.notion.com/mcp".into()),
        enabled: true,
        created_by: user.id,
        team_id: Team::ROOT_ID,
        ..Default::default()
    };
    mcp.insert(&**db).await.unwrap();
    assert!(mcp.id > 0 && mcp.is_persisted());
    assert_eq!(mcp.status, McpStatus::Draft);
    let stored_at: String = sqlx::query_scalar("select `created_at` from `mcps`")
        .fetch_one(&**db)
        .await
        .unwrap();
    assert_eq!(stored_at, mcp.created_at.to_sql());
    assert_eq!(stored_at.len(), "2026-10-07 12:19:57".len());

    // Two requests load the same row and change different columns.
    let mut refresher = Mcp::find(&**db, mcp.id).await.unwrap().unwrap();
    let mut prober = Mcp::find(&**db, mcp.id).await.unwrap().unwrap();
    refresher.oauth_access_token = core.encrypt_secret(Some("fresh-token"));
    refresher.save(&**db).await.unwrap();
    prober.status = McpStatus::Ready;
    prober.last_error = None;
    prober.save(&**db).await.unwrap();

    mcp.refresh(&**db).await.unwrap();
    assert_eq!(mcp.status, McpStatus::Ready);
    assert_eq!(
        core.decrypt_secret(mcp.oauth_access_token.as_deref())
            .as_deref(),
        Some("fresh-token")
    );

    // Nothing changed: nothing is written, not even updated_at.
    sqlx::query("update `mcps` set `updated_at` = '2020-01-01 00:00:00'")
        .execute(&**db)
        .await
        .unwrap();
    mcp.refresh(&**db).await.unwrap();
    mcp.save(&**db).await.unwrap();
    let updated_at: String = sqlx::query_scalar("select `updated_at` from `mcps`")
        .fetch_one(&**db)
        .await
        .unwrap();
    assert_eq!(updated_at, "2020-01-01 00:00:00");

    mcp.set_npm_args_list(&["--flag".to_string(), "value".to_string()]);
    mcp.tool_timeout_seconds = Some(240);
    mcp.save(&**db).await.unwrap();
    let reloaded = Mcp::find(&**db, mcp.id).await.unwrap().unwrap();
    assert_eq!(reloaded.tool_timeout_seconds, Some(240));
    assert_eq!(reloaded.npm_args.as_deref(), Some("[\"--flag\",\"value\"]"));
    assert_eq!(reloaded.npm_args_list(), ["--flag", "value"]);
    assert!(reloaded.updated_at.unwrap() > Timestamp::parse_sql("2020-01-01 00:00:00").unwrap());

    // Deleting the user removes what they created, as the Node app's driver enforced.
    user.delete(&**db).await.unwrap();
    assert!(Mcp::find(&**db, mcp.id).await.unwrap().is_none());
    assert!(matches!(
        mcp.refresh(&**db).await,
        Err(sqlx::Error::RowNotFound)
    ));
}

#[tokio::test]
async fn a_column_cleared_on_purpose_is_written_even_when_the_row_was_read_empty() {
    let (core, _dir) = core().await;
    let db = &core.db;
    let user = admin(db).await;
    let mut mcp = mcp_of(db, Team::ROOT_ID, user.id, "Notion").await;

    // An edit reads the row with no token and clears it, while another
    // request saves one.
    let mut edit = Mcp::find(&**db, mcp.id).await.unwrap().unwrap();
    mcp.oauth_access_token = core.encrypt_secret(Some("saved-meanwhile"));
    mcp.save(&**db).await.unwrap();

    edit.oauth_access_token = None;
    edit.name = "Renamed".into();
    edit.save(&**db).await.unwrap();
    let after = Mcp::find(&**db, mcp.id).await.unwrap().unwrap();
    assert_eq!(after.name, "Renamed");
    assert!(
        after.oauth_access_token.is_some(),
        "unchanged as far as the edit knows: not written"
    );

    // Cleared on purpose, it is written whatever the edit read.
    mcp.oauth_access_token = None;
    mcp.save(&**db).await.unwrap();
    let mut edit = Mcp::find(&**db, mcp.id).await.unwrap().unwrap();
    mcp.oauth_access_token = core.encrypt_secret(Some("saved-again"));
    mcp.save(&**db).await.unwrap();
    edit.oauth_access_token = None;
    edit.oauth_refresh_token = None;
    edit.write_over(&["oauth_access_token", "oauth_refresh_token"]);
    edit.save(&**db).await.unwrap();
    let after = Mcp::find(&**db, mcp.id).await.unwrap().unwrap();
    assert_eq!(after.oauth_access_token, None);
    // Written once: the next save of the same value is a save of nothing.
    assert!(edit.forced.is_empty());
}

#[tokio::test]
async fn a_secret_that_did_not_change_keeps_its_ciphertext() {
    let (core, _dir) = core().await;
    let saved = core.encrypt_secret(Some("client-secret"));
    assert_eq!(
        core.encrypt_secret_keeping(saved.as_deref(), Some("client-secret")),
        saved
    );
    let changed = core.encrypt_secret_keeping(saved.as_deref(), Some("another-secret"));
    assert_ne!(changed, saved);
    assert_eq!(
        core.decrypt_secret(changed.as_deref()).as_deref(),
        Some("another-secret")
    );
    // Nothing saved, or another APP_KEY's ciphertext that cannot be read: encrypted anew.
    for saved in [None, Some("not-ciphertext")] {
        let fresh = core.encrypt_secret_keeping(saved, Some("client-secret"));
        assert_eq!(
            core.decrypt_secret(fresh.as_deref()).as_deref(),
            Some("client-secret")
        );
    }
    assert_eq!(core.encrypt_secret_keeping(saved.as_deref(), None), None);
}

#[test]
fn slugs_and_lists_follow_the_node_models() {
    assert_eq!(
        Mcp::slugify("  My Notion — Workspace! "),
        "my-notion-workspace"
    );
    assert_eq!(Mcp::slugify("Été 2026"), "t-2026");
    assert_eq!(Mcp::slugify("***"), "mcp");
    // Cut at 80 characters after trimming the dashes, even on a dash.
    assert_eq!(Mcp::slugify(&"abc ".repeat(40)).len(), 80);
    assert!(Mcp::slugify(&"abc ".repeat(40)).ends_with("abc-"));

    let mcp = Mcp {
        npm_args: Some("[\"a\",2,true]".into()),
        ..Default::default()
    };
    assert_eq!(mcp.npm_args_list(), ["a", "2", "true"]);
    for unreadable in [None, Some(""), Some("{}"), Some("not json")] {
        let mcp = Mcp {
            npm_args: unreadable.map(str::to_string),
            ..Default::default()
        };
        assert!(mcp.npm_args_list().is_empty());
    }

    let client = OauthClient {
        redirect_uris: "[\"http://localhost:1/callback\",3]".into(),
        grant_types: format!("[\"{}\"]", "x".repeat(300)),
        response_types: "[\"code\"]".into(),
        ..Default::default()
    };
    assert_eq!(client.redirect_uri_list(), ["http://localhost:1/callback"]);
    assert!(client.grant_type_list().is_empty());
    assert_eq!(client.response_type_list(), ["code"]);
}

#[test]
fn a_tool_call_waits_the_timeout_of_its_mcp_within_what_its_transport_allows() {
    let timeout = |transport: McpTransport, seconds: Option<i64>| {
        Mcp {
            transport,
            tool_timeout_seconds: seconds,
            ..Default::default()
        }
        .tool_timeout()
        .as_secs()
    };
    // Nothing saved waits the default.
    assert_eq!(timeout(McpTransport::Http, None), 60);
    assert_eq!(timeout(McpTransport::Npm, None), 60);
    assert_eq!(timeout(McpTransport::Http, Some(240)), 240);
    assert_eq!(timeout(McpTransport::Npm, Some(5)), 5);
    // A remote server only holds a connection open; an npm package is a
    // process of this server, which waits less.
    assert_eq!(McpTransport::Http.max_tool_timeout_seconds(), Some(300));
    assert_eq!(McpTransport::Npm.max_tool_timeout_seconds(), Some(120));
    assert_eq!(McpTransport::Builtin.max_tool_timeout_seconds(), None);
    // A row saved with more, or with nothing sensible, is held to the rules.
    assert_eq!(timeout(McpTransport::Http, Some(3600)), 300);
    assert_eq!(timeout(McpTransport::Npm, Some(300)), 120);
    assert_eq!(timeout(McpTransport::Http, Some(0)), 60);
    assert_eq!(timeout(McpTransport::Npm, Some(-5)), 60);
}

#[tokio::test]
async fn states_follow_the_clock() {
    let now = Timestamp::now();
    let hour = chrono::Duration::hours(1);

    let invite = Invite {
        expires_at: now + hour,
        ..Default::default()
    };
    assert!(invite.is_usable());
    assert!(
        !Invite {
            expires_at: now - hour,
            ..Default::default()
        }
        .is_usable()
    );
    assert!(
        !Invite {
            expires_at: now + hour,
            accepted_at: Some(now),
            ..Default::default()
        }
        .is_usable()
    );
    assert_eq!(Invite::generate_token().len(), 64);

    let token = AccessToken::default();
    assert!(token.is_usable() && token.is_active());
    assert!(
        !AccessToken {
            revoked_at: Some(now),
            ..Default::default()
        }
        .is_usable()
    );
    assert!(
        !AccessToken {
            expires_at: Some(now - hour),
            ..Default::default()
        }
        .is_usable()
    );
    let oauth = AccessToken {
        source: mymcps_core::models::TokenSource::Oauth,
        expires_at: Some(now - hour),
        oauth_refresh_expires_at: Some(now + hour),
        ..Default::default()
    };
    assert!(!oauth.is_usable(), "the access token itself has expired");
    assert!(
        oauth.is_active(),
        "but the connection can still be refreshed"
    );

    let pending = ApprovalRequest {
        expires_at: now + hour,
        ..Default::default()
    };
    assert_eq!(pending.state(), ApprovalState::Pending);
    assert_eq!(
        ApprovalRequest {
            expires_at: now - hour,
            ..Default::default()
        }
        .state(),
        ApprovalState::Expired
    );
    let approved = ApprovalRequest {
        status: ApprovalDecision::Approved,
        expires_at: now + hour,
        ..Default::default()
    };
    assert_eq!(approved.state(), ApprovalState::Approved);
    assert_eq!(
        ApprovalRequest {
            consumed_at: Some(now),
            expires_at: now - hour,
            ..approved.clone()
        }
        .state(),
        ApprovalState::Used
    );
    assert_eq!(
        ApprovalRequest {
            status: ApprovalDecision::Denied,
            expires_at: now - hour,
            ..Default::default()
        }
        .state(),
        ApprovalState::Denied
    );
}

#[tokio::test]
async fn instance_settings_exist_once() {
    let (core, _dir) = core().await;
    let db = &core.db;
    let mut settings = InstanceSetting::current(&**db).await.unwrap();
    assert_eq!(settings.id, 1);
    assert_eq!(settings.mcp_log_level, McpLogLevel::Metadata);
    assert_eq!(settings.mcp_log_retention_days, 14);
    assert_eq!(settings.mcp_auto_update_cron, "0 2 * * *");
    assert!(!settings.mcp_auto_update_enabled);

    settings.mcp_log_level = McpLogLevel::Responses;
    settings.save(&**db).await.unwrap();
    assert_eq!(
        InstanceSetting::current(&**db).await.unwrap().mcp_log_level,
        McpLogLevel::Responses
    );

    // A database whose row was removed gets it back with the defaults.
    sqlx::query("delete from `instance_settings`")
        .execute(&**db)
        .await
        .unwrap();
    assert_eq!(
        InstanceSetting::current(&**db).await.unwrap().mcp_log_level,
        McpLogLevel::Metadata
    );
    let rows: i64 = sqlx::query_scalar("select count(*) from `instance_settings`")
        .fetch_one(&**db)
        .await
        .unwrap();
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn limiters_count_per_window() {
    let (core, _dir) = core().await;
    for limiter in [
        Limiter::database(&core.db, 3, Duration::from_secs(900)),
        Limiter::memory(3, Duration::from_secs(900)),
    ] {
        assert_eq!(limiter.remaining("login:a").await.unwrap(), 3);
        assert_eq!(limiter.available_in("login:a").await.unwrap(), 0);
        for consumed in 1..=3 {
            let response = limiter.consume("login:a").await.unwrap();
            assert_eq!(
                (response.consumed, response.remaining),
                (consumed, 3 - consumed)
            );
        }
        let Err(LimiterError::TooManyRequests(refused)) = limiter.consume("login:a").await else {
            panic!("the fourth request is refused");
        };
        assert_eq!(refused.remaining, 0);
        assert!((1..=900).contains(&refused.available_in));
        assert!((1..=900).contains(&limiter.available_in("login:a").await.unwrap()));
        assert!(!limiter.attempt("login:a").await.unwrap());

        // Other keys are untouched, and attempts stop at the allowance.
        for _ in 0..3 {
            assert!(limiter.attempt("login:b").await.unwrap());
        }
        assert!(!limiter.attempt("login:b").await.unwrap());

        limiter.decrement("login:b").await.unwrap();
        limiter.decrement("login:b").await.unwrap();
        assert_eq!(limiter.remaining("login:b").await.unwrap(), 1);
        limiter.decrement("login:never-seen").await.unwrap();
        assert_eq!(limiter.remaining("login:never-seen").await.unwrap(), 3);

        limiter.delete("login:a").await.unwrap();
        assert_eq!(limiter.remaining("login:a").await.unwrap(), 3);
        assert!(limiter.attempt("login:a").await.unwrap());
    }

    // A window that has ended starts over.
    let brief = Limiter::database(&core.db, 1, Duration::from_millis(40));
    assert!(brief.attempt("k").await.unwrap());
    assert!(!brief.attempt("k").await.unwrap());
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(brief.attempt("k").await.unwrap());
}

/// A later account, with its personal team.
async fn account(db: &Db, email: &str, name: Option<&str>) -> (User, Team) {
    let mut user = User::with_password(email, name, "secret-password")
        .await
        .unwrap();
    let mut transaction = db.begin().await.unwrap();
    let team = user.create(&mut transaction).await.unwrap();
    transaction.commit().await.unwrap();
    (user, team)
}

async fn mcp_of(db: &Db, team_id: i64, created_by: i64, name: &str) -> Mcp {
    let mut mcp = Mcp {
        name: name.into(),
        slug: Mcp::slugify(name),
        transport: McpTransport::Http,
        created_by,
        team_id,
        ..Default::default()
    };
    mcp.insert(&**db).await.unwrap();
    mcp
}

async fn token_of(db: &Db, team_id: i64, created_by: i64, name: &str) -> AccessToken {
    let mut token = AccessToken {
        name: name.into(),
        token_prefix: "mcp_".into(),
        token_hash: format!("hash-{team_id}-{created_by}-{name}"),
        created_by,
        team_id,
        ..Default::default()
    };
    token.insert(&**db).await.unwrap();
    token
}

async fn roles(db: &Db, team_id: i64) -> Vec<(i64, TeamRole)> {
    sqlx::query_as("select `user_id`, `role` from `team_user` where `team_id` = ? order by `id`")
        .bind(team_id)
        .fetch_all(&**db)
        .await
        .unwrap()
}

#[tokio::test]
async fn every_account_has_a_team_and_the_first_one_owns_the_root_team() {
    let (core, _dir) = core().await;
    let db = &core.db;

    let first = admin(db).await;
    assert_eq!(
        roles(db, Team::ROOT_ID).await,
        [(first.id, TeamRole::Owner)]
    );
    let teams = Team::memberships(&**db, first.id).await.unwrap();
    assert_eq!(
        teams.len(),
        1,
        "the Root Team is the team of the first account"
    );
    assert!(teams[0].team.is_root() && teams[0].team.personal_team);

    let (grace, personal) = account(db, "grace@example.com", Some("Grace Hopper")).await;
    assert_eq!(personal.name, "Grace Hopper's Team");
    assert!(personal.personal_team && !personal.is_root());
    assert_eq!(roles(db, personal.id).await, [(grace.id, TeamRole::Owner)]);
    assert!(!grace.is_super_admin);
    let (nameless, team) = account(db, "nameless@example.com", None).await;
    assert_eq!(team.name, "nameless@example.com's Team");

    // Who runs the instance is said by the account, and by no team: an
    // administrator of the Root Team runs that team only.
    Team::attach(&**db, Team::ROOT_ID, grace.id, TeamRole::Admin)
        .await
        .unwrap();
    Team::attach(&**db, Team::ROOT_ID, nameless.id, TeamRole::Member)
        .await
        .unwrap();
    for account in [&grace, &nameless] {
        let account = User::find(&**db, account.id).await.unwrap().unwrap();
        assert!(!account.is_super_admin);
    }
    assert_eq!(User::super_admin_count(&**db).await.unwrap(), 1);
    assert_eq!(Team::owner_count(&**db, Team::ROOT_ID).await.unwrap(), 1);
    assert!(
        Team::attach(&**db, Team::ROOT_ID, grace.id, TeamRole::Member)
            .await
            .is_err(),
        "nobody is in a team twice"
    );

    // Teams in the order they were joined, each with what the person is there.
    let teams: Vec<(i64, TeamRole)> = Team::memberships(&**db, grace.id)
        .await
        .unwrap()
        .into_iter()
        .map(|membership| (membership.team.id, membership.role))
        .collect();
    assert_eq!(
        teams,
        [
            (personal.id, TeamRole::Owner),
            (Team::ROOT_ID, TeamRole::Admin)
        ]
    );
    assert_eq!(
        Team::membership(&**db, Team::ROOT_ID, grace.id)
            .await
            .unwrap()
            .map(|membership| membership.role),
        Some(TeamRole::Admin)
    );
    assert!(
        Team::membership(&**db, personal.id, first.id)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn leaving_a_team_revokes_the_tokens_made_there_and_nothing_else() {
    let (core, _dir) = core().await;
    let db = &core.db;
    let first = admin(db).await;
    let (grace, personal) = account(db, "grace@example.com", Some("Grace Hopper")).await;
    Team::attach(&**db, Team::ROOT_ID, grace.id, TeamRole::Member)
        .await
        .unwrap();
    let shared = token_of(db, Team::ROOT_ID, grace.id, "shared").await;
    let own = token_of(db, personal.id, grace.id, "own").await;
    let other = token_of(db, Team::ROOT_ID, first.id, "other").await;
    let mut grace = grace;
    grace.remember_team(&**db, Team::ROOT_ID).await.unwrap();

    let mut transaction = db.begin().await.unwrap();
    Team::detach(&mut transaction, Team::ROOT_ID, grace.id)
        .await
        .unwrap();
    transaction.commit().await.unwrap();

    assert_eq!(
        roles(db, Team::ROOT_ID).await,
        [(first.id, TeamRole::Owner)]
    );
    let revoked = |token: &AccessToken| {
        let id = token.id;
        async move {
            AccessToken::find(&**db, id)
                .await
                .unwrap()
                .unwrap()
                .is_revoked()
        }
    };
    assert!(revoked(&shared).await);
    assert!(!revoked(&own).await);
    assert!(!revoked(&other).await);
    let grace = User::find(&**db, grace.id).await.unwrap().unwrap();
    assert_eq!(grace.current_team_id, None, "they no longer work there");
}

#[tokio::test]
async fn deleting_an_account_settles_the_teams_it_was_in() {
    let (core, _dir) = core().await;
    let db = &core.db;
    let first = admin(db).await;
    let (grace, personal) = account(db, "grace@example.com", Some("Grace Hopper")).await;
    let (linus, _) = account(db, "linus@example.com", Some("Linus T")).await;
    let (margaret, _) = account(db, "margaret@example.com", Some("Margaret H")).await;

    // A team Grace owns, with an administrator and a member who joined first.
    let mut transaction = db.begin().await.unwrap();
    let shared = Team::create_owned(&mut transaction, "Shared", None, &grace)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    Team::attach(&**db, shared.id, linus.id, TeamRole::Member)
        .await
        .unwrap();
    Team::attach(&**db, shared.id, margaret.id, TeamRole::Admin)
        .await
        .unwrap();
    let kept = mcp_of(db, shared.id, grace.id, "Kept").await;
    let token = token_of(db, shared.id, grace.id, "agent").await;
    let lost = mcp_of(db, personal.id, grace.id, "Lost").await;
    // And a row she made in a team she has left since.
    Team::attach(&**db, Team::ROOT_ID, grace.id, TeamRole::Member)
        .await
        .unwrap();
    let orphan = mcp_of(db, Team::ROOT_ID, grace.id, "Orphan").await;
    let mut transaction = db.begin().await.unwrap();
    Team::detach(&mut transaction, Team::ROOT_ID, grace.id)
        .await
        .unwrap();
    transaction.commit().await.unwrap();

    let mut transaction = db.begin().await.unwrap();
    assert_eq!(
        Team::delete_account(&mut transaction, grace.id)
            .await
            .unwrap(),
        Ok(())
    );
    transaction.commit().await.unwrap();

    assert!(User::find(&**db, grace.id).await.unwrap().is_none());
    // Her personal team went with her, and what it had.
    assert!(Team::find(&**db, personal.id).await.unwrap().is_none());
    assert!(Mcp::find(&**db, lost.id).await.unwrap().is_none());
    // The team she shared stays, owned by its administrator rather than by
    // the member who joined before them.
    assert_eq!(
        roles(db, shared.id).await,
        [(linus.id, TeamRole::Member), (margaret.id, TeamRole::Owner)]
    );
    let kept = Mcp::find(&**db, kept.id).await.unwrap().unwrap();
    assert_eq!(kept.created_by, margaret.id);
    let token = AccessToken::find(&**db, token.id).await.unwrap().unwrap();
    assert!(
        token.is_revoked(),
        "a token acts for the person who made it"
    );
    assert_eq!(token.created_by, margaret.id);
    let orphan = Mcp::find(&**db, orphan.id).await.unwrap().unwrap();
    assert_eq!(orphan.created_by, first.id);

    // The instance always keeps someone to run it.
    let mut transaction = db.begin().await.unwrap();
    assert_eq!(
        Team::delete_account(&mut transaction, first.id)
            .await
            .unwrap(),
        Err(mymcps_core::models::AccountRemovalRefusal::LastSuperAdmin)
    );
    transaction.rollback().await.unwrap();
    assert!(User::find(&**db, first.id).await.unwrap().is_some());

    // Once someone else runs it, the first account goes as any other does,
    // and the Root Team it was alone in goes with it.
    let mut transaction = db.begin().await.unwrap();
    assert!(
        User::set_super_admin(&mut transaction, margaret.id, true)
            .await
            .unwrap()
    );
    assert_eq!(
        Team::delete_account(&mut transaction, first.id)
            .await
            .unwrap(),
        Ok(())
    );
    transaction.commit().await.unwrap();
    assert!(User::find(&**db, first.id).await.unwrap().is_none());
    assert!(Team::find(&**db, Team::ROOT_ID).await.unwrap().is_none());
    assert!(Mcp::find(&**db, orphan.id).await.unwrap().is_none());
}

#[tokio::test]
async fn the_instance_always_keeps_a_super_admin() {
    let (core, _dir) = core().await;
    let db = &core.db;
    let first = admin(db).await;
    let (grace, _) = account(db, "grace@example.com", Some("Grace Hopper")).await;
    let is_super_admin = async |id: i64| {
        let user = User::find(&**db, id).await.unwrap().unwrap();
        user.is_super_admin
    };

    // The only one stays one.
    let mut transaction = db.begin().await.unwrap();
    assert!(
        !User::set_super_admin(&mut transaction, first.id, false)
            .await
            .unwrap()
    );
    transaction.commit().await.unwrap();
    assert!(is_super_admin(first.id).await);

    // With a second one, either can stop being one, and what they are in
    // their teams does not change.
    let mut transaction = db.begin().await.unwrap();
    assert!(
        User::set_super_admin(&mut transaction, grace.id, true)
            .await
            .unwrap()
    );
    assert!(
        User::set_super_admin(&mut transaction, first.id, false)
            .await
            .unwrap()
    );
    transaction.commit().await.unwrap();
    assert!(is_super_admin(grace.id).await && !is_super_admin(first.id).await);
    assert_eq!(User::super_admin_count(&**db).await.unwrap(), 1);
    assert_eq!(
        Team::role_of(&**db, Team::ROOT_ID, first.id).await.unwrap(),
        Some(TeamRole::Owner)
    );
    assert_eq!(
        Team::role_of(&**db, Team::ROOT_ID, grace.id).await.unwrap(),
        None
    );

    let mut transaction = db.begin().await.unwrap();
    assert!(
        !User::set_super_admin(&mut transaction, grace.id, false)
            .await
            .unwrap()
    );
    transaction.commit().await.unwrap();
    assert!(is_super_admin(grace.id).await);
}

async fn tailnet_of(db: &Db, team_id: i64, created_by: i64, name: &str) -> Tailnet {
    Tailnet::create_for_tests(&**db, team_id, created_by, name).await
}

#[tokio::test]
async fn a_team_connects_tailnets_that_its_mcps_use() {
    let (core, _dir) = core().await;
    let db = &core.db;
    let ada = admin(db).await;
    let (bob, bob_team) = account(db, "bob@example.com", Some("Bob")).await;

    let work = tailnet_of(db, Team::ROOT_ID, ada.id, "work").await;
    let mut lab = tailnet_of(db, Team::ROOT_ID, ada.id, "Lab").await;
    let other = tailnet_of(db, bob_team.id, bob.id, "work").await;
    assert!(work.is_persisted() && work.id > 0);
    assert!(work.created_at.to_string().len() > 10 && work.updated_at.is_some());
    assert_eq!(work.control_url, None);

    // Listed by team and by name, whatever the case; counted per team.
    let names = |tailnets: Vec<Tailnet>| -> Vec<String> {
        tailnets.into_iter().map(|tailnet| tailnet.name).collect()
    };
    assert_eq!(
        names(Tailnet::of_team(&**db, Team::ROOT_ID).await.unwrap()),
        ["Lab", "work"]
    );
    assert_eq!(
        names(Tailnet::of_team(&**db, bob_team.id).await.unwrap()),
        ["work"]
    );
    assert_eq!(
        Tailnet::count_in_team(&**db, Team::ROOT_ID).await.unwrap(),
        2
    );
    assert_eq!(Tailnet::count_in_team(&**db, bob_team.id).await.unwrap(), 1);
    assert_eq!(Tailnet::count_in_team(&**db, 999).await.unwrap(), 0);

    // Found by id in its own team only.
    let found = Tailnet::find_in_team(&**db, Team::ROOT_ID, work.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.hostname, "mymcps-work");
    assert!(
        Tailnet::find_in_team(&**db, bob_team.id, work.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        Tailnet::find_in_team(&**db, Team::ROOT_ID, other.id)
            .await
            .unwrap()
            .is_none()
    );

    // A name is unique in a team, and free in another.
    let mut duplicate = Tailnet::for_tests(Team::ROOT_ID, ada.id, "work");
    assert!(duplicate.insert(&**db).await.is_err());
    // Whatever its case: lookups by name ignore it, so the index does too.
    for name in ["WORK", "Work", "lab"] {
        let mut variant = Tailnet::for_tests(Team::ROOT_ID, ada.id, name);
        let error = variant.insert(&**db).await.unwrap_err();
        assert!(
            error
                .as_database_error()
                .is_some_and(|error| error.is_unique_violation()),
            "{name}: {error}"
        );
    }
    assert_eq!(
        Tailnet::count_in_team(&**db, Team::ROOT_ID).await.unwrap(),
        2
    );

    // Only the columns that changed are written.
    lab.control_url = Some("https://headscale.example.com".into());
    lab.save(&**db).await.unwrap();
    let lab = Tailnet::find(&**db, lab.id).await.unwrap().unwrap();
    assert_eq!(
        lab.control_url.as_deref(),
        Some("https://headscale.example.com")
    );
    assert_eq!(lab.name, "Lab");

    // MCPs reach their upstream through one tailnet, or none.
    let mut tagged = mcp_of(db, Team::ROOT_ID, ada.id, "Wiki").await;
    tagged.tailnet_id = Some(work.id);
    tagged.save(&**db).await.unwrap();
    let mut second = mcp_of(db, Team::ROOT_ID, ada.id, "Build").await;
    second.tailnet_id = Some(work.id);
    second.save(&**db).await.unwrap();
    let direct = mcp_of(db, Team::ROOT_ID, ada.id, "Direct").await;
    assert_eq!(direct.tailnet_id, None);
    let direct = Mcp::find(&**db, direct.id).await.unwrap().unwrap();
    assert_eq!(direct.tailnet_id, None);

    let using = work.mcps(&**db).await.unwrap();
    assert_eq!(
        using
            .iter()
            .map(|mcp| mcp.name.as_str())
            .collect::<Vec<_>>(),
        ["Build", "Wiki"]
    );
    assert_eq!(using[1].tailnet_id, Some(work.id));
    assert_eq!(work.mcp_count(&**db).await.unwrap(), 2);
    assert_eq!(lab.mcp_count(&**db).await.unwrap(), 0);
    assert!(lab.mcps(&**db).await.unwrap().is_empty());

    // A tailnet that does not exist cannot be used.
    let mut stray = mcp_of(db, Team::ROOT_ID, ada.id, "Stray").await;
    stray.tailnet_id = Some(9999);
    assert!(stray.save(&**db).await.is_err());

    // Deleting a tailnet leaves its MCPs, on the direct network.
    sqlx::query("delete from `tailnets` where `id` = ?")
        .bind(work.id)
        .execute(&**db)
        .await
        .unwrap();
    let wiki = Mcp::find(&**db, tagged.id).await.unwrap().unwrap();
    assert_eq!(wiki.tailnet_id, None);
    assert_eq!(wiki.name, "Wiki");

    // Deleting a team deletes its tailnets, not those of other teams.
    let mut kept = mcp_of(db, bob_team.id, bob.id, "Kept").await;
    kept.tailnet_id = Some(other.id);
    kept.save(&**db).await.unwrap();
    sqlx::query("delete from `teams` where `id` = ?")
        .bind(Team::ROOT_ID)
        .execute(&**db)
        .await
        .unwrap();
    assert_eq!(
        Tailnet::count_in_team(&**db, Team::ROOT_ID).await.unwrap(),
        0
    );
    assert_eq!(Tailnet::count_in_team(&**db, bob_team.id).await.unwrap(), 1);
    assert_eq!(other.mcp_count(&**db).await.unwrap(), 1);
}

#[tokio::test]
async fn a_tailnet_signs_out_its_mcps_when_its_node_runs_in_another_tailnet() {
    let (core, _dir) = core().await;
    let db = &core.db;
    let ada = admin(db).await;
    let work = tailnet_of(db, Team::ROOT_ID, ada.id, "work").await;
    let lab = tailnet_of(db, Team::ROOT_ID, ada.id, "lab").await;
    let signed_in = |name: &'static str, tailnet: &Tailnet| {
        let tailnet_id = tailnet.id;
        async move {
            let mut mcp = mcp_of(db, Team::ROOT_ID, ada.id, name).await;
            mcp.tailnet_id = Some(tailnet_id);
            mcp.auth_bearer = Some("bearer-ciphertext".into());
            mcp.auth_header_value = Some("header-ciphertext".into());
            mcp.oauth_client_id = Some("client".into());
            mcp.oauth_access_token = Some("access-ciphertext".into());
            mcp.oauth_refresh_token = Some("refresh-ciphertext".into());
            mcp.oauth_token_url = Some("https://auth.example/token".into());
            mcp.status = McpStatus::Ready;
            mcp.renew_oauth_generation();
            mcp.save(&**db).await.unwrap();
            mcp
        }
    };
    let wiki = signed_in("Wiki", &work).await;
    let builds = signed_in("Builds", &lab).await;
    // An MCP of the same tailnet that needs no sign-in.
    let mut open = mcp_of(db, Team::ROOT_ID, ada.id, "Open").await;
    open.tailnet_id = Some(work.id);
    open.status = McpStatus::Ready;
    open.save(&**db).await.unwrap();
    let find = |id| async move { Mcp::find(&**db, id).await.unwrap().unwrap() };

    // First found running: remembered, in lowercase, without its dots.
    assert_eq!(
        work.settle_identity(db, ".Tail1234.ts.net.").await.unwrap(),
        TailnetIdentity::Bound
    );
    let work = Tailnet::find(&**db, work.id).await.unwrap().unwrap();
    assert_eq!(work.magic_dns_suffix.as_deref(), Some("tail1234.ts.net"));
    assert_eq!(
        work.settle_identity(db, "tail1234.ts.net").await.unwrap(),
        TailnetIdentity::Same
    );
    // A copy read before it was remembered finds it settled the same way.
    let stale = Tailnet::for_tests(Team::ROOT_ID, ada.id, "work");
    let stale = Tailnet {
        id: work.id,
        ..stale
    };
    assert_eq!(
        stale.settle_identity(db, "tail1234.ts.net").await.unwrap(),
        TailnetIdentity::Same
    );
    assert_eq!(find(wiki.id).await.auth_bearer, wiki.auth_bearer);

    // Another tailnet: its MCPs lose what was entered for the first one.
    assert_eq!(
        work.settle_identity(db, "tail9999.ts.net").await.unwrap(),
        TailnetIdentity::Changed { mcps: 2 }
    );
    let signed_out = find(wiki.id).await;
    // Signed in with OAuth, it waits for its account again, so Connect shows.
    assert!(signed_out.oauth_required);
    assert!(!find(open.id).await.oauth_required);
    assert_eq!(signed_out.auth_bearer, None);
    assert_eq!(signed_out.auth_header_value, None);
    assert_eq!(signed_out.oauth_client_id, None);
    assert_eq!(signed_out.oauth_access_token, None);
    assert_eq!(signed_out.oauth_refresh_token, None);
    assert_eq!(signed_out.oauth_token_url, None);
    assert_ne!(signed_out.oauth_generation, wiki.oauth_generation);
    assert!(signed_out.oauth_generation >= 0);
    assert_eq!(signed_out.status, McpStatus::Error);
    assert_eq!(
        signed_out.last_error.as_deref(),
        Some(SIGNED_OUT_FOR_ANOTHER_TAILNET)
    );
    assert_eq!(signed_out.tailnet_id, Some(work.id));
    let work_now = Tailnet::find(&**db, work.id).await.unwrap().unwrap();
    assert_eq!(
        work_now.magic_dns_suffix.as_deref(),
        Some("tail9999.ts.net")
    );
    // The MCPs of another tailnet keep theirs.
    let untouched = find(builds.id).await;
    assert_eq!(untouched.auth_bearer, builds.auth_bearer);
    assert_eq!(untouched.oauth_access_token, builds.oauth_access_token);
    assert_eq!(untouched.oauth_generation, builds.oauth_generation);
    assert_eq!(untouched.status, McpStatus::Ready);

    // A request that read the first tailnet after it moved finds the move
    // done, and signs nothing out twice.
    assert_eq!(
        work.settle_identity(db, "tail9999.ts.net").await.unwrap(),
        TailnetIdentity::Same
    );
}
