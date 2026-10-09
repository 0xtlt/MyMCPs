//! `tests/functional/vine_gateway_oauth.spec.ts`, at the level of the
//! functions the endpoints call.
//!
//! The parameters of a request are given the way the web layer hands them
//! over: a parameter sent twice is a list, and one left out is missing. An
//! empty parameter is an empty string in a query string and `null` in a
//! body, where it reads as left out.

mod support;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use mymcps_core::models::{OauthClient, User};
use mymcps_gateway::access_token::{self, NewOauthGrant};
use mymcps_gateway::oauth::{GatewayAuthorizationRequest, oauth_redirect};
use mymcps_gateway::validators::gateway_oauth::CONSENT_APPROVAL;
use mymcps_gateway::{Error, GatewayOauthError};
use serde_json::{Value, json};
use support::*;
use url::Url;

const LOOPBACK_REDIRECT_URI: &str = "http://127.0.0.1:49152/callback";
const REMOTE_REDIRECT_URI: &str = "https://client.example/callback";
const CODE_VERIFIER: &str = "vine-oauth-code-verifier-for-mymcps-gateway-tests-123";

fn registration_metadata() -> Value {
    registration("Vine client", "http://127.0.0.1/callback")
}

async fn register_with(gateway: &TestGateway, overrides: &[(&str, Option<Value>)]) -> OauthClient {
    let registered = register(gateway, &with(&registration_metadata(), overrides)).await;
    find_client(gateway.db(), registered["client_id"].as_str().unwrap()).await
}

fn authorization_for(client_id: &str, redirect_uri: &str) -> Value {
    authorization(client_id, redirect_uri, &code_challenge(CODE_VERIFIER))
}

async fn authorize(
    gateway: &TestGateway,
    base: &Value,
    changes: &[(&str, Option<Value>)],
) -> Result<GatewayAuthorizationRequest, Error> {
    gateway
        .oauth
        .parse_authorization_request(&with(base, changes))
        .await
}

/// The error a loopback client is sent back with, as (error, description, state).
fn redirected_error(
    result: Result<GatewayAuthorizationRequest, Error>,
) -> (String, String, Option<String>) {
    let error: GatewayOauthError = oauth_error(result);
    assert_eq!(error.status, 400);
    assert_eq!(error.redirect_uri.as_deref(), Some(LOOPBACK_REDIRECT_URI));
    let callback = Url::parse(
        &oauth_redirect(
            LOOPBACK_REDIRECT_URI,
            &[
                ("error", Some(error.code)),
                ("error_description", Some(&error.message)),
                ("state", error.state.as_deref()),
            ],
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        format!(
            "{}{}",
            callback.origin().ascii_serialization(),
            callback.path()
        ),
        LOOPBACK_REDIRECT_URI
    );
    let parameter = |name: &str| {
        callback
            .query_pairs()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.into_owned())
    };
    (
        parameter("error").unwrap(),
        parameter("error_description").unwrap(),
        parameter("state"),
    )
}

fn basic(id: &str, secret: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{id}:{secret}")))
}

// vine: gateway OAuth client registration

#[tokio::test]
async fn answers_with_the_first_rule_a_registration_breaks() {
    let gateway = TestGateway::new().await;
    let wrong = [
        ("redirect_uris", json!(["http://example.com/callback"])),
        ("token_endpoint_auth_method", json!("private_key_jwt")),
        ("grant_types", json!(["implicit"])),
        ("response_types", json!(["token"])),
        ("scope", json!("other")),
        ("client_name", json!("x".repeat(121))),
    ];
    let expected = [
        (
            "invalid_redirect_uri",
            "Redirect URIs must use HTTPS, HTTP on an exact loopback host, or an approved native-app callback",
        ),
        (
            "invalid_client_metadata",
            "Unsupported token endpoint authentication method",
        ),
        ("invalid_client_metadata", "Unsupported OAuth grant type"),
        (
            "invalid_client_metadata",
            "Only the code response type is supported",
        ),
        ("invalid_client_metadata", "Unsupported OAuth scope"),
        ("invalid_client_metadata", "Client name is too long"),
    ];

    // Everything from one field onwards is wrong: that field decides.
    for (index, (error, description)) in expected.into_iter().enumerate() {
        let changes: Vec<(&str, Option<Value>)> = wrong[index..]
            .iter()
            .map(|(field, value)| (*field, Some(value.clone())))
            .collect();
        let refused = gateway
            .oauth
            .register_client(&with(&registration_metadata(), &changes))
            .await;
        assert_oauth_error(refused, 400, error, description);
    }
    assert_eq!(count(gateway.db(), "oauth_clients").await, 0);
}

#[tokio::test]
async fn leaves_the_shape_of_the_metadata_to_the_mcp_sdk_schema() {
    let gateway = TestGateway::new().await;

    for (field, value) in [
        ("redirect_uris", json!("https://client.example/callback")),
        ("redirect_uris", json!(["not a url"])),
        ("redirect_uris", json!([""])),
        // What the body parser makes of an empty string.
        ("redirect_uris", json!([null])),
        ("token_endpoint_auth_method", json!(["none"])),
        ("grant_types", json!("authorization_code")),
        ("response_types", json!([1])),
        ("scope", json!(["mcp:tools"])),
        ("client_name", json!(5)),
    ] {
        let refused = gateway
            .oauth
            .register_client(&with(&registration_metadata(), &[(field, Some(value))]))
            .await;
        assert_oauth_error(
            refused,
            400,
            "invalid_client_metadata",
            "Invalid OAuth client metadata",
        );
    }
    for not_metadata in [json!(null), json!([]), json!("metadata"), json!({})] {
        let refused = gateway.oauth.register_client(&not_metadata).await;
        assert_oauth_error(
            refused,
            400,
            "invalid_client_metadata",
            "Invalid OAuth client metadata",
        );
    }
}

#[tokio::test]
async fn accepts_between_one_and_ten_redirect_uris_of_at_most_2048_characters() {
    let gateway = TestGateway::new().await;
    let uri = |length: usize| {
        format!(
            "https://client.example/{}",
            "a".repeat(length - "https://client.example/".len())
        )
    };
    let uris = |count: usize| -> Vec<String> {
        (0..count)
            .map(|index| format!("https://client.example/{index}"))
            .collect()
    };

    for (redirect_uris, accepted) in [
        (vec![], false),
        (uris(10), true),
        (uris(11), false),
        (vec![uri(2048)], true),
        (vec![uri(2049)], false),
        (
            vec![
                "https://client.example/callback".to_owned(),
                "http://example.com/callback".to_owned(),
            ],
            false,
        ),
    ] {
        let answer = gateway
            .oauth
            .register_client(&with(
                &registration_metadata(),
                &[("redirect_uris", Some(json!(redirect_uris)))],
            ))
            .await;
        if accepted {
            assert_eq!(answer.unwrap()["redirect_uris"], json!(redirect_uris));
        } else {
            let refusal = oauth_error(answer);
            assert_eq!(
                (refusal.status, refusal.code),
                (400, "invalid_redirect_uri")
            );
        }
    }
}

#[tokio::test]
async fn fills_in_the_defaults_and_echoes_the_metadata_it_was_sent() {
    let gateway = TestGateway::new().await;

    let body = register(
        &gateway,
        &json!({
            "redirect_uris": [REMOTE_REDIRECT_URI],
            "client_uri": "https://client.example",
            "contacts": ["ops@client.example"],
            "software_id": "vine-client",
            "software_version": "1.2.3",
            "not_in_the_registry": "dropped",
        }),
    )
    .await;

    let client_secret = body["client_secret"].as_str().unwrap();
    assert!(
        client_secret
            .strip_prefix("mcp_secret_")
            .is_some_and(|random| {
                random.len() == 43
                    && random
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            })
    );
    let client = find_client(gateway.db(), body["client_id"].as_str().unwrap()).await;
    assert_eq!(
        client.client_secret_hash.as_deref(),
        Some(access_token::hash(client_secret).as_str())
    );
    assert_eq!(
        client.client_secret_prefix.as_deref(),
        Some(&client_secret[..12])
    );
    let issued_at = client.created_at.as_datetime().timestamp();
    assert_eq!(
        body,
        json!({
            "redirect_uris": [REMOTE_REDIRECT_URI],
            "client_uri": "https://client.example",
            "contacts": ["ops@client.example"],
            "software_id": "vine-client",
            "software_version": "1.2.3",
            "client_name": "MCP client",
            "token_endpoint_auth_method": "client_secret_basic",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "scope": "mcp:tools",
            "client_id": client.client_id,
            "client_id_issued_at": issued_at,
            "client_secret": client_secret,
            "client_secret_expires_at": client
                .client_secret_expires_at
                .unwrap()
                .as_datetime()
                .timestamp(),
        })
    );
    // The secret is good for a year.
    assert_eq!(
        body["client_secret_expires_at"].as_i64().unwrap() - issued_at,
        365 * 24 * 60 * 60
    );
    // In the order the Node app wrote them: the metadata, then what the gateway adds.
    assert_eq!(
        body.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "redirect_uris",
            "client_uri",
            "contacts",
            "software_id",
            "software_version",
            "client_name",
            "token_endpoint_auth_method",
            "grant_types",
            "response_types",
            "scope",
            "client_id",
            "client_id_issued_at",
            "client_secret",
            "client_secret_expires_at",
        ]
    );

    let padded = register(
        &gateway,
        &with(
            &registration_metadata(),
            &[
                (
                    "client_name",
                    Some(json!(format!("  {}  ", "x".repeat(120)))),
                ),
                ("scope", Some(json!("  mcp:tools  "))),
                (
                    "grant_types",
                    Some(json!([
                        "refresh_token",
                        "authorization_code",
                        "refresh_token"
                    ])),
                ),
            ],
        ),
    )
    .await;
    assert_eq!(padded["client_name"], json!("x".repeat(120)));
    assert_eq!(padded["scope"], "mcp:tools");
    assert_eq!(
        padded["grant_types"],
        json!(["refresh_token", "authorization_code"])
    );
    assert_eq!(
        padded.as_object().unwrap().keys().collect::<Vec<_>>(),
        [
            "redirect_uris",
            "token_endpoint_auth_method",
            "grant_types",
            "response_types",
            "client_name",
            "scope",
            "client_id",
            "client_id_issued_at",
        ]
    );
}

// vine: gateway OAuth authorization request

#[tokio::test]
async fn tells_a_missing_client_id_from_one_that_names_no_client() {
    let gateway = TestGateway::new().await;
    let oauth_client = register_with(&gateway, &[]).await;
    let base = authorization_for(&oauth_client.client_id, LOOPBACK_REDIRECT_URI);

    for client_id in [
        None,
        Some(json!("")),
        Some(json!([oauth_client.client_id, oauth_client.client_id])),
    ] {
        let refused = authorize(&gateway, &base, &[("client_id", client_id)]).await;
        assert_oauth_error(refused, 400, "invalid_request", "client_id is required");
    }

    // A blank value is a client id, and names no client.
    for client_id in [
        " ".to_owned(),
        format!(" {}", oauth_client.client_id),
        "mcp_client_unknown".to_owned(),
    ] {
        let refused = authorize(&gateway, &base, &[("client_id", Some(json!(client_id)))]).await;
        let error = oauth_error(refused);
        assert_eq!(
            (error.status, error.code, error.message.as_str()),
            (400, "invalid_client", "Unknown OAuth client")
        );
        assert_eq!(
            error.www_authenticate(),
            Some("Basic realm=\"MyMCPs OAuth\"")
        );
        assert_eq!(error.redirect_uri, None);
    }
}

#[tokio::test]
async fn never_redirects_to_a_uri_the_client_did_not_register() {
    let gateway = TestGateway::new().await;
    let oauth_client = register_with(&gateway, &[]).await;
    let base = authorization_for(&oauth_client.client_id, LOOPBACK_REDIRECT_URI);

    for redirect_uri in [
        None,
        Some(json!("")),
        Some(json!(" ")),
        Some(json!([LOOPBACK_REDIRECT_URI, LOOPBACK_REDIRECT_URI])),
        Some(json!("http://127.0.0.1:49152/other")),
        Some(json!("http://localhost:49152/callback")),
        Some(json!("https://127.0.0.1:49152/callback")),
        Some(json!("http://127.0.0.1:49152/callback?x=1")),
        Some(json!(REMOTE_REDIRECT_URI)),
    ] {
        let refused = authorize(
            &gateway,
            &base,
            &[
                ("redirect_uri", redirect_uri),
                ("response_type", Some(json!("token"))),
            ],
        )
        .await;
        let error = oauth_error(refused);
        assert_eq!(
            (error.status, error.code, error.message.as_str()),
            (400, "invalid_request", "Unregistered redirect_uri")
        );
        assert_eq!(error.redirect_uri, None);
        assert_eq!(error.www_authenticate(), None);
    }

    let remote = register_with(
        &gateway,
        &[("redirect_uris", Some(json!([REMOTE_REDIRECT_URI])))],
    )
    .await;
    let other_port = authorize(
        &gateway,
        &authorization_for(&remote.client_id, "https://client.example:8443/callback"),
        &[],
    )
    .await;
    assert_oauth_error(
        other_port,
        400,
        "invalid_request",
        "Unregistered redirect_uri",
    );
}

#[tokio::test]
async fn answers_with_the_first_parameter_an_authorization_request_gets_wrong() {
    let gateway = TestGateway::new().await;
    let oauth_client = register_with(&gateway, &[]).await;
    let base = authorization_for(&oauth_client.client_id, LOOPBACK_REDIRECT_URI);
    let long_state = "s".repeat(2049);
    let wrong = [
        ("response_type", json!("token")),
        ("code_challenge", json!("too-short")),
        ("scope", json!("other")),
        ("resource", json!("https://other.example/mcp")),
        ("state", json!(long_state)),
    ];
    let expected = [
        (
            "unsupported_response_type",
            "Only the code response type is supported",
        ),
        ("invalid_request", "PKCE with the S256 method is required"),
        ("invalid_scope", "Unsupported OAuth scope"),
        (
            "invalid_target",
            "The OAuth resource must be the MyMCPs gateway",
        ),
        ("invalid_request", "OAuth state is too long"),
    ];

    // Everything from one parameter onwards is wrong: that parameter decides.
    for (index, (error, description)) in expected.into_iter().enumerate() {
        let changes: Vec<(&str, Option<Value>)> = wrong[index..]
            .iter()
            .map(|(field, value)| (*field, Some(value.clone())))
            .collect();
        assert_eq!(
            redirected_error(authorize(&gateway, &base, &changes).await),
            (
                error.to_owned(),
                description.to_owned(),
                Some(long_state.clone())
            )
        );
    }
}

#[tokio::test]
async fn requires_pkce_with_a_well_formed_s256_challenge() {
    let gateway = TestGateway::new().await;
    let oauth_client = register_with(&gateway, &[]).await;
    let base = authorization_for(&oauth_client.client_id, LOOPBACK_REDIRECT_URI);
    let challenge = code_challenge(CODE_VERIFIER);

    for change in [
        ("code_challenge", None),
        ("code_challenge", Some(json!(""))),
        ("code_challenge", Some(json!("a".repeat(42)))),
        ("code_challenge", Some(json!("a".repeat(129)))),
        (
            "code_challenge",
            Some(json!(format!("{}.", "a".repeat(42)))),
        ),
        ("code_challenge", Some(json!([challenge, challenge]))),
        ("code_challenge_method", None),
        ("code_challenge_method", Some(json!("plain"))),
        ("code_challenge_method", Some(json!("s256"))),
        ("code_challenge_method", Some(json!(["S256", "S256"]))),
    ] {
        assert_eq!(
            redirected_error(authorize(&gateway, &base, &[change]).await),
            (
                "invalid_request".to_owned(),
                "PKCE with the S256 method is required".to_owned(),
                Some("state-from-client".to_owned())
            )
        );
    }

    for challenge in ["a".repeat(43), "a".repeat(128)] {
        let request = authorize(
            &gateway,
            &base,
            &[("code_challenge", Some(json!(challenge)))],
        )
        .await
        .unwrap();
        assert_eq!(request.code_challenge, challenge);
    }
}

#[tokio::test]
async fn accepts_the_gateway_scope_and_resource_in_the_spellings_clients_use() {
    let gateway = TestGateway::new().await;
    let oauth_client = register_with(&gateway, &[]).await;
    let base = authorization_for(&oauth_client.client_id, LOOPBACK_REDIRECT_URI);

    for change in [
        ("scope", None),
        ("scope", Some(json!(" mcp:tools "))),
        // Sent twice, the parameter is not a scope string and counts as left out.
        ("scope", Some(json!(["other", "another"]))),
        ("resource", Some(json!("http://LOCALHOST:3333/mcp"))),
        ("resource", Some(json!("HTTP://localhost:3333/a/../mcp"))),
    ] {
        let request = authorize(&gateway, &base, &[change]).await.unwrap();
        // What is granted is spelled the gateway's way, whatever was asked.
        assert_eq!(request.scopes, "mcp:tools");
        assert_eq!(request.resource, RESOURCE);
    }

    for scope in [
        "",
        " ",
        "mcp:tools mcp:tools",
        "mcp:tools other",
        "MCP:TOOLS",
    ] {
        let (error, description, _) =
            redirected_error(authorize(&gateway, &base, &[("scope", Some(json!(scope)))]).await);
        assert_eq!(
            (error.as_str(), description.as_str()),
            ("invalid_scope", "Unsupported OAuth scope")
        );
    }

    for target in [
        None,
        Some(json!("")),
        Some(json!("http://localhost:3333/mcp/")),
        Some(json!("http://localhost:3333/mcp?x=1")),
        Some(json!("http://localhost:3333")),
        Some(json!("not a url")),
        Some(json!([RESOURCE, RESOURCE])),
    ] {
        let (error, description, _) =
            redirected_error(authorize(&gateway, &base, &[("resource", target)]).await);
        assert_eq!(
            (error.as_str(), description.as_str()),
            (
                "invalid_target",
                "The OAuth resource must be the MyMCPs gateway"
            )
        );
    }
}

#[tokio::test]
async fn echoes_the_state_exactly_as_the_client_sent_it() {
    let gateway = TestGateway::new().await;
    let oauth_client = register_with(&gateway, &[]).await;
    let base = with(
        &authorization_for(&oauth_client.client_id, LOOPBACK_REDIRECT_URI),
        &[("response_type", Some(json!("token")))],
    );
    let long = "s".repeat(2049);

    for (state, echoed) in [
        (None, None),
        (Some(json!("")), Some("")),
        (Some(json!(" ")), Some(" ")),
        (Some(json!("a b&c=d#e%20")), Some("a b&c=d#e%20")),
        (Some(json!(long)), Some(long.as_str())),
        // Sent twice, the parameter is not a state.
        (Some(json!(["one", "two"])), None),
    ] {
        assert_eq!(
            redirected_error(authorize(&gateway, &base, &[("state", state)]).await),
            (
                "unsupported_response_type".to_owned(),
                "Only the code response type is supported".to_owned(),
                echoed.map(str::to_owned)
            )
        );
    }
}

#[tokio::test]
async fn grants_access_on_an_explicit_approval_only() {
    let gateway = TestGateway::new().await;
    let admin = create_admin(gateway.db()).await;
    let oauth_client = register_with(&gateway, &[]).await;
    let base = authorization_for(&oauth_client.client_id, LOOPBACK_REDIRECT_URI);

    for decision in [
        None,
        // What the body parser makes of an empty field.
        Some(json!(null)),
        Some(json!("")),
        Some(json!("deny")),
        Some(json!("APPROVE")),
        Some(json!("true")),
        Some(json!(["approve", "approve"])),
        Some(json!({ "value": "approve" })),
    ] {
        let form = with(&base, &[("decision", decision)]);
        gateway
            .oauth
            .parse_authorization_request(&form)
            .await
            .unwrap();
        assert!(CONSENT_APPROVAL.validate(form.get("decision")).is_err());
    }
    assert_eq!(count(gateway.db(), "oauth_authorization_codes").await, 0);

    let form = with(&base, &[("decision", Some(json!("approve")))]);
    let request = gateway
        .oauth
        .parse_authorization_request(&form)
        .await
        .unwrap();
    assert!(CONSENT_APPROVAL.validate(form.get("decision")).is_ok());
    let code = gateway
        .oauth
        .create_authorization_code(&request, admin.id)
        .await
        .unwrap();
    assert_eq!(
        oauth_redirect(
            &request.redirect_uri,
            &[
                ("code", Some(code.as_str())),
                ("state", request.state.as_deref())
            ]
        )
        .unwrap(),
        format!("{LOOPBACK_REDIRECT_URI}?code={code}&state=state-from-client")
    );
    assert_eq!(code.len(), 43);
}

// vine: gateway OAuth token and revocation endpoints

async fn issue_code(gateway: &TestGateway, oauth_client: &OauthClient, admin: &User) -> String {
    gateway
        .oauth
        .create_authorization_code(
            &GatewayAuthorizationRequest {
                client: oauth_client.clone(),
                redirect_uri: LOOPBACK_REDIRECT_URI.to_owned(),
                state: None,
                code_challenge: code_challenge(CODE_VERIFIER),
                scopes: "mcp:tools".to_owned(),
                resource: RESOURCE.to_owned(),
            },
            admin.id,
        )
        .await
        .unwrap()
}

/// The access token and the refresh token of a new grant.
async fn issue_grant(
    gateway: &TestGateway,
    oauth_client: &OauthClient,
    admin: &User,
) -> (String, String) {
    let created = access_token::create_oauth_grant(
        &**gateway.db(),
        NewOauthGrant {
            name: &oauth_client.client_name,
            client_id: oauth_client.id,
            client_supports_refresh: true,
            scopes: "mcp:tools",
            resource: RESOURCE,
            created_by: admin.id,
        },
    )
    .await
    .unwrap();
    (created.plaintext, created.refresh_token.unwrap())
}

#[tokio::test]
async fn authenticates_the_client_before_it_reads_the_grant() {
    let gateway = TestGateway::new().await;
    let oauth_client = register_with(&gateway, &[]).await;
    let client_id = oauth_client.client_id.as_str();

    for credentials in [
        json!({}),
        json!({ "client_id": null }),
        json!({ "client_id": [client_id, client_id] }),
    ] {
        let refused = gateway.oauth.issue_tokens(None, &credentials).await;
        let error = oauth_error(refused);
        assert_eq!(
            (error.status, error.body()),
            (
                401,
                json!({
                    "error": "invalid_client",
                    "error_description": "OAuth client authentication is required",
                })
            )
        );
        assert_eq!(
            error.www_authenticate(),
            Some("Basic realm=\"MyMCPs OAuth\"")
        );
    }

    let unknown = gateway
        .oauth
        .issue_tokens(None, &json!({ "client_id": "mcp_client_unknown" }))
        .await;
    assert_oauth_error(
        unknown,
        401,
        "invalid_client",
        "Invalid OAuth client credentials",
    );

    // A public client that sends a secret is not the client that registered.
    let with_secret = gateway
        .oauth
        .issue_tokens(
            None,
            &json!({ "client_id": client_id, "client_secret": "surplus" }),
        )
        .await;
    assert_oauth_error(
        with_secret,
        401,
        "invalid_client",
        "Invalid OAuth client credentials",
    );

    // A secret sent twice is not a secret, and the client is a public one.
    let secret_twice = gateway
        .oauth
        .issue_tokens(
            None,
            &json!({ "client_id": client_id, "client_secret": ["surplus", "surplus"] }),
        )
        .await;
    assert_oauth_error(
        secret_twice,
        400,
        "invalid_request",
        "grant_type is required",
    );
}

#[tokio::test]
async fn accepts_a_client_secret_in_the_body_or_in_a_basic_header() {
    let gateway = TestGateway::new().await;
    let posted = register(
        &gateway,
        &with(
            &registration_metadata(),
            &[(
                "token_endpoint_auth_method",
                Some(json!("client_secret_post")),
            )],
        ),
    )
    .await;
    let registered_basic = register(
        &gateway,
        &with(
            &registration_metadata(),
            &[(
                "token_endpoint_auth_method",
                Some(json!("client_secret_basic")),
            )],
        ),
    )
    .await;
    let text = |client: &Value, name: &str| client[name].as_str().unwrap().to_owned();
    let (posted_id, posted_secret) = (text(&posted, "client_id"), text(&posted, "client_secret"));
    let (basic_id, basic_secret) = (
        text(&registered_basic, "client_id"),
        text(&registered_basic, "client_secret"),
    );

    let via_body = gateway
        .oauth
        .issue_tokens(
            None,
            &json!({ "client_id": posted_id, "client_secret": posted_secret }),
        )
        .await;
    assert_oauth_error(via_body, 400, "invalid_request", "grant_type is required");

    let via_header = gateway
        .oauth
        .issue_tokens(Some(&basic(&basic_id, &basic_secret)), &json!({}))
        .await;
    assert_oauth_error(via_header, 400, "invalid_request", "grant_type is required");

    for (header, input) in [
        (None, json!({ "client_id": posted_id })),
        (
            None,
            json!({ "client_id": posted_id, "client_secret": "mcp_secret_wrong" }),
        ),
        (Some(basic(&basic_id, "mcp_secret_wrong")), json!({})),
        // The method a client registered is the only one it may use.
        (Some(basic(&posted_id, &posted_secret)), json!({})),
        (
            None,
            json!({ "client_id": basic_id, "client_secret": basic_secret }),
        ),
    ] {
        let refused = gateway.oauth.issue_tokens(header.as_deref(), &input).await;
        assert_oauth_error(
            refused,
            401,
            "invalid_client",
            "Invalid OAuth client credentials",
        );
    }
}

#[tokio::test]
async fn refuses_a_client_secret_that_has_expired() {
    let gateway = TestGateway::new().await;
    let registered = register(
        &gateway,
        &with(
            &registration_metadata(),
            &[(
                "token_endpoint_auth_method",
                Some(json!("client_secret_post")),
            )],
        ),
    )
    .await;
    let credentials = json!({
        "client_id": registered["client_id"],
        "client_secret": registered["client_secret"],
    });
    sqlx::query("update `oauth_clients` set `client_secret_expires_at` = ?")
        .bind(mymcps_core::Timestamp::now())
        .execute(&**gateway.db())
        .await
        .unwrap();

    let refused = gateway.oauth.issue_tokens(None, &credentials).await;
    assert_oauth_error(
        refused,
        401,
        "invalid_client",
        "Invalid OAuth client credentials",
    );
}

#[tokio::test]
async fn names_the_grant_type_it_misses_and_refuses_those_it_does_not_have() {
    let gateway = TestGateway::new().await;
    let oauth_client = register_with(&gateway, &[]).await;
    let credentials = json!({ "client_id": oauth_client.client_id });

    for grant_type in [
        None,
        Some(json!(null)),
        Some(json!(["refresh_token", "refresh_token"])),
    ] {
        let refused = gateway
            .oauth
            .issue_tokens(None, &with(&credentials, &[("grant_type", grant_type)]))
            .await;
        assert_oauth_error(refused, 400, "invalid_request", "grant_type is required");
    }

    for grant_type in ["client_credentials", "password", "AUTHORIZATION_CODE"] {
        let refused = gateway
            .oauth
            .issue_tokens(
                None,
                &with(&credentials, &[("grant_type", Some(json!(grant_type)))]),
            )
            .await;
        assert_oauth_error(
            refused,
            400,
            "unsupported_grant_type",
            "Only authorization_code and refresh_token grants are supported",
        );
    }
}

#[tokio::test]
async fn names_the_first_parameter_an_authorization_code_grant_misses() {
    let gateway = TestGateway::new().await;
    let admin = create_admin(gateway.db()).await;
    let oauth_client = register_with(&gateway, &[]).await;
    let grant = json!({
        "grant_type": "authorization_code",
        "client_id": oauth_client.client_id,
        "code": issue_code(&gateway, &oauth_client, &admin).await,
        "code_verifier": CODE_VERIFIER,
        "redirect_uri": LOOPBACK_REDIRECT_URI,
        "resource": RESOURCE,
    });
    let parameters = ["code", "code_verifier", "redirect_uri", "resource"];

    for (index, parameter) in parameters.into_iter().enumerate() {
        // Everything from this parameter onwards is missing, empty or sent twice.
        for value in [None, Some(json!(null)), Some(json!(["one", "two"]))] {
            let changes: Vec<(&str, Option<Value>)> = parameters[index..]
                .iter()
                .map(|name| (*name, value.clone()))
                .collect();
            let refused = gateway
                .oauth
                .issue_tokens(None, &with(&grant, &changes))
                .await;
            assert_oauth_error(
                refused,
                400,
                "invalid_request",
                &format!("{parameter} is required"),
            );
        }
    }

    // None of these requests used the code up.
    let exchange = gateway.oauth.issue_tokens(None, &grant).await.unwrap();
    assert!(exchange.plaintext.starts_with("mcp_"));
    assert_eq!(exchange.plaintext.len(), 47);
}

#[tokio::test]
async fn answers_a_malformed_verifier_or_a_foreign_resource_like_a_code_that_does_not_match() {
    let gateway = TestGateway::new().await;
    let admin = create_admin(gateway.db()).await;
    let oauth_client = register_with(&gateway, &[]).await;
    let grant = json!({
        "grant_type": "authorization_code",
        "client_id": oauth_client.client_id,
        "code": issue_code(&gateway, &oauth_client, &admin).await,
        "code_verifier": CODE_VERIFIER,
        "redirect_uri": LOOPBACK_REDIRECT_URI,
        "resource": RESOURCE,
    });

    for (name, value) in [
        ("code_verifier", "a".repeat(42)),
        ("code_verifier", "a".repeat(129)),
        ("code_verifier", format!("{CODE_VERIFIER}!")),
        (
            "code_verifier",
            "another-verifier-that-is-long-enough-to-be-well-formed".to_owned(),
        ),
        ("resource", "https://other.example/mcp".to_owned()),
        ("resource", "http://localhost:3333/mcp/".to_owned()),
        ("resource", "not a url".to_owned()),
        ("redirect_uri", "http://127.0.0.1/callback".to_owned()),
        ("code", "unknown-code".to_owned()),
    ] {
        let refused = gateway
            .oauth
            .issue_tokens(None, &with(&grant, &[(name, Some(json!(value)))]))
            .await;
        assert_oauth_error(
            refused,
            400,
            "invalid_grant",
            "Invalid or expired authorization code",
        );
    }

    // Another client cannot exchange the code either.
    let other_client = register_with(&gateway, &[]).await;
    let stolen = gateway
        .oauth
        .issue_tokens(
            None,
            &with(
                &grant,
                &[("client_id", Some(json!(other_client.client_id)))],
            ),
        )
        .await;
    assert_oauth_error(
        stolen,
        400,
        "invalid_grant",
        "Invalid or expired authorization code",
    );

    gateway
        .oauth
        .issue_tokens(
            None,
            &with(
                &grant,
                &[("resource", Some(json!("http://LOCALHOST:3333/mcp")))],
            ),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn refuses_an_authorization_code_that_has_expired() {
    let gateway = TestGateway::new().await;
    let admin = create_admin(gateway.db()).await;
    let oauth_client = register_with(&gateway, &[]).await;
    let code = issue_code(&gateway, &oauth_client, &admin).await;
    sqlx::query("update `oauth_authorization_codes` set `expires_at` = ?")
        .bind(mymcps_core::Timestamp::now())
        .execute(&**gateway.db())
        .await
        .unwrap();

    let refused = gateway
        .oauth
        .issue_tokens(
            None,
            &json!({
                "grant_type": "authorization_code",
                "client_id": oauth_client.client_id,
                "code": code,
                "code_verifier": CODE_VERIFIER,
                "redirect_uri": LOOPBACK_REDIRECT_URI,
                "resource": RESOURCE,
            }),
        )
        .await;
    assert_oauth_error(
        refused,
        400,
        "invalid_grant",
        "Invalid or expired authorization code",
    );
}

#[tokio::test]
async fn checks_a_refresh_request_in_the_order_client_scope_resource_token() {
    let gateway = TestGateway::new().await;
    let admin = create_admin(gateway.db()).await;
    let oauth_client = register_with(&gateway, &[]).await;
    let (_, refresh_token) = issue_grant(&gateway, &oauth_client, &admin).await;
    let grant = json!({
        "grant_type": "refresh_token",
        "client_id": oauth_client.client_id,
        "refresh_token": refresh_token,
        "resource": RESOURCE,
    });

    for (changes, description) in [
        (
            vec![("refresh_token", None), ("resource", None)],
            "refresh_token is required",
        ),
        (
            vec![("refresh_token", Some(json!(["one", "two"])))],
            "refresh_token is required",
        ),
        (
            vec![("resource", None), ("scope", Some(json!("other")))],
            "resource is required",
        ),
        (
            vec![("resource", Some(json!(null)))],
            "resource is required",
        ),
    ] {
        let refused = gateway
            .oauth
            .issue_tokens(None, &with(&grant, &changes))
            .await;
        assert_oauth_error(refused, 400, "invalid_request", description);
    }

    for scope in ["other", "mcp:tools mcp:tools", "MCP:TOOLS"] {
        let refused = gateway
            .oauth
            .issue_tokens(
                None,
                &with(
                    &grant,
                    &[
                        ("scope", Some(json!(scope))),
                        ("resource", Some(json!("https://other.example/mcp"))),
                    ],
                ),
            )
            .await;
        assert_oauth_error(refused, 400, "invalid_scope", "Unsupported OAuth scope");
    }
    // An empty scope in the query string is a scope, and names none.
    let empty_scope = gateway
        .oauth
        .issue_tokens(None, &with(&grant, &[("scope", Some(json!("")))]))
        .await;
    assert_oauth_error(empty_scope, 400, "invalid_scope", "Unsupported OAuth scope");

    for target in [
        "https://other.example/mcp",
        "http://localhost:3333/mcp/",
        "not a url",
    ] {
        let refused = gateway
            .oauth
            .issue_tokens(
                None,
                &with(
                    &grant,
                    &[
                        ("resource", Some(json!(target))),
                        ("refresh_token", Some(json!("mcp_refresh_unknown"))),
                    ],
                ),
            )
            .await;
        assert_oauth_error(
            refused,
            400,
            "invalid_target",
            "The OAuth resource must be the MyMCPs gateway",
        );
    }

    let unknown = gateway
        .oauth
        .issue_tokens(
            None,
            &with(
                &grant,
                &[("refresh_token", Some(json!("mcp_refresh_unknown")))],
            ),
        )
        .await;
    assert_oauth_error(
        unknown,
        400,
        "invalid_grant",
        "Invalid, expired, or revoked refresh token",
    );

    // Sent twice, the scope is not a scope string and counts as left out.
    let refreshed = gateway
        .oauth
        .issue_tokens(
            None,
            &with(
                &grant,
                &[
                    ("scope", Some(json!(["other", "another"]))),
                    ("resource", Some(json!("http://LOCALHOST:3333/mcp"))),
                ],
            ),
        )
        .await
        .unwrap();
    let next_refresh_token = refreshed.refresh_token.unwrap();
    assert!(next_refresh_token.starts_with("mcp_refresh_"));
    assert_eq!(next_refresh_token.len(), 55);

    gateway
        .oauth
        .issue_tokens(
            None,
            &with(
                &grant,
                &[
                    ("refresh_token", Some(json!(next_refresh_token))),
                    ("scope", Some(json!("mcp:tools"))),
                ],
            ),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn refuses_a_refresh_token_that_has_expired_or_belongs_to_another_client() {
    let gateway = TestGateway::new().await;
    let admin = create_admin(gateway.db()).await;
    let oauth_client = register_with(&gateway, &[]).await;
    let other_client = register_with(&gateway, &[]).await;
    let (access_token, refresh_token) = issue_grant(&gateway, &oauth_client, &admin).await;
    let grant = json!({
        "grant_type": "refresh_token",
        "client_id": oauth_client.client_id,
        "refresh_token": refresh_token,
        "resource": RESOURCE,
    });

    let foreign = gateway
        .oauth
        .issue_tokens(
            None,
            &with(
                &grant,
                &[("client_id", Some(json!(other_client.client_id)))],
            ),
        )
        .await;
    assert_oauth_error(
        foreign,
        400,
        "invalid_grant",
        "Invalid, expired, or revoked refresh token",
    );

    sqlx::query("update `access_tokens` set `oauth_refresh_expires_at` = ?")
        .bind(mymcps_core::Timestamp::now())
        .execute(&**gateway.db())
        .await
        .unwrap();
    let expired = gateway.oauth.issue_tokens(None, &grant).await;
    assert_oauth_error(
        expired,
        400,
        "invalid_grant",
        "Invalid, expired, or revoked refresh token",
    );

    // Neither request touched the grant.
    assert!(
        access_token::find_usable_by_plaintext(gateway.db(), &access_token)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn refuses_a_refresh_to_a_client_registered_without_that_grant_whatever_else_is_wrong() {
    let gateway = TestGateway::new().await;
    let admin = create_admin(gateway.db()).await;
    let oauth_client = register_with(
        &gateway,
        &[("grant_types", Some(json!(["authorization_code"])))],
    )
    .await;
    let (_, refresh_token) = issue_grant(&gateway, &oauth_client, &admin).await;

    let refused = gateway
        .oauth
        .issue_tokens(
            None,
            &json!({
                "grant_type": "refresh_token",
                "client_id": oauth_client.client_id,
                "refresh_token": refresh_token,
                "scope": "other",
                "resource": "https://other.example/mcp",
            }),
        )
        .await;

    assert_oauth_error(
        refused,
        400,
        "unauthorized_client",
        "This OAuth client cannot refresh tokens",
    );
}

#[tokio::test]
async fn gives_no_refresh_token_to_a_client_registered_without_that_grant() {
    let gateway = TestGateway::new().await;
    let admin = create_admin(gateway.db()).await;
    let oauth_client = register_with(
        &gateway,
        &[("grant_types", Some(json!(["authorization_code"])))],
    )
    .await;

    let created = gateway
        .oauth
        .issue_tokens(
            None,
            &json!({
                "grant_type": "authorization_code",
                "client_id": oauth_client.client_id,
                "code": issue_code(&gateway, &oauth_client, &admin).await,
                "code_verifier": CODE_VERIFIER,
                "redirect_uri": LOOPBACK_REDIRECT_URI,
                "resource": RESOURCE,
            }),
        )
        .await
        .unwrap();

    assert_eq!(created.refresh_token, None);
    assert_eq!(created.token.oauth_refresh_token_hash, None);
    assert_eq!(created.token.oauth_refresh_expires_at, None);
    assert_eq!(
        mymcps_gateway::oauth::oauth_token_response(&created)
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["access_token", "token_type", "expires_in", "scope"]
    );
}

#[tokio::test]
async fn requires_a_token_to_revoke_and_says_nothing_about_tokens_it_does_not_know() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let oauth_client = register_with(&gateway, &[]).await;
    let (access_token, _) = issue_grant(&gateway, &oauth_client, &admin).await;
    let credentials = json!({ "client_id": oauth_client.client_id });
    let usable = |plaintext: String| async move {
        access_token::find_usable_by_plaintext(db, &plaintext)
            .await
            .unwrap()
            .is_some()
    };

    for token in [None, Some(json!(null)), Some(json!(["one", "two"]))] {
        let refused = gateway
            .oauth
            .revoke_token(None, &with(&credentials, &[("token", token)]))
            .await;
        assert_oauth_error(refused, 400, "invalid_request", "token is required");
    }
    let anonymous = gateway
        .oauth
        .revoke_token(None, &json!({ "token": access_token }))
        .await;
    assert_oauth_error(
        anonymous,
        401,
        "invalid_client",
        "OAuth client authentication is required",
    );
    assert!(usable(access_token.clone()).await);

    // A blank token in the query string is a token, and one nobody holds.
    for token in ["mcp_unknown", " "] {
        gateway
            .oauth
            .revoke_token(None, &with(&credentials, &[("token", Some(json!(token)))]))
            .await
            .unwrap();
    }
    assert!(usable(access_token.clone()).await);

    // Another client cannot revoke the grant.
    let other_client = register_with(&gateway, &[]).await;
    gateway
        .oauth
        .revoke_token(
            None,
            &json!({ "client_id": other_client.client_id, "token": access_token }),
        )
        .await
        .unwrap();
    assert!(usable(access_token.clone()).await);

    gateway
        .oauth
        .revoke_token(
            None,
            &with(&credentials, &[("token", Some(json!(access_token)))]),
        )
        .await
        .unwrap();
    assert!(!usable(access_token.clone()).await);
}

#[tokio::test]
async fn revokes_a_grant_by_its_refresh_token_or_one_it_gave_up() {
    let gateway = TestGateway::new().await;
    let db = gateway.db();
    let admin = create_admin(db).await;
    let oauth_client = register_with(&gateway, &[]).await;
    let credentials = json!({ "client_id": oauth_client.client_id });

    let (access_token, refresh_token) = issue_grant(&gateway, &oauth_client, &admin).await;
    gateway
        .oauth
        .revoke_token(
            None,
            &with(&credentials, &[("token", Some(json!(refresh_token)))]),
        )
        .await
        .unwrap();
    assert!(
        access_token::find_usable_by_plaintext(db, &access_token)
            .await
            .unwrap()
            .is_none()
    );

    let (_, first_refresh_token) = issue_grant(&gateway, &oauth_client, &admin).await;
    let refreshed = gateway
        .oauth
        .issue_tokens(
            None,
            &json!({
                "grant_type": "refresh_token",
                "client_id": oauth_client.client_id,
                "refresh_token": first_refresh_token,
                "resource": RESOURCE,
            }),
        )
        .await
        .unwrap();
    gateway
        .oauth
        .revoke_token(
            None,
            &with(&credentials, &[("token", Some(json!(first_refresh_token)))]),
        )
        .await
        .unwrap();
    assert!(
        access_token::find_usable_by_plaintext(db, &refreshed.plaintext)
            .await
            .unwrap()
            .is_none()
    );
}
