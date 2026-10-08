use std::time::Duration;

use mymcps_core::limiter::{Limiter, LimiterError};
use mymcps_core::models::{
    AccessToken, ApprovalDecision, ApprovalRequest, ApprovalState, InstanceSetting, Invite, Mcp,
    McpLogLevel, McpStatus, McpTransport, OauthClient, User, UserRole,
};
use mymcps_core::{Config, Core, Db, Timestamp};

async fn core() -> (std::sync::Arc<Core>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::boot(Config::for_tests(dir.path())).await.unwrap();
    (core, dir)
}

async fn admin(db: &Db) -> User {
    let mut user = User::with_password(
        "admin@example.com",
        Some("Ada Lovelace"),
        "secret-password",
        UserRole::Admin,
    )
    .await
    .unwrap();
    user.insert(&**db).await.unwrap();
    user
}

#[tokio::test]
async fn users_sign_in_and_change_their_password() {
    let (core, _dir) = core().await;
    let db = &core.db;
    assert!(!User::setup_complete(&**db).await.unwrap());

    let mut user = admin(db).await;
    assert!(user.id > 0);
    assert!(user.is_admin());
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
    mcp.save(&**db).await.unwrap();
    let reloaded = Mcp::find(&**db, mcp.id).await.unwrap().unwrap();
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
