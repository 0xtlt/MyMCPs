//! The scopes of an OAuth grant: `mcp:tools` always, and `mcps:manage` only
//! when the person checks it on the consent screen.

mod support;

use mymcps_core::models::{ScopeMode, User};
use mymcps_gateway::access_token;
use mymcps_gateway::access_token::CreatedOauthTokens;
use mymcps_gateway::bearer::{BearerError, BearerRejection};
use mymcps_gateway::oauth::{GatewayAuthorizationRequest, oauth_token_response};
use serde_json::{Value, json};
use support::*;

const REDIRECT_URI: &str = "http://127.0.0.1:49152/callback";
const CODE_VERIFIER: &str = "oauth-code-verifier-for-mymcps-tests-1234567890";

async fn register_client(gateway: &TestGateway, scope: Option<&str>) -> String {
    let metadata = with(
        &registration("Scoped client", REDIRECT_URI),
        &[("scope", scope.map(|scope| json!(scope)))],
    );
    register(gateway, &metadata).await["client_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn parse(
    gateway: &TestGateway,
    client_id: &str,
    scope: Option<&str>,
) -> GatewayAuthorizationRequest {
    let payload = with(
        &authorization(client_id, REDIRECT_URI, &code_challenge(CODE_VERIFIER)),
        &[("scope", scope.map(|scope| json!(scope)))],
    );
    gateway
        .oauth
        .parse_authorization_request(&payload)
        .await
        .unwrap()
}

/// The consent screen answered by `user`, and the code exchanged for tokens.
async fn grant(
    gateway: &TestGateway,
    client_id: &str,
    requested: Option<&str>,
    user: &User,
    team_id: i64,
    manage_mcps: bool,
) -> CreatedOauthTokens {
    let request = parse(gateway, client_id, requested).await;
    let code = gateway
        .oauth
        .create_authorization_code(&request, user.id, team_id, manage_mcps)
        .await
        .unwrap();
    gateway
        .oauth
        .issue_tokens(
            None,
            &json!({
                "grant_type": "authorization_code",
                "client_id": client_id,
                "code": code,
                "code_verifier": CODE_VERIFIER,
                "redirect_uri": REDIRECT_URI,
                "resource": RESOURCE,
            }),
        )
        .await
        .unwrap()
}

fn refresh(client_id: &str, refresh_token: &str, scope: Option<&str>) -> Value {
    with(
        &json!({
            "grant_type": "refresh_token",
            "client_id": client_id,
            "refresh_token": refresh_token,
            "resource": RESOURCE,
        }),
        &[("scope", scope.map(|scope| json!(scope)))],
    )
}

async fn accepts(gateway: &TestGateway, access_token: &str) -> bool {
    match gateway
        .authenticate_bearer(Some(&format!("Bearer {access_token}")))
        .await
    {
        Ok(_) => true,
        Err(BearerError::Rejected(BearerRejection::Invalid)) => false,
        Err(other) => panic!("unexpected answer: {other:?}"),
    }
}

#[tokio::test]
async fn grants_only_the_gateway_scope_unless_the_permission_is_checked() {
    let gateway = TestGateway::new().await;
    let admin = create_admin(gateway.db()).await;
    let client_id = register_client(&gateway, None).await;

    let plain = grant(&gateway, &client_id, None, &admin, Team::ROOT_ID, false).await;
    assert_eq!(oauth_token_response(&plain)["scope"], "mcp:tools");
    assert_eq!(plain.token.oauth_scopes.as_deref(), Some("mcp:tools"));
    assert!(!access_token::grants_mcp_management(&plain.token));

    let managing = grant(&gateway, &client_id, None, &admin, Team::ROOT_ID, true).await;
    assert_eq!(
        oauth_token_response(&managing)["scope"],
        "mcp:tools mcps:manage"
    );
    let stored = find_token(gateway.db(), managing.token.id).await;
    assert_eq!(
        stored.oauth_scopes.as_deref(),
        Some("mcp:tools mcps:manage")
    );
    assert!(access_token::grants_mcp_management(&stored));
}

#[tokio::test]
async fn refuses_the_permission_to_someone_who_is_not_an_admin_of_the_team() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let member = create_user(db, "member", TeamRole::Member).await;
    let admin = create_admin(db).await;
    let client_id = register_client(&gateway, None).await;
    let request = parse(&gateway, &client_id, None).await;

    let refused = gateway
        .oauth
        .create_authorization_code(&request, member.id, Team::ROOT_ID, true)
        .await;
    let refusal = oauth_error(refused);
    assert_eq!((refusal.status, refusal.code), (400, "invalid_request"));
    assert_eq!(
        refusal.message,
        "Only team admins and owners can let an agent manage MCPs"
    );
    assert_eq!(refusal.redirect_uri.as_deref(), Some(REDIRECT_URI));
    assert_eq!(refusal.state.as_deref(), Some("state-from-client"));
    assert_eq!(count(db, "oauth_authorization_codes").await, 0);

    // Being an admin elsewhere is not being one of this team.
    let other_team = create_team(db).await;
    Team::attach(&**db, other_team.id, admin.id, TeamRole::Member)
        .await
        .unwrap();
    let refused = gateway
        .oauth
        .create_authorization_code(&request, admin.id, other_team.id, true)
        .await;
    assert_eq!(oauth_error(refused).code, "invalid_request");
    assert_eq!(count(db, "oauth_authorization_codes").await, 0);

    // Without the permission a member is let through as before.
    gateway
        .oauth
        .create_authorization_code(&request, member.id, Team::ROOT_ID, false)
        .await
        .unwrap();
    assert_eq!(count(db, "oauth_authorization_codes").await, 1);
}

#[tokio::test]
async fn does_not_grant_the_permission_a_client_asks_for() {
    let gateway = TestGateway::new().await;
    let admin = create_admin(gateway.db()).await;

    let registered = register(
        &gateway,
        &with(
            &registration("Greedy client", REDIRECT_URI),
            &[("scope", Some(json!("mcps:manage mcp:tools")))],
        ),
    )
    .await;
    // Recorded and answered the way grants list them.
    assert_eq!(registered["scope"], "mcp:tools mcps:manage");
    let client_id = registered["client_id"].as_str().unwrap();
    assert_eq!(
        find_client(gateway.db(), client_id).await.scope,
        "mcp:tools mcps:manage"
    );

    let request = parse(&gateway, client_id, Some("mcps:manage mcp:tools")).await;
    assert_eq!(request.scopes, "mcp:tools mcps:manage");

    // The admin answers without ticking the box.
    let created = grant(
        &gateway,
        client_id,
        Some("mcp:tools mcps:manage"),
        &admin,
        Team::ROOT_ID,
        false,
    )
    .await;
    assert_eq!(created.token.oauth_scopes.as_deref(), Some("mcp:tools"));
    assert_eq!(oauth_token_response(&created)["scope"], "mcp:tools");
}

#[tokio::test]
async fn refuses_a_scope_the_gateway_does_not_know() {
    let gateway = TestGateway::new().await;
    let client_id = register_client(&gateway, None).await;

    for scope in [
        "other",
        "mcp:tools other",
        "MCPS:MANAGE",
        "mcps:manage mcps:manage",
    ] {
        let refused = gateway
            .oauth
            .register_client(&with(
                &registration("Client", REDIRECT_URI),
                &[("scope", Some(json!(scope)))],
            ))
            .await;
        assert_oauth_error(
            refused,
            400,
            "invalid_client_metadata",
            "Unsupported OAuth scope",
        );

        let payload = with(
            &authorization(&client_id, REDIRECT_URI, &code_challenge(CODE_VERIFIER)),
            &[("scope", Some(json!(scope)))],
        );
        let refused = gateway.oauth.parse_authorization_request(&payload).await;
        assert_eq!(oauth_error(refused).code, "invalid_scope");
    }
}

#[tokio::test]
async fn a_refresh_keeps_the_scopes_of_its_grant_and_never_widens_them() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let client_id = register_client(&gateway, Some("mcp:tools mcps:manage")).await;

    let managing = grant(&gateway, &client_id, None, &admin, Team::ROOT_ID, true).await;
    let mut refresh_token = managing.refresh_token.unwrap();
    // Left out, repeated in part, repeated in full, or in another order:
    // the grant is the same afterwards.
    for scope in [
        None,
        Some("mcp:tools"),
        Some("mcps:manage"),
        Some("mcp:tools mcps:manage"),
        Some("mcps:manage mcp:tools"),
    ] {
        let refreshed = gateway
            .oauth
            .issue_tokens(None, &refresh(&client_id, &refresh_token, scope))
            .await
            .unwrap();
        assert_eq!(
            oauth_token_response(&refreshed)["scope"],
            "mcp:tools mcps:manage"
        );
        assert_eq!(
            find_token(db, managing.token.id)
                .await
                .oauth_scopes
                .as_deref(),
            Some("mcp:tools mcps:manage")
        );
        refresh_token = refreshed.refresh_token.unwrap();
    }

    let plain = grant(&gateway, &client_id, None, &admin, Team::ROOT_ID, false).await;
    let refresh_token = plain.refresh_token.unwrap();
    for scope in [Some("mcps:manage"), Some("mcp:tools mcps:manage")] {
        let refused = gateway
            .oauth
            .issue_tokens(None, &refresh(&client_id, &refresh_token, scope))
            .await;
        assert_oauth_error(refused, 400, "invalid_scope", "Unsupported OAuth scope");
    }
    for scope in ["other", "mcp:tools other", "mcp:tools mcp:tools"] {
        let refused = gateway
            .oauth
            .issue_tokens(None, &refresh(&client_id, &refresh_token, Some(scope)))
            .await;
        assert_oauth_error(refused, 400, "invalid_scope", "Unsupported OAuth scope");
    }

    // None of those refusals used up the refresh token.
    let refreshed = gateway
        .oauth
        .issue_tokens(
            None,
            &refresh(&client_id, &refresh_token, Some("mcp:tools")),
        )
        .await
        .unwrap();
    assert_eq!(oauth_token_response(&refreshed)["scope"], "mcp:tools");
    assert_eq!(
        find_token(db, plain.token.id).await.oauth_scopes.as_deref(),
        Some("mcp:tools")
    );
}

#[tokio::test]
async fn lets_a_grant_in_when_it_holds_the_gateway_scope_and_only_known_scopes() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let client_id = register_client(&gateway, None).await;
    let created = grant(&gateway, &client_id, None, &admin, Team::ROOT_ID, true).await;
    let mut token = find_token(db, created.token.id).await;

    assert!(accepts(&gateway, &created.plaintext).await);

    for (scopes, accepted) in [
        ("mcp:tools", true),
        ("mcp:tools mcps:manage", true),
        ("mcps:manage", false),
        ("mcp:tools other", false),
        ("mcp:tools mcps:manage other", false),
        ("other", false),
        ("", false),
    ] {
        token.oauth_scopes = Some(scopes.to_owned());
        token.save(&**db).await.unwrap();
        assert_eq!(
            accepts(&gateway, &created.plaintext).await,
            accepted,
            "{scopes:?}"
        );
    }

    token.oauth_scopes = None;
    token.save(&**db).await.unwrap();
    assert!(!accepts(&gateway, &created.plaintext).await);
}

#[tokio::test]
async fn takes_the_permission_off_a_grant_without_ending_the_connection() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let client_id = register_client(&gateway, None).await;
    let created = grant(&gateway, &client_id, None, &admin, Team::ROOT_ID, true).await;

    let mut token = find_token(db, created.token.id).await;
    assert!(
        access_token::remove_mcp_management(db, &mut token)
            .await
            .unwrap()
    );
    assert_eq!(token.oauth_scopes.as_deref(), Some("mcp:tools"));
    let stored = find_token(db, created.token.id).await;
    assert_eq!(stored.oauth_scopes.as_deref(), Some("mcp:tools"));
    assert!(!access_token::grants_mcp_management(&stored));
    assert!(stored.is_active());

    // The connection works, and refreshes with what is left.
    assert!(accepts(&gateway, &created.plaintext).await);
    let refreshed = gateway
        .oauth
        .issue_tokens(
            None,
            &refresh(&client_id, created.refresh_token.as_deref().unwrap(), None),
        )
        .await
        .unwrap();
    assert_eq!(oauth_token_response(&refreshed)["scope"], "mcp:tools");

    // There is nothing more to take off.
    let mut token = find_token(db, created.token.id).await;
    assert!(
        !access_token::remove_mcp_management(db, &mut token)
            .await
            .unwrap()
    );

    // A manual token never had it.
    let manual = create_access_token(db, admin.id, ScopeMode::All, &[]).await;
    let mut manual = find_token(db, manual.token.id).await;
    assert!(!access_token::grants_mcp_management(&manual));
    assert!(
        !access_token::remove_mcp_management(db, &mut manual)
            .await
            .unwrap()
    );
}
