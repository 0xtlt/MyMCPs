//! `tests/unit/hardening_upstream_oauth_session.spec.ts`: the pending OAuth
//! starts of a browser session stay within what a session cookie can hold.

mod support;

use mymcps_core::models::Mcp;
use mymcps_upstream::{
    MAX_PENDING_OAUTH_START_BYTES, OauthSession, OauthStart, read_oauth_session,
    start_oauth_session,
};
use serde_json::json;
use support::*;

fn start(session: &MemorySession, state: &str, client_id: &str) -> OauthSession {
    let mcp = Mcp {
        id: 1,
        ..Default::default()
    };
    start_oauth_session(
        session,
        &mcp,
        OauthStart {
            redirect_uri: CALLBACK.to_owned(),
            authorization_server_url: "https://auth.example.com".to_owned(),
            resource: Some("https://mcp.example.com/mcp".to_owned()),
            client_id: client_id.to_owned(),
            code_verifier: Some("v".repeat(43)),
            state: state.to_owned(),
        },
    )
}

#[test]
fn older_starts_make_room_so_the_pending_ones_fit_in_a_cookie_store_session() {
    let session = MemorySession::new();
    session.set("auth_web", json!(42));

    // Long client identifiers: five of these would not fit in the session cookie.
    for index in 0..5 {
        start(&session, &format!("state-{index}"), &"c".repeat(400));
    }

    let pending = session.pending();
    assert!(pending.len() < 5);
    assert!(session.pending_bytes() <= MAX_PENDING_OAUTH_START_BYTES);
    assert_eq!(
        pending.last().map(String::as_str),
        Some("mcp_oauth:state-4")
    );
    assert_eq!(session.value("auth_web"), Some(json!(42)));
    // Each weighs 672 bytes as the Node app counted them: two fit, three do not.
    assert_eq!(pending, ["mcp_oauth:state-3", "mcp_oauth:state-4"]);
    assert_eq!(session.pending_bytes(), 2 * 672);
}

#[test]
fn the_newest_start_is_kept_even_when_it_alone_is_over_the_size() {
    let session = MemorySession::new();
    start(&session, "older", "client");
    start(
        &session,
        "newest",
        &"c".repeat(MAX_PENDING_OAUTH_START_BYTES),
    );

    assert_eq!(session.pending(), ["mcp_oauth:newest"]);
}

#[test]
fn short_starts_are_still_limited_by_their_number() {
    let session = MemorySession::new();
    for index in 0..8 {
        start(&session, &format!("s{index}"), "c");
    }

    let pending = session.pending();
    assert!(pending.len() <= 5);
    assert_eq!(pending.last().map(String::as_str), Some("mcp_oauth:s7"));
    assert_eq!(
        pending,
        [
            "mcp_oauth:s3",
            "mcp_oauth:s4",
            "mcp_oauth:s5",
            "mcp_oauth:s6",
            "mcp_oauth:s7"
        ]
    );
    assert_eq!(session.pending_bytes(), 1315);
}

#[test]
fn a_start_is_read_back_as_it_was_put_and_only_under_its_own_state() {
    let session = MemorySession::new();
    let started = start(&session, "state-a", "client");

    assert_eq!(
        serde_json::to_string(&session.value("mcp_oauth:state-a").unwrap()).unwrap(),
        format!(
            r#"{{"mcpId":1,"redirectUri":"{CALLBACK}","authorizationServerUrl":"https://auth.example.com","resource":"https://mcp.example.com/mcp","clientId":"client","codeVerifier":"{}","state":"state-a"}}"#,
            "v".repeat(43)
        )
    );
    assert_eq!(read_oauth_session(&session, Some("state-a")), Some(started));
    assert_eq!(read_oauth_session(&session, Some("state-b")), None);

    // What another part of the app, or an older version, left under a key
    // of this kind is not an authorization.
    session.set(
        "mcp_oauth:tampered",
        json!({ "mcpId": 1, "state": "tampered" }),
    );
    assert_eq!(read_oauth_session(&session, Some("tampered")), None);
    session.set("mcp_oauth:text", json!("state"));
    assert_eq!(read_oauth_session(&session, Some("text")), None);
}

#[test]
fn starting_again_under_a_state_already_pending_keeps_one_entry() {
    let session = MemorySession::new();
    start(&session, "same", "first");
    let again = start(&session, "same", "second");

    assert_eq!(session.pending(), ["mcp_oauth:same"]);
    assert_eq!(read_oauth_session(&session, Some("same")), Some(again));
}
