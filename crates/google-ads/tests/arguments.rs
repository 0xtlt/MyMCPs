//! The arguments of the tools: the port of the last group of
//! `tests/unit/builtin_google_ads.spec.ts`, then every schema against what
//! the TypeScript answers.
//!
//! `fixtures/arguments.json` holds what the validators of
//! `app/validators/builtin_google_ads.ts` answer, run from that very file by
//! Node: each argument tried with some two hundred and fifty values while
//! the others stay valid, whole calls, how each schema reads as a JSON
//! Schema, and what the schemas for Google's answers make of them. The
//! script that wrote it is not part of the repository, since it runs Node on
//! the TypeScript app.

use std::sync::LazyLock;

use mymcps_builtin::tool_input::tool_input;
use mymcps_google_ads::validators::{
    ADD_CAMPAIGN_IMAGES_VALIDATOR, ADD_KEYWORDS_VALIDATOR, CAMPAIGN_VALIDATOR,
    CREATE_AD_GROUP_VALIDATOR, CREATE_CAMPAIGN_VALIDATOR, CREATE_DISPLAY_AD_VALIDATOR,
    CREATE_IMAGE_ASSET_VALIDATOR, CREATE_IMAGE_UPLOAD_LINK_VALIDATOR, CREATE_SEARCH_AD_VALIDATOR,
    GET_PERFORMANCE_VALIDATOR, GOOGLE_ADS_FAILURE_VALIDATOR, GOOGLE_ADS_MUTATION_VALIDATOR,
    GOOGLE_ADS_ROWS_VALIDATOR, GoogleAdsKeyword, GoogleAdsMatchType,
    IMAGE_UPLOAD_REFERENCE_VALIDATOR, KEYWORD_IDEAS_VALIDATOR, LIST_AD_GROUPS_VALIDATOR,
    LIST_ASSETS_VALIDATOR, LIST_CAMPAIGNS_VALIDATOR, LIST_CHANGES_VALIDATOR,
    LIST_IN_CAMPAIGN_VALIDATOR, RUN_QUERY_VALIDATOR, SEARCH_LOCATIONS_VALIDATOR,
    SET_AD_STATUS_VALIDATOR, SET_CAMPAIGN_STATUS_VALIDATOR, UPDATE_AD_GROUP_VALIDATOR,
    UPDATE_CAMPAIGN_BUDGET_VALIDATOR, UPDATE_CAMPAIGN_TARGETING_VALIDATOR,
    UPDATE_CAMPAIGN_VALIDATOR, UPDATE_KEYWORD_VALIDATOR,
};
use mymcps_vine as vine;
use serde::Deserialize;
use serde_json::{Value, json};
use vine::Validator;

const FIXTURE: &str = include_str!("fixtures/arguments.json");

static EXPECTED: LazyLock<Value> = LazyLock::new(|| serde_json::from_str(FIXTURE).unwrap());

fn campaign(overrides: Value) -> Value {
    let mut campaign = json!({
        "customer_id": "123-456-7890",
        "name": "Autumn sale",
        "channel": "SEARCH",
        "daily_budget": 12.5,
        "bidding_strategy": "MAXIMIZE_CLICKS",
    });
    campaign
        .as_object_mut()
        .unwrap()
        .extend(overrides.as_object().unwrap().clone());
    campaign
}

fn refusal(validator: &Validator, arguments: &Value) -> String {
    tool_input::<Value>(validator, arguments)
        .unwrap_err()
        .to_string()
}

#[test]
fn takes_identifiers_as_numbers_or_text_and_keeps_them_as_text() {
    #[derive(Deserialize)]
    struct Campaign {
        customer_id: String,
        location_ids: Vec<String>,
        languages: Vec<String>,
    }
    let arguments = campaign(
        json!({ "customer_id": 1234567890, "location_ids": [2250, "21167"], "languages": "fr" }),
    );
    let input: Campaign = tool_input(&CREATE_CAMPAIGN_VALIDATOR, &arguments).unwrap();

    assert_eq!(input.customer_id, "1234567890");
    assert_eq!(input.location_ids, ["2250", "21167"]);
    // A single value is a list of one.
    assert_eq!(input.languages, ["fr"]);
}

#[test]
fn writes_language_codes_the_way_google_ads_does() {
    let targeting = json!({ "customer_id": "1234567890", "campaign_id": "111", "add_languages": ["FR", "pt-br", "zh_cn"] });
    let input: Value = tool_input(&UPDATE_CAMPAIGN_TARGETING_VALIDATOR, &targeting).unwrap();
    assert_eq!(input["add_languages"], json!(["fr", "pt_BR", "zh_CN"]));

    let targeting =
        json!({ "customer_id": "1234567890", "campaign_id": "111", "add_languages": ["french"] });
    assert_eq!(
        refusal(&UPDATE_CAMPAIGN_TARGETING_VALIDATOR, &targeting),
        "add_languages must be a list of 1 to 30 language codes such as en, fr, or pt_BR"
    );
}

#[test]
fn refuses_dates_that_do_not_exist_and_budgets_out_of_range() {
    assert_eq!(
        refusal(
            &CREATE_CAMPAIGN_VALIDATOR,
            &campaign(json!({ "start_date": "2026-02-30" }))
        ),
        "start_date must be a date such as 2026-01-31"
    );
    assert_eq!(
        refusal(
            &CREATE_CAMPAIGN_VALIDATOR,
            &campaign(json!({ "start_date": "31/01/2026" }))
        ),
        "start_date must be a date such as 2026-01-31"
    );
    assert_eq!(
        refusal(
            &CREATE_CAMPAIGN_VALIDATOR,
            &campaign(json!({ "daily_budget": 2_000_000 }))
        ),
        "daily_budget must be a number between 0.01 and 1000000"
    );
    assert_eq!(
        refusal(
            &CREATE_CAMPAIGN_VALIDATOR,
            &campaign(json!({ "customer_id": "12345" }))
        ),
        "customer_id must be a Google Ads account ID such as 123-456-7890"
    );
}

#[test]
fn wants_a_match_type_for_every_keyword() {
    #[derive(Deserialize)]
    struct AddKeywords {
        keywords: Vec<GoogleAdsKeyword>,
    }
    let arguments = |keywords: Value| json!({ "customer_id": "1234567890", "ad_group_id": "333", "keywords": keywords });
    let input: AddKeywords = tool_input(
        &ADD_KEYWORDS_VALIDATOR,
        &arguments(json!([{ "text": " trail shoes ", "match_type": "EXACT", "max_cpc": "0.8" }])),
    )
    .unwrap();
    assert_eq!(
        input.keywords,
        [GoogleAdsKeyword {
            text: "trail shoes".into(),
            match_type: GoogleAdsMatchType::Exact,
            max_cpc: Some(0.8)
        }]
    );
    // As the TypeScript tool reads them: `{ text, matchType, maxCpc }`.
    assert_eq!(
        serde_json::to_value(&input.keywords).unwrap(),
        json!([{ "text": "trail shoes", "matchType": "EXACT", "maxCpc": 0.8 }])
    );

    for wrong in [
        json!([{ "text": "trail shoes" }]),
        json!([{ "text": "trail shoes", "match_type": "LOOSE" }]),
        json!([{ "text": "x".repeat(81), "match_type": "EXACT" }]),
        json!([{ "text": "trail shoes", "match_type": "EXACT", "max_cpc": 0 }]),
        json!(["trail shoes"]),
        json!([]),
    ] {
        let refused = refusal(&ADD_KEYWORDS_VALIDATOR, &arguments(wrong.clone()));
        assert!(
            refused.starts_with("keywords must be a list of 1 to 100 keywords such as"),
            "{wrong}: {refused}"
        );
    }
}

#[test]
fn checks_each_argument_as_the_typescript_does() {
    let pool = EXPECTED["pool"].as_array().unwrap();
    let fields = EXPECTED["fields"].as_array().unwrap();
    assert_eq!(fields.len(), 130);
    assert!(pool.len() >= 250);

    let mut checked = 0;
    for entry in fields {
        let (name, field) = (
            entry["validator"].as_str().unwrap(),
            entry["field"].as_str().unwrap(),
        );
        let validator = ported_validator(name);
        let described = &validator.to_json_schema()["properties"][field];
        assert_eq!(
            described.to_string(),
            entry["schema"].to_string(),
            "{name}.{field} as a JSON Schema"
        );

        let messages = entry["messages"].as_array().unwrap();
        for (value, expected) in pool.iter().zip(entry["outcomes"].as_array().unwrap()) {
            let value = value.as_array().unwrap().first();
            let mut arguments = entry["base"].as_object().unwrap().clone();
            match value {
                Some(value) => arguments.insert(field.to_owned(), value.clone()),
                None => arguments.remove(field),
            };
            let said = format!("{name}.{field} of {}", Value::Object(arguments.clone()));

            let outcome = tool_input::<Value>(validator, &Value::Object(arguments));
            match expected {
                Value::Number(message) => {
                    let message = messages[message.as_u64().unwrap() as usize]
                        .as_str()
                        .unwrap();
                    assert_eq!(
                        outcome.map_err(|error| error.to_string()),
                        Err(message.to_owned()),
                        "{said}"
                    );
                }
                Value::String(_) => assert_eq!(outcome.unwrap().get(field), value, "{said}"),
                changed => {
                    let output = outcome.unwrap_or_else(|error| panic!("{said}: {error}"));
                    assert_eq!(
                        output.get(field),
                        changed["ok"].as_array().unwrap().first(),
                        "{said}"
                    );
                }
            }
            checked += 1;
        }
    }
    assert_eq!(checked, fields.len() * pool.len());
}

/// The validator the TypeScript exports as `<name>Validator`.
fn ported_validator(name: &str) -> &'static Validator {
    match name {
        "listCampaigns" => &LIST_CAMPAIGNS_VALIDATOR,
        "campaign" => &CAMPAIGN_VALIDATOR,
        "listInCampaign" => &LIST_IN_CAMPAIGN_VALIDATOR,
        "listAdGroups" => &LIST_AD_GROUPS_VALIDATOR,
        "getPerformance" => &GET_PERFORMANCE_VALIDATOR,
        "listAssets" => &LIST_ASSETS_VALIDATOR,
        "listChanges" => &LIST_CHANGES_VALIDATOR,
        "searchLocations" => &SEARCH_LOCATIONS_VALIDATOR,
        "keywordIdeas" => &KEYWORD_IDEAS_VALIDATOR,
        "runQuery" => &RUN_QUERY_VALIDATOR,
        "createCampaign" => &CREATE_CAMPAIGN_VALIDATOR,
        "updateCampaign" => &UPDATE_CAMPAIGN_VALIDATOR,
        "setCampaignStatus" => &SET_CAMPAIGN_STATUS_VALIDATOR,
        "updateCampaignBudget" => &UPDATE_CAMPAIGN_BUDGET_VALIDATOR,
        "updateCampaignTargeting" => &UPDATE_CAMPAIGN_TARGETING_VALIDATOR,
        "createAdGroup" => &CREATE_AD_GROUP_VALIDATOR,
        "updateAdGroup" => &UPDATE_AD_GROUP_VALIDATOR,
        "addKeywords" => &ADD_KEYWORDS_VALIDATOR,
        "updateKeyword" => &UPDATE_KEYWORD_VALIDATOR,
        "createSearchAd" => &CREATE_SEARCH_AD_VALIDATOR,
        "createDisplayAd" => &CREATE_DISPLAY_AD_VALIDATOR,
        "setAdStatus" => &SET_AD_STATUS_VALIDATOR,
        "createImageUploadLink" => &CREATE_IMAGE_UPLOAD_LINK_VALIDATOR,
        "imageUploadReference" => &IMAGE_UPLOAD_REFERENCE_VALIDATOR,
        "createImageAsset" => &CREATE_IMAGE_ASSET_VALIDATOR,
        "addCampaignImages" => &ADD_CAMPAIGN_IMAGES_VALIDATOR,
        "googleAdsRows" => &GOOGLE_ADS_ROWS_VALIDATOR,
        "googleAdsMutation" => &GOOGLE_ADS_MUTATION_VALIDATOR,
        "googleAdsFailure" => &GOOGLE_ADS_FAILURE_VALIDATOR,
        other => panic!("no validator {other}"),
    }
}

#[test]
fn checks_whole_calls_as_the_typescript_does() {
    let calls = EXPECTED["whole"].as_array().unwrap();
    assert_eq!(calls.len(), 183);
    for call in calls {
        let name = call["validator"].as_str().unwrap();
        let validator = ported_validator(name);
        // No arguments at all is `undefined`.
        let arguments = call["arguments"].as_array().unwrap().first();
        let said = format!("{name} of {arguments:?}");

        let outcome = tool_input::<Value>(validator, arguments);
        match call["outcome"].get("error") {
            Some(message) => assert_eq!(
                outcome.unwrap_err().to_string(),
                message.as_str().unwrap(),
                "{said}"
            ),
            // As text: the arguments come out in the order the schema lists them.
            None => assert_eq!(
                outcome.unwrap().to_string(),
                call["outcome"]["ok"].to_string(),
                "{said}"
            ),
        }
    }
}

#[test]
fn describes_its_schemas_as_the_typescript_does() {
    let schemas = EXPECTED["schemas"].as_object().unwrap();
    assert_eq!(schemas.len(), 29);
    for (name, expected) in schemas {
        assert_eq!(
            ported_validator(name).to_json_schema().to_string(),
            expected.to_string(),
            "{name}"
        );
    }
}

#[test]
fn reads_what_google_answers_as_the_typescript_does() {
    let answers = EXPECTED["answers"].as_array().unwrap();
    assert_eq!(answers.len(), 41);
    for answer in answers {
        let name = answer["validator"].as_str().unwrap();
        let body = answer["body"].as_array().unwrap().first();
        let outcome = ported_validator(name).validate(body);
        match answer["outcome"].get("ok") {
            Some(output) => assert_eq!(
                outcome.unwrap().to_string(),
                output.to_string(),
                "{name} of {body:?}"
            ),
            None => assert!(outcome.is_err(), "{name} of {body:?}"),
        }
    }
}
