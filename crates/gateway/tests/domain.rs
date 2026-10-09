//! The cases of `tests/unit/domain.spec.ts` about access tokens.

mod support;

use chrono::Duration;
use mymcps_core::Timestamp;
use mymcps_core::models::ScopeMode;
use mymcps_gateway::access_token;
use support::*;

#[test]
fn generates_and_hashes_access_tokens() {
    let plaintext = access_token::generate_plaintext();

    let random = plaintext.strip_prefix("mcp_").unwrap();
    assert_eq!(random.len(), 43);
    assert!(
        random
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    );
    assert_eq!(access_token::prefix(&plaintext), &plaintext[..12]);
    assert_eq!(access_token::hash(&plaintext).len(), 64);
    assert_ne!(access_token::hash(&plaintext), plaintext);
    // The SHA-256 of the token, in hex, as existing databases hold it.
    assert_eq!(
        access_token::hash("mcp_example"),
        "a7fd7a2b18fd70d3f7f8cc393c89a1ea85de1929924cec88edeeea7c76e7874b"
    );

    let refresh_token = access_token::generate_refresh_token();
    assert_eq!(access_token::prefix(&refresh_token), "mcp_refresh_");
    assert_eq!(refresh_token.len(), 55);
}

#[tokio::test]
async fn creates_tokens_and_resolves_enabled_mcps_by_scope() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let enabled_mcp = create_mcp(db, admin.id, |mcp| mcp.name = "Enabled MCP".into()).await;
    let disabled_mcp = create_mcp(db, admin.id, |mcp| {
        mcp.name = "Disabled MCP".into();
        mcp.enabled = false;
    })
    .await;

    let all_token = create_access_token(db, admin.id, ScopeMode::All, &[]).await;
    let selected_token = create_access_token(
        db,
        admin.id,
        ScopeMode::Selected,
        &[enabled_mcp.id, disabled_mcp.id],
    )
    .await;

    let allowed_for_all_token = access_token::resolve_allowed_mcps(db, &all_token.token)
        .await
        .unwrap();
    let allowed_for_selected_token = access_token::resolve_allowed_mcps(db, &selected_token.token)
        .await
        .unwrap();

    let ids = |mcps: &[mymcps_core::models::Mcp]| mcps.iter().map(|mcp| mcp.id).collect::<Vec<_>>();
    assert_eq!(ids(&allowed_for_all_token), [enabled_mcp.id]);
    assert_eq!(ids(&allowed_for_selected_token), [enabled_mcp.id]);
    assert_eq!(
        all_token.token.token_hash,
        access_token::hash(&all_token.plaintext)
    );
    assert_eq!(
        all_token.token.token_prefix,
        access_token::prefix(&all_token.plaintext)
    );
}

#[tokio::test]
async fn lists_the_mcps_of_a_token_by_name_and_includes_those_added_later() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let zebra = create_mcp(db, admin.id, |mcp| mcp.name = "Zebra".into()).await;
    let apple = create_mcp(db, admin.id, |mcp| mcp.name = "Apple".into()).await;
    let other = create_mcp(db, admin.id, |mcp| mcp.name = "Mango".into()).await;

    let all_token = create_access_token(db, admin.id, ScopeMode::All, &[]).await;
    let selected_token =
        create_access_token(db, admin.id, ScopeMode::Selected, &[zebra.id, apple.id]).await;
    let later = create_mcp(db, admin.id, |mcp| mcp.name = "Banana".into()).await;

    let names = |mcps: Vec<mymcps_core::models::Mcp>| {
        mcps.into_iter().map(|mcp| mcp.name).collect::<Vec<_>>()
    };
    assert_eq!(
        names(
            access_token::resolve_allowed_mcps(db, &all_token.token)
                .await
                .unwrap()
        ),
        ["Apple", "Banana", "Mango", "Zebra"]
    );
    assert_eq!(
        names(
            access_token::resolve_allowed_mcps(db, &selected_token.token)
                .await
                .unwrap()
        ),
        ["Apple", "Zebra"]
    );
    let _ = (other, later);
}

#[tokio::test]
async fn finds_only_usable_access_tokens_and_throttles_last_used_writes() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let mut created = create_access_token(db, admin.id, ScopeMode::All, &[]).await;

    let found = access_token::find_usable_by_plaintext(db, &created.plaintext)
        .await
        .unwrap();
    assert_eq!(found.map(|token| token.id), Some(created.token.id));

    created.token.revoked_at = Some(Timestamp::now());
    created.token.save(&**db).await.unwrap();
    assert!(
        access_token::find_usable_by_plaintext(db, &created.plaintext)
            .await
            .unwrap()
            .is_none()
    );

    create_stored_access_token(db, admin.id, |token| {
        token.token_hash = access_token::hash("expired-token");
        token.expires_at = Some(Timestamp::now() - Duration::minutes(1));
    })
    .await;
    assert!(
        access_token::find_usable_by_plaintext(db, "expired-token")
            .await
            .unwrap()
            .is_none()
    );

    let mut recent = create_stored_access_token(db, admin.id, |token| {
        token.token_hash = "recent-token-hash".into();
        token.last_used_at = Some(Timestamp::now() - Duration::minutes(1));
    })
    .await;
    let last_used_at = recent.last_used_at;
    access_token::touch_last_used(db, &mut recent)
        .await
        .unwrap();
    assert_eq!(recent.last_used_at, last_used_at);
    assert_eq!(find_token(db, recent.id).await.last_used_at, last_used_at);

    let stale_last_used_at = Timestamp::now() - Duration::minutes(6);
    recent.last_used_at = Some(stale_last_used_at);
    access_token::touch_last_used(db, &mut recent)
        .await
        .unwrap();
    assert!(recent.last_used_at.unwrap() > stale_last_used_at);
    assert_eq!(
        find_token(db, recent.id).await.last_used_at,
        recent.last_used_at
    );

    // A token that was never used is marked on its first request.
    let mut unused = create_stored_access_token(db, admin.id, |token| {
        token.token_hash = "unused-token-hash".into();
    })
    .await;
    access_token::touch_last_used(db, &mut unused)
        .await
        .unwrap();
    assert!(find_token(db, unused.id).await.last_used_at.is_some());
}
