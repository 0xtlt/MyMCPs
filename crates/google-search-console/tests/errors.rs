//! What an agent is told when Google refuses a request.

mod support;

use mymcps_google_search_console::testing::{FakeSearchConsole, SITE, google_failure, google_json};
use mymcps_net::CannedResponse;
use serde_json::json;
use support::Google;

async fn refusal_for(
    answer: impl Fn() -> CannedResponse + Send + Sync + 'static,
) -> mymcps_builtin::BuiltinError {
    let google = Google::new(FakeSearchConsole::responding(move |_| Some(answer()))).await;
    google
        .call("list_sitemaps", json!({ "site_url": SITE }), &[])
        .await
        .unwrap_err()
}

#[tokio::test]
async fn explains_what_the_setup_must_fix() {
    let cases: [(u16, &str, &str, &str, &str); 5] = [
        (
            403,
            "PERMISSION_DENIED",
            "SERVICE_DISABLED",
            "Google Search Console API has not been used in project 123 before or it is disabled.",
            "The Google Search Console API is not enabled in the Google Cloud project of the OAuth client.",
        ),
        (
            403,
            "PERMISSION_DENIED",
            "forbidden",
            "User does not have sufficient permission for site 'sc-domain:example.com'.",
            "The connected Google account is not a user of this property",
        ),
        (
            403,
            "PERMISSION_DENIED",
            "insufficientPermissions",
            "Request had insufficient authentication scopes.",
            "turn on write access from the MCPs page in MyMCPs, then re-authorize the MCP.",
        ),
        (
            429,
            "RESOURCE_EXHAUSTED",
            "rateLimitExceeded",
            "Quota exceeded.",
            "Search Console quota reached for now.",
        ),
        (
            404,
            "NOT_FOUND",
            "notFound",
            "Requested entity was not found.",
            "",
        ),
    ];
    for (status, code, reason, message, hint) in cases {
        let error = refusal_for(move || google_failure(status, code, reason, message)).await;
        assert!(error.is_tool_error(), "{reason}: {error}");
        let said = error.to_string();
        assert!(
            said.starts_with(&format!(
                "Google Search Console refused the request: {message}"
            )),
            "{said}"
        );
        assert!(said.contains(hint), "{reason}: {said}");
    }
}

#[tokio::test]
async fn a_rejected_sign_in_asks_to_authorize_again() {
    let error = refusal_for(|| {
        google_json(
            &json!({ "error": { "code": 401, "message": "Invalid Credentials", "status": "UNAUTHENTICATED" } }),
            401,
        )
    })
    .await;
    assert!(error.is_authorization_error());
    assert_eq!(
        error.to_string(),
        "Google rejected the saved authorization. Re-authorize this MCP in MyMCPs."
    );
}

#[tokio::test]
async fn a_refusal_without_json_names_its_status() {
    let error =
        refusal_for(|| CannedResponse::new(http::StatusCode::BAD_GATEWAY).body("<html>")).await;
    assert_eq!(
        error.to_string(),
        "Google Search Console refused the request: HTTP 502"
    );
}
