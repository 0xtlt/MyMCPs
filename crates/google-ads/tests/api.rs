//! Changes to an account, as the write tools will send them: against what
//! `mutateGoogleAds` of `app/services/builtin/google_ads/api.ts` does with
//! the same answers of Google, then in front of the fake Google the tests of
//! the tools use.
//!
//! `fixtures/mutations.json` was written by Node from the TypeScript. The
//! script that wrote it is not part of the repository, since it runs Node on
//! the TypeScript app. Reports are covered with the read tools, in
//! `read_tools.rs`.

mod support;

use mymcps_builtin::BuiltinEnv;
use mymcps_core::TestCore;
use mymcps_google_ads::api::{mutate_google_ads, resource_id};
use mymcps_google_ads::testing::{
    FakeGoogleAds, GOOGLE_ADS_CUSTOMER, GoogleAdsFault, google_ads_context, google_ads_failure,
};
use serde_json::{Value, json};
use support::{Google, ScriptedGoogle};

const FIXTURE: &str = include_str!("fixtures/mutations.json");

#[tokio::test]
async fn changes_an_account_as_the_typescript_does() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let mutations = fixture["mutations"].as_array().unwrap();
    assert_eq!(mutations.len(), 8);
    let core = TestCore::new().await;

    for case in mutations {
        let name = case["name"].as_str().unwrap();
        let google = ScriptedGoogle::new(case["responses"].as_array().unwrap());
        let context = google_ads_context(
            BuiltinEnv::new(core.core.clone()).with_fetcher(google.fetcher.clone()),
            &[],
        );

        let outcome = mutate_google_ads(
            &context,
            GOOGLE_ADS_CUSTOMER,
            case["operations"].as_array().unwrap(),
            case["validateOnly"].as_bool().unwrap(),
        )
        .await;

        let requests = google.requests();
        assert_eq!(requests.len(), 1, "{name}");
        let expected = &case["requests"][0];
        assert_eq!(requests[0].method.as_str(), expected["method"], "{name}");
        assert_eq!(requests[0].url.as_str(), expected["url"], "{name}");
        assert_eq!(
            requests[0].text(),
            expected["body"].as_str().unwrap(),
            "{name}"
        );
        match case.get("error") {
            Some(message) => {
                let error = outcome.unwrap_err();
                assert!(error.is_tool_error(), "{name}: {error}");
                assert_eq!(error.to_string(), message.as_str().unwrap(), "{name}");
            }
            None => assert_eq!(json!(outcome.unwrap()), case["result"], "{name}"),
        }
    }
}

#[tokio::test]
async fn makes_changes_and_says_what_each_one_made() {
    let google = Google::new(FakeGoogleAds::new()).await;
    let customer = format!("customers/{GOOGLE_ADS_CUSTOMER}");
    let operations = [
        json!({ "campaignBudgetOperation": { "create": { "resourceName": format!("{customer}/campaignBudgets/-1"), "amountMicros": "12500000" } } }),
        json!({ "campaignOperation": { "create": { "resourceName": format!("{customer}/campaigns/-2"), "name": "Autumn sale" } } }),
        json!({ "campaignCriterionOperation": { "remove": format!("{customer}/campaignCriteria/111~2056") } }),
        json!({ "adGroupOperation": { "update": { "resourceName": format!("{customer}/adGroups/333"), "name": "Trail shoes" }, "updateMask": "name" } }),
        json!({ "adGroupCriterionOperation": { "create": { "adGroup": format!("{customer}/adGroups/333") } } }),
    ];

    let made = mutate_google_ads(
        &google.context(&[]),
        GOOGLE_ADS_CUSTOMER,
        &operations,
        false,
    )
    .await
    .unwrap();

    // The fake Google numbers what it makes from 9000, by the place of the change, and
    // names it roughly: the tools only read the number.
    assert_eq!(
        made,
        [
            Some(format!("{customer}/campaignBudgets/9000")),
            Some(format!("{customer}/campaigns/9001")),
            Some(format!("{customer}/campaignCriteria/111~2056")),
            Some(format!("{customer}/adGroups/333")),
            Some(format!("{customer}/adGroupCriterions/9004")),
        ]
    );
    assert_eq!(resource_id(made[1].as_deref()).as_deref(), Some("9001"));
    assert_eq!(resource_id(made[2].as_deref()).as_deref(), Some("2056"));
    assert_eq!(google.fake.mutations(), [json!(operations)]);
    assert!(google.fake.validations().is_empty());
    let request = &google.fake.requests()[0];
    assert_eq!(
        request.url.path(),
        format!("/v25/customers/{GOOGLE_ADS_CUSTOMER}/googleAds:mutate")
    );
    assert!(request.json.as_ref().unwrap().get("validateOnly").is_none());
}

#[tokio::test]
async fn has_google_check_changes_without_making_them() {
    let google = Google::new(FakeGoogleAds::new()).await;
    let budget_change = [json!({
        "campaignBudgetOperation": {
            "update": { "resourceName": format!("customers/{GOOGLE_ADS_CUSTOMER}/campaignBudgets/222"), "amountMicros": "250000000" },
            "updateMask": "amount_micros",
        },
    })];

    let made = mutate_google_ads(
        &google.context(&[]),
        GOOGLE_ADS_CUSTOMER,
        &budget_change,
        true,
    )
    .await
    .unwrap();

    assert!(made.is_empty());
    assert_eq!(google.fake.validations(), [json!(budget_change)]);
    assert!(google.fake.mutations().is_empty());
    assert_eq!(
        google.fake.requests()[0].json.as_ref().unwrap()["validateOnly"],
        true
    );
}

#[tokio::test]
async fn says_where_in_a_change_google_found_a_mistake() {
    let google = Google::new(FakeGoogleAds::responding(|request| {
        let too_large = GoogleAdsFault {
            error_code: ("campaignBudgetError", "MONEY_AMOUNT_TOO_LARGE"),
            message: "The amount is too large.",
            fields: Some(vec![
                "mutate_operations",
                "campaign_budget_operation",
                "update",
                "amount_micros",
            ]),
        };
        request
            .url
            .path()
            .ends_with("/googleAds:mutate")
            .then(|| google_ads_failure(&[too_large], 400))
    }))
    .await;

    let error = mutate_google_ads(
        &google.context(&[]),
        GOOGLE_ADS_CUSTOMER,
        &[json!({})],
        true,
    )
    .await
    .unwrap_err();

    assert!(error.is_tool_error());
    assert_eq!(
        error.to_string(),
        "Google Ads refused the request: The amount is too large. (MONEY_AMOUNT_TOO_LARGE) at mutate_operations.campaign_budget_operation.update.amount_micros"
    );
}
