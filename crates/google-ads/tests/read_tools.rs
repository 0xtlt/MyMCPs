//! The read tools: the port of the "monitoring" group of
//! `tests/functional/builtin_google_ads_mcp.spec.ts`, then every read tool
//! against what the TypeScript does with the same answers of Google.
//!
//! `fixtures/read_tools.json` holds some eighty calls, run by Node through
//! the tools of `app/services/builtin/google_ads/read_tools.ts` in front of
//! a Google that answers from a script: the requests each call made, to the
//! character, and what it returned or why it failed. The script that wrote
//! it is not part of the repository, since it runs Node on the TypeScript
//! app.

mod support;

use chrono::{Duration, Utc};
use mymcps_builtin::{BuiltinEnv, BuiltinError};
use mymcps_core::TestCore;
use mymcps_google_ads::testing::{
    FakeGoogleAds, GOOGLE_ADS_CUSTOMER, GoogleAdsFault, fixtures, google_ads_context,
    google_ads_failure, google_json,
};
use serde_json::{Value, json};
use support::{Google, ScriptedGoogle, provider};

const FIXTURE: &str = include_str!("fixtures/read_tools.json");

/// What an agent reads of a call that failed, once it is known to be told why.
fn refused(outcome: Result<Value, BuiltinError>) -> String {
    let error = outcome.unwrap_err();
    assert!(error.is_tool_error(), "{error}");
    error.to_string()
}

#[tokio::test]
async fn lists_campaigns_with_amounts_in_the_currency_of_the_account() {
    let google = Google::new(FakeGoogleAds::new()).await;

    let data = google
        .call(
            "list_campaigns",
            json!({ "customer_id": "123-456-7890", "date_range": "LAST_7_DAYS" }),
            &[("loginCustomerId", "9876543210")],
        )
        .await
        .unwrap();

    assert_eq!(
        data,
        json!({
            "currency": "EUR",
            "period": "LAST_7_DAYS",
            "campaigns": [{
                "id": "111",
                "name": "Spring sale",
                "status": "ENABLED",
                "serving": "ELIGIBLE",
                "type": "SEARCH",
                "bidding_strategy": "TARGET_SPEND",
                "daily_budget": 2.5,
                "budget_shared_by": 1,
                "start_date": "2026-03-01",
                "end_date": null,
                "impressions": 12000,
                "clicks": 300,
                "cost": 150,
                "ctr_percent": 2.5,
                "average_cpc": 0.5,
                "conversions": 12,
                "conversion_value": 960,
                "cost_per_conversion": 12.5,
            }],
            "truncated": false,
        })
    );

    let request = &google.fake.requests()[0];
    assert_eq!(
        request.url.path(),
        format!("/v25/customers/{GOOGLE_ADS_CUSTOMER}/googleAds:search")
    );
    // The manager account the sign-in acts through, without dashes.
    assert_eq!(
        request.header("login-customer-id").as_deref(),
        Some("9876543210")
    );
    assert!(request.query().contains("campaign.status != 'REMOVED'"));
    assert!(request.query().contains("segments.date DURING LAST_7_DAYS"));
    assert!(request.json.as_ref().unwrap().get("pageSize").is_none());
}

#[tokio::test]
async fn says_when_an_account_has_more_than_it_returned() {
    let google = Google::new(FakeGoogleAds::responding(|request| {
        let mut second = fixtures::campaign();
        second["campaign"]["id"] = json!("112");
        second["campaign"]["name"] = json!("Summer sale");
        request
            .query()
            .contains(" FROM campaign ")
            .then(|| google_json(&json!({ "results": [fixtures::campaign(), second] }), 200))
    }))
    .await;

    let data = google
        .call(
            "list_campaigns",
            json!({ "customer_id": GOOGLE_ADS_CUSTOMER, "limit": 1 }),
            &[],
        )
        .await
        .unwrap();

    assert_eq!(data["campaigns"].as_array().unwrap().len(), 1);
    assert_eq!(data["truncated"], true);
    // One row more than asked for is how Google is made to say there is more.
    assert!(google.fake.queries()[0].ends_with(" LIMIT 2"));
}

#[tokio::test]
async fn takes_a_custom_period_and_wants_both_of_its_ends() {
    let google = Google::new(FakeGoogleAds::new()).await;

    let custom = google
        .call(
            "get_performance",
            json!({
                "customer_id": GOOGLE_ADS_CUSTOMER,
                "level": "account",
                "segment": "date",
                "start_date": "2026-09-01",
                "end_date": "2026-09-30",
            }),
            &[],
        )
        .await
        .unwrap();
    assert_eq!(custom["period"], "2026-09-01 to 2026-09-30");
    assert!(
        google.fake.queries()[0].contains("segments.date BETWEEN '2026-09-01' AND '2026-09-30'")
    );
    assert!(google.fake.queries()[0].contains("ORDER BY segments.date"));

    let half = google
        .call(
            "list_campaigns",
            json!({ "customer_id": GOOGLE_ADS_CUSTOMER, "start_date": "2026-09-01" }),
            &[],
        )
        .await;
    assert_eq!(
        refused(half),
        "Set both start_date and end_date, or neither and use date_range"
    );

    let impossible = google
        .call(
            "list_campaigns",
            json!({ "customer_id": GOOGLE_ADS_CUSTOMER, "start_date": "2026-02-30", "end_date": "2026-03-01" }),
            &[],
        )
        .await;
    assert_eq!(
        refused(impossible),
        "start_date must be a date such as 2026-01-31"
    );
    assert_eq!(google.fake.queries().len(), 1);
}

#[tokio::test]
async fn keeps_agents_to_the_accounts_the_admin_listed() {
    let google = Google::new(FakeGoogleAds::new()).await;
    let listed = [("customerIds", "5555555555")];

    let refusal = refused(
        google
            .call(
                "list_campaigns",
                json!({ "customer_id": "123-456-7890" }),
                &listed,
            )
            .await,
    );
    assert!(
        refusal.contains("This MCP may not use the Google Ads account 123-456-7890. It is limited to: 555-555-5555."),
        "{refusal}"
    );
    assert_eq!(google.fake.requests().len(), 0);

    let data = google
        .call("list_accounts", json!({}), &listed)
        .await
        .unwrap();
    assert_eq!(data["accounts"], json!([]));
    assert_eq!(data["limited_to"], json!(["555-555-5555"]));
}

#[tokio::test]
async fn lists_the_accounts_of_the_sign_in_and_the_clients_of_its_managers() {
    let google = Google::new(FakeGoogleAds::responding(|request| {
        let path = request.url.path();
        if path == "/v25/customers:listAccessibleCustomers" {
            return Some(google_json(
                &json!({ "resourceNames": ["customers/9876543210", "customers/4444444444"] }),
                200,
            ));
        }
        if path.contains("/customers/4444444444/") {
            let not_enabled = GoogleAdsFault {
                error_code: ("authorizationError", "CUSTOMER_NOT_ENABLED"),
                message: "Not enabled.",
                fields: None,
            };
            return Some(google_ads_failure(&[not_enabled], 403));
        }
        if request.query().contains(" FROM customer_client ") {
            return Some(google_json(
                &json!({ "results": [{ "customerClient": fixtures::account() }] }),
                200,
            ));
        }
        if request.query().contains(" FROM customer ") {
            let mut manager = fixtures::account();
            manager["id"] = json!("9876543210");
            manager["manager"] = json!(true);
            return Some(google_json(
                &json!({ "results": [{ "customer": manager }] }),
                200,
            ));
        }
        None
    }))
    .await;

    let data = google.call("list_accounts", json!({}), &[]).await.unwrap();

    assert_eq!(data["accounts"][0]["customer_id"], "987-654-3210");
    assert_eq!(data["accounts"][0]["manager"], true);
    // An account that cannot be opened does not hide the others.
    assert_eq!(data["accounts"][1]["customer_id"], "444-444-4444");
    assert!(
        data["accounts"][1]["error"]
            .as_str()
            .unwrap()
            .contains("This Google Ads account is not active")
    );
    assert_eq!(
        data["client_accounts"],
        json!([{
            "customer_id": "123-456-7890",
            "name": "Acme Shoes",
            "currency": "EUR",
            "time_zone": "Europe/Paris",
            "manager": false,
            "test_account": false,
            "status": "ENABLED",
            "manager_id": "987-654-3210",
        }])
    );
}

#[tokio::test]
async fn says_what_google_refused_and_what_to_do_about_a_project_without_access() {
    let google = Google::new(FakeGoogleAds::responding(|request| {
        if request.query().contains("FROM keyword_view") {
            let no_access = GoogleAdsFault {
                error_code: (
                    "authorizationError",
                    "CLOUD_PROJECT_NOT_APPROVED_FOR_PRODUCTION",
                ),
                message: "The Cloud project is only approved for use with test accounts.",
                fields: None,
            };
            return Some(google_ads_failure(&[no_access], 403));
        }
        if request.query().contains("bogus") {
            let unknown = GoogleAdsFault {
                error_code: ("queryError", "UNRECOGNIZED_FIELD"),
                message: "Unrecognized field in the query: 'campaign.bogus'.",
                fields: None,
            };
            return Some(google_ads_failure(&[unknown], 400));
        }
        None
    }))
    .await;

    let no_access = refused(
        google
            .call(
                "list_keywords",
                json!({ "customer_id": GOOGLE_ADS_CUSTOMER }),
                &[],
            )
            .await,
    );
    assert!(
        no_access.contains("(CLOUD_PROJECT_NOT_APPROVED_FOR_PRODUCTION)"),
        "{no_access}"
    );
    assert!(
        no_access.contains("Apply for Explorer access on the Google Ads API Overview page"),
        "{no_access}"
    );

    let bad_query = google
        .call("run_query", json!({ "customer_id": GOOGLE_ADS_CUSTOMER, "query": "SELECT campaign.bogus FROM campaign" }), &[])
        .await;
    assert_eq!(
        refused(bad_query),
        "Google Ads refused the request: Unrecognized field in the query: 'campaign.bogus'. (UNRECOGNIZED_FIELD)"
    );

    let not_a_query = google
        .call(
            "run_query",
            json!({ "customer_id": GOOGLE_ADS_CUSTOMER, "query": "DELETE FROM campaign" }),
            &[],
        )
        .await;
    assert_eq!(
        refused(not_a_query),
        "query must be a Google Ads Query Language query starting with SELECT"
    );
}

#[tokio::test]
async fn asks_to_re_authorize_when_google_no_longer_knows_the_token() {
    let google = Google::new(FakeGoogleAds::responding(|request| {
        (request.url.host_str() == Some("googleads.googleapis.com")).then(|| {
            google_json(
                &json!({ "error": { "code": 401, "message": "Invalid credentials", "status": "UNAUTHENTICATED" } }),
                401,
            )
        })
    }))
    .await;

    let error = google
        .call("list_accounts", json!({}), &[])
        .await
        .unwrap_err();
    assert!(error.is_authorization_error());
    assert_eq!(
        error.to_string(),
        "Google rejected the saved authorization. Re-authorize this MCP in MyMCPs."
    );
}

/// The headers the TypeScript sends, and the one it must never send.
const HEADERS: [&str; 5] = [
    "accept",
    "authorization",
    "login-customer-id",
    "content-type",
    "developer-token",
];

/// The first and the last day `list_changes` asks for, when it looks `days` back.
fn days_of(days: i64) -> (String, String) {
    let now = Utc::now();
    let day = |offset: i64| {
        (now + Duration::days(offset))
            .format("%Y-%m-%d")
            .to_string()
    };
    (day(1 - days), day(1))
}

/// Run one call of the fixture, and say how it differs from what the TypeScript did.
async fn replay(core: &TestCore, case: &Value) -> Result<(), String> {
    let name = case["name"].as_str().unwrap();
    let definition = mymcps_google_ads::definition();
    let google = ScriptedGoogle::new(case["responses"].as_array().unwrap());
    let settings: Vec<(&str, &str)> = case["settings"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str().unwrap()))
        .collect();
    let context = google_ads_context(
        BuiltinEnv::new(core.core.clone()).with_fetcher(google.fetcher.clone()),
        &settings,
    );

    // The period of `list_changes` follows the clock: the fixture names its days.
    let days = case.get("days").and_then(Value::as_i64).map(days_of);
    let tool = provider(&definition)
        .tool(case["tool"].as_str().unwrap())
        .unwrap();
    let outcome = tool
        .run(case["arguments"].as_object().unwrap().clone(), context)
        .await;
    if days != case.get("days").and_then(Value::as_i64).map(days_of) {
        return Err("midnight".to_owned());
    }

    let requests = google.requests();
    let expected = case["requests"].as_array().unwrap();
    if requests.len() != expected.len() || google.unasked() != 0 {
        return Err(format!(
            "{name}: {} requests instead of {}",
            requests.len(),
            expected.len()
        ));
    }
    for (index, (request, expected)) in requests.iter().zip(expected).enumerate() {
        let said = format!("{name}: request {index}");
        if request.method.as_str() != expected["method"] || request.url.as_str() != expected["url"]
        {
            return Err(format!(
                "{said} is {} {}, not {} {}",
                request.method, request.url, expected["method"], expected["url"]
            ));
        }
        for header in HEADERS {
            if json!(request.header(header)) != expected["headers"][header] {
                return Err(format!(
                    "{said} has {header}: {:?}, not {}",
                    request.header(header),
                    expected["headers"][header]
                ));
            }
        }
        let body = match (expected["body"].as_str(), &days) {
            (Some(body), Some((from, to))) => {
                Some(body.replace("{FROM}", from).replace("{TO}", to))
            }
            (body, _) => body.map(str::to_owned),
        };
        if request.body.as_ref().map(|_| request.text()) != body {
            return Err(format!(
                "{said} sends\n  {}\ninstead of\n  {}",
                request.text(),
                body.unwrap_or_default()
            ));
        }
    }

    match (outcome, case.get("result"), case.get("error")) {
        (Ok(result), expected, _) => {
            // As text: the keys in their order, and every number as JavaScript writes it.
            let written = result.to_string();
            match expected.and_then(Value::as_str) {
                Some(expected) if written == expected => Ok(()),
                expected => Err(format!(
                    "{name} returns\n  {written}\ninstead of\n  {expected:?}"
                )),
            }
        }
        (Err(error), _, Some(expected)) => {
            let kind = match &error {
                BuiltinError::Authorization(_) => "authorization",
                BuiltinError::Tool(_) => "tool",
                BuiltinError::Internal(_) => "internal",
            };
            // What an agent is not told is worded by V8 there, and by this crate here.
            let same_words = kind == "internal" || error.to_string() == expected["message"];
            if kind == expected["kind"] && same_words {
                Ok(())
            } else {
                Err(format!(
                    "{name} fails as {kind} with\n  {error}\ninstead of {expected}"
                ))
            }
        }
        (Err(error), expected, _) => Err(format!(
            "{name} fails with\n  {error}\ninstead of returning {expected:?}"
        )),
    }
}

#[tokio::test]
async fn answers_as_the_typescript_does() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 79);
    let core = TestCore::new().await;

    let mut differences = Vec::new();
    for case in cases {
        let mut outcome = replay(&core, case).await;
        if outcome
            .as_ref()
            .is_err_and(|difference| difference == "midnight")
        {
            outcome = replay(&core, case).await;
        }
        differences.extend(outcome.err());
    }
    assert!(
        differences.is_empty(),
        "{} of {} calls differ:\n\n{}",
        differences.len(),
        cases.len(),
        differences.join("\n\n")
    );

    // Every read tool is called at least once.
    let definition = mymcps_google_ads::definition();
    for tool in definition.tools().iter().filter(|tool| !tool.write) {
        assert!(
            cases.iter().any(|case| case["tool"] == tool.name),
            "{} is never called",
            tool.name
        );
    }
}
