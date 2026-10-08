//! `tests/functional/access_tokens.spec.ts`, for what the Tokens page asks
//! of the access token service.

mod support;

use chrono::Duration;
use mymcps_core::models::{ScopeMode, TokenSource};
use mymcps_core::{Db, Timestamp};
use mymcps_gateway::access_token::{self, AccessTokenUpdate, NewAccessToken};
use support::*;

/// The MCPs a token was given, and when each was.
async fn token_mcps(db: &Db, token_id: i64) -> Vec<(i64, String)> {
    sqlx::query_as(
        "select `mcp_id`, `created_at` from `access_token_mcps` where `access_token_id` = ? order by `mcp_id` asc",
    )
    .bind(token_id)
    .fetch_all(&**db)
    .await
    .unwrap()
}

async fn token_mcp_ids(db: &Db, token_id: i64) -> Vec<i64> {
    token_mcps(db, token_id)
        .await
        .into_iter()
        .map(|(mcp_id, _)| mcp_id)
        .collect()
}

#[tokio::test]
async fn creates_a_token_and_shows_its_plaintext_only_once() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;

    let created = access_token::create(
        db,
        NewAccessToken {
            name: "Production agent",
            scope_mode: ScopeMode::All,
            mcp_ids: &[],
            expires_at: None,
            created_by: admin.id,
        },
    )
    .await
    .unwrap();

    let random = created.plaintext.strip_prefix("mcp_").unwrap();
    assert_eq!(random.len(), 43);
    assert!(
        random
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    );

    let token = find_token_by_name(db, "Production agent").await;
    assert_ne!(token.token_hash, created.plaintext);
    assert_eq!(token.token_hash, access_token::hash(&created.plaintext));
    assert_eq!(token.token_prefix, &created.plaintext[..12]);
    assert_eq!(token.source, TokenSource::Manual);
    assert_eq!(token.created_by, admin.id);
    assert!(!token.is_revoked());
    assert!(token.is_usable());
    // Nothing of the row holds the token itself.
    let row: String = sqlx::query_scalar(
        "select `name` || `token_prefix` || `token_hash` from `access_tokens` where `id` = ?",
    )
    .bind(token.id)
    .fetch_one(&**db)
    .await
    .unwrap();
    assert!(!row.contains(&created.plaintext));
}

#[tokio::test]
async fn gives_a_token_its_selected_mcps_only_when_its_scope_is_selected() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let first = create_mcp(db, admin.id, |_| {}).await;
    let second = create_mcp(db, admin.id, |_| {}).await;

    let selected =
        create_access_token(db, admin.id, ScopeMode::Selected, &[second.id, first.id]).await;
    assert_eq!(
        token_mcp_ids(db, selected.token.id).await,
        [first.id, second.id]
    );

    let all = create_access_token(db, admin.id, ScopeMode::All, &[first.id]).await;
    assert_eq!(token_mcp_ids(db, all.token.id).await, Vec::<i64>::new());

    // An MCP that is deleted leaves the tokens that had it.
    first.delete(&**db).await.unwrap();
    assert_eq!(token_mcp_ids(db, selected.token.id).await, [second.id]);
}

#[tokio::test]
async fn updates_token_settings_without_rotating_its_secret() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let first_mcp = create_mcp(db, admin.id, |mcp| mcp.name = "First MCP".into()).await;
    let second_mcp = create_mcp(db, admin.id, |mcp| mcp.name = "Second MCP".into()).await;
    let created =
        create_named_access_token(db, admin.id, "Original token", ScopeMode::All, &[], None).await;
    let original_hash = created.token.token_hash.clone();
    let original_prefix = created.token.token_prefix.clone();
    let expires_at = Timestamp::now() + Duration::days(7);

    let mut token = find_token(db, created.token.id).await;
    access_token::update(
        db,
        &mut token,
        AccessTokenUpdate {
            name: "Updated token",
            scope_mode: ScopeMode::Selected,
            mcp_ids: &[first_mcp.id, second_mcp.id],
            expires_at: Some(expires_at),
        },
    )
    .await
    .unwrap();

    let updated = find_token(db, created.token.id).await;
    assert_eq!(updated.name, "Updated token");
    assert_eq!(updated.scope_mode, ScopeMode::Selected);
    assert_eq!(updated.expires_at, Some(expires_at));
    assert_eq!(
        token_mcp_ids(db, updated.id).await,
        [first_mcp.id, second_mcp.id]
    );
    assert_eq!(updated.token_hash, original_hash);
    assert_eq!(updated.token_prefix, original_prefix);
    assert_eq!(updated.created_by, admin.id);
    assert_eq!(updated.revoked_at, None);
    assert!(
        access_token::find_usable_by_plaintext(db, &created.plaintext)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn clears_selected_mcps_and_reactivates_an_expired_token() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = create_mcp(db, admin.id, |_| {}).await;
    let created = create_named_access_token(
        db,
        admin.id,
        "Expired token",
        ScopeMode::Selected,
        &[mcp.id],
        Some(Timestamp::now() - Duration::days(1)),
    )
    .await;
    assert!(!created.token.is_usable());

    let mut token = find_token(db, created.token.id).await;
    access_token::update(
        db,
        &mut token,
        AccessTokenUpdate {
            name: "Reactivated token",
            scope_mode: ScopeMode::All,
            // The form may still send the MCPs that were selected.
            mcp_ids: &[mcp.id],
            expires_at: None,
        },
    )
    .await
    .unwrap();

    let updated = find_token(db, created.token.id).await;
    assert_eq!(updated.scope_mode, ScopeMode::All);
    assert_eq!(updated.expires_at, None);
    assert_eq!(token_mcp_ids(db, updated.id).await, Vec::<i64>::new());
    assert!(updated.is_usable());
}

#[tokio::test]
async fn keeps_the_mcps_a_token_already_had_when_others_are_added_or_removed() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let kept = create_mcp(db, admin.id, |_| {}).await;
    let removed = create_mcp(db, admin.id, |_| {}).await;
    let added = create_mcp(db, admin.id, |_| {}).await;
    let created =
        create_access_token(db, admin.id, ScopeMode::Selected, &[kept.id, removed.id]).await;
    sqlx::query("update `access_token_mcps` set `created_at` = '2020-01-01 00:00:00'")
        .execute(&**db)
        .await
        .unwrap();

    let mut token = find_token(db, created.token.id).await;
    access_token::update(
        db,
        &mut token,
        AccessTokenUpdate {
            name: &created.token.name,
            scope_mode: ScopeMode::Selected,
            mcp_ids: &[added.id, kept.id, added.id],
            expires_at: None,
        },
    )
    .await
    .unwrap();

    let mcps = token_mcps(db, token.id).await;
    assert_eq!(
        mcps.iter().map(|(mcp_id, _)| *mcp_id).collect::<Vec<_>>(),
        [kept.id, added.id]
    );
    assert_eq!(mcps[0].1, "2020-01-01 00:00:00");
    assert_ne!(mcps[1].1, "2020-01-01 00:00:00");
}

#[tokio::test]
async fn changes_nothing_when_a_selected_mcp_does_not_exist() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mcp = create_mcp(db, admin.id, |_| {}).await;
    let created =
        create_named_access_token(db, admin.id, "Unchanged token", ScopeMode::All, &[], None).await;

    // The Tokens page refuses such a selection before it gets here; the
    // database refuses it too, and the update is undone as a whole.
    let mut token = find_token(db, created.token.id).await;
    let refused = access_token::update(
        db,
        &mut token,
        AccessTokenUpdate {
            name: "Rejected update",
            scope_mode: ScopeMode::Selected,
            mcp_ids: &[mcp.id, 999_999],
            expires_at: None,
        },
    )
    .await;
    assert!(refused.is_err());

    let unchanged = find_token(db, created.token.id).await;
    assert_eq!(unchanged.name, "Unchanged token");
    assert_eq!(unchanged.scope_mode, ScopeMode::All);
    assert_eq!(token_mcp_ids(db, unchanged.id).await, Vec::<i64>::new());
}

#[tokio::test]
async fn revokes_a_token() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let created =
        create_named_access_token(db, admin.id, "Revocable agent", ScopeMode::All, &[], None).await;
    let mut token = find_token_by_name(db, "Revocable agent").await;

    access_token::revoke(db, &mut token).await.unwrap();

    let revoked_token = find_token(db, token.id).await;
    assert!(revoked_token.is_revoked());
    assert!(!revoked_token.is_usable());
    assert!(
        access_token::find_usable_by_plaintext(db, &created.plaintext)
            .await
            .unwrap()
            .is_none()
    );
}
