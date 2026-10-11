//! The write tools: the port of the "changes" group of
//! `tests/functional/builtin_google_ads_mcp.spec.ts`, then every write tool
//! against what the TypeScript does with the same answers of Google.
//!
//! In the TypeScript tests a call went through the gateway, which held it for
//! approval when the MCP asks. Here a test asks the tool itself for what the
//! runtime asks it: what the call would do (`describe`), then to do it
//! (`call`).
//!
//! `fixtures/write_tools.json` holds some hundred and ninety calls, run by
//! Node through the tools of
//! `app/services/builtin/google_ads/write_tools.ts` in front of a Google that
//! answers from a script. Each was described, then run: the fixture has the
//! reports it asked for and the change it sent, to the character, the
//! summary a person would read, and what the agent got back or why it
//! failed. It also has what the upload hook makes of a reference, and the
//! links `create_image_upload_link` hands out, apart from what changes with
//! every call. The script that wrote it is not part of the repository, since
//! it runs Node on the TypeScript app.

mod support;

use std::convert::Infallible;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::{Stream, stream};
use mymcps_builtin::file_link::{
    BUILTIN_FILE_PURPOSE, BUILTIN_UPLOAD_PURPOSE, decode_file_reference, has_valid_signature,
};
use mymcps_builtin::upload_store::is_builtin_upload_id;
use mymcps_builtin::{
    ApprovalDetail, BuiltinEnv, BuiltinError, BuiltinToolContext, BuiltinUploadTarget, UploadStore,
};
use mymcps_core::TestCore;
use mymcps_google_ads::testing::{
    FakeGoogleAds, GOOGLE_ADS_CUSTOMER, GoogleAdsFault, fixtures, google_ads_context,
    google_ads_failure, google_json,
};
use serde_json::{Value, json};
use support::{Google, ScriptedGoogle, provider};

const FIXTURE: &str = include_str!("fixtures/write_tools.json");

/// What an agent reads of a call that failed, once it is known to be told why.
fn refused<T>(outcome: Result<T, BuiltinError>) -> String {
    match outcome {
        Ok(_) => panic!("the call was not refused"),
        Err(error) => {
            assert!(error.is_tool_error(), "{error}");
            error.to_string()
        }
    }
}

/// A 1×1 PNG stretched in its header to the size a test needs: tools only read the header.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = vec![0; 33];
    bytes[..8].copy_from_slice(&[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    bytes[8..12].copy_from_slice(&13_u32.to_be_bytes());
    bytes[12..16].copy_from_slice(b"IHDR");
    bytes[16..20].copy_from_slice(&width.to_be_bytes());
    bytes[20..24].copy_from_slice(&height.to_be_bytes());
    bytes
}

fn chunks(bytes: Vec<u8>) -> impl Stream<Item = Result<Bytes, Infallible>> {
    stream::iter([Ok(Bytes::from(bytes))])
}

fn customer() -> String {
    format!("customers/{GOOGLE_ADS_CUSTOMER}")
}

/// The gateway lists the tools of an MCP that may not write without these.
#[test]
fn lists_no_write_tool_until_write_access_is_allowed() {
    let definition = mymcps_google_ads::definition();
    let tools = definition.tools();
    let reads: Vec<&str> = tools
        .iter()
        .filter(|tool| !tool.write)
        .map(|tool| tool.name)
        .collect();

    assert_eq!(reads.len(), 13);
    assert!(!reads.contains(&"update_campaign_budget"));
    assert_eq!(tools.len(), 28);
}

/// What the tools page of an MCP starts from, until the admin decides otherwise.
#[test]
fn asks_before_the_tools_that_commit_money() {
    let definition = mymcps_google_ads::definition();
    let asks: Vec<&str> = definition
        .tools()
        .iter()
        .filter(|tool| tool.asks_approval)
        .map(|tool| tool.name)
        .collect();

    assert_eq!(
        asks,
        [
            "create_campaign",
            "update_campaign",
            "set_campaign_status",
            "update_campaign_budget"
        ]
    );
}

#[tokio::test]
async fn describes_a_budget_change_from_what_google_says_not_from_what_the_agent_says() {
    let google = Google::new(FakeGoogleAds::new()).await;
    // The agent meant 2.50 and wrote 250.
    let call = json!({ "customer_id": "123-456-7890", "campaign_id": "111", "daily_budget": 250 });

    let summary = google
        .describe("update_campaign_budget", call.clone(), &[])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&summary).unwrap(),
        json!({
            "title": "Change the daily budget of the campaign \"Spring sale\" from €2.50 to €250.00",
            "details": [
                { "label": "Account", "value": "Acme Shoes (123-456-7890)" },
                { "label": "Campaign", "value": "Spring sale (enabled)" },
                { "label": "Daily budget", "value": "€250.00 a day", "before": "€2.50 a day" },
                { "label": "Most it can cost in a month", "value": "€7,600.00", "before": "€76.00" },
            ],
            "warnings": [
                "The new budget is 100 times the current one.",
                "The campaign is live: the new budget applies at once.",
            ],
        })
    );

    // Google checked the change without making it.
    let budget_change = json!([{
        "campaignBudgetOperation": {
            "update": {
                "resourceName": format!("{}/campaignBudgets/222", customer()),
                "amountMicros": "250000000",
            },
            "updateMask": "amount_micros",
        },
    }]);
    assert_eq!(
        google.fake.validations(),
        std::slice::from_ref(&budget_change)
    );
    assert!(google.fake.mutations().is_empty());

    // Once a person approved it, the runtime runs the very call it held.
    let approved = google
        .call("update_campaign_budget", call, &[])
        .await
        .unwrap();
    assert_eq!(
        approved,
        json!({
            "campaign_id": "111",
            "name": "Spring sale",
            "daily_budget": 250,
            "previous_daily_budget": 2.5,
            "currency": "EUR",
        })
    );
    assert_eq!(google.fake.mutations(), [budget_change]);
}

#[tokio::test]
async fn does_not_ask_anyone_to_approve_what_google_would_refuse() {
    let google = Google::new(FakeGoogleAds::responding(|request| {
        let checked = request.url.path().ends_with("/googleAds:mutate")
            && request
                .json
                .as_ref()
                .is_some_and(|json| json["validateOnly"] == true);
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
        checked.then(|| google_ads_failure(&[too_large], 400))
    }))
    .await;

    // What the runtime asks before it holds a call: a failure here is the answer of the call.
    let described = google
        .describe(
            "update_campaign_budget",
            json!({ "customer_id": GOOGLE_ADS_CUSTOMER, "campaign_id": "111", "daily_budget": 900000 }),
            &[],
        )
        .await;

    assert_eq!(
        refused(described),
        "Google Ads refused the request: The amount is too large. (MONEY_AMOUNT_TOO_LARGE) at mutate_operations.campaign_budget_operation.update.amount_micros"
    );
    assert_eq!(google.fake.validations().len(), 1);
    assert!(google.fake.mutations().is_empty());
}

#[tokio::test]
async fn creates_a_paused_campaign_with_its_own_budget_in_one_request() {
    let google = Google::new(FakeGoogleAds::responding(|request| {
        request.query().contains("FROM geo_target_constant").then(|| {
            google_json(
                &json!({ "results": [{ "geoTargetConstant": { "id": "2250", "canonicalName": "France", "name": "France" } }] }),
                200,
            )
        })
    }))
    .await;

    let data = google
        .call(
            "create_campaign",
            json!({
                "customer_id": GOOGLE_ADS_CUSTOMER,
                "name": "Autumn sale",
                "channel": "SEARCH",
                "daily_budget": 12.5,
                "bidding_strategy": "MAXIMIZE_CLICKS",
                "max_cpc": 1.2,
                "location_ids": [2250],
                "start_date": "2026-11-01",
                "end_date": "2026-11-30",
            }),
            &[],
        )
        .await
        .unwrap();

    assert_eq!(data["campaign_id"], "9001");
    assert_eq!(data["status"], "PAUSED");
    let customer = customer();
    let expected = json!([[
        {
            "campaignBudgetOperation": {
                "create": {
                    "resourceName": format!("{customer}/campaignBudgets/-1"),
                    "amountMicros": "12500000",
                    "deliveryMethod": "STANDARD",
                    "explicitlyShared": false,
                },
            },
        },
        {
            "campaignOperation": {
                "create": {
                    "resourceName": format!("{customer}/campaigns/-2"),
                    "name": "Autumn sale",
                    "status": "PAUSED",
                    "advertisingChannelType": "SEARCH",
                    "campaignBudget": format!("{customer}/campaignBudgets/-1"),
                    "networkSettings": {
                        "targetGoogleSearch": true,
                        "targetSearchNetwork": false,
                        "targetContentNetwork": false,
                        "targetPartnerSearchNetwork": false,
                    },
                    "targetSpend": { "cpcBidCeilingMicros": "1200000" },
                    "startDateTime": "2026-11-01 00:00:00",
                    "endDateTime": "2026-11-30 23:59:59",
                    "containsEuPoliticalAdvertising": "DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING",
                },
            },
        },
        {
            "campaignCriterionOperation": {
                "create": {
                    "campaign": format!("{customer}/campaigns/-2"),
                    "location": { "geoTargetConstant": "geoTargetConstants/2250" },
                },
            },
        },
    ]]);
    // As text: Google is sent the fields in the order the TypeScript writes them.
    assert_eq!(
        json!(google.fake.mutations()).to_string(),
        expected.to_string()
    );
}

#[tokio::test]
async fn refuses_settings_that_do_not_go_together_before_calling_google() {
    let google = Google::new(FakeGoogleAds::new()).await;
    let create = async |overrides: Value| {
        let mut campaign = json!({
            "customer_id": GOOGLE_ADS_CUSTOMER,
            "name": "Autumn sale",
            "channel": "SEARCH",
            "daily_budget": 12.5,
            "bidding_strategy": "MAXIMIZE_CLICKS",
        });
        campaign
            .as_object_mut()
            .unwrap()
            .extend(overrides.as_object().unwrap().clone());
        refused(google.call("create_campaign", campaign, &[]).await)
    };

    assert_eq!(
        create(json!({ "target_cpa": 20 })).await,
        "target_cpa does not go with MAXIMIZE_CLICKS, which takes max_cpc"
    );
    let languages = create(json!({ "languages": ["fr"] })).await;
    assert!(
        languages.contains("Google no longer takes languages for Search campaigns"),
        "{languages}"
    );
    assert_eq!(
        create(json!({ "channel": "DISPLAY", "search_partners": true })).await,
        "search_partners and display_network are for Search campaigns only"
    );
    assert_eq!(
        create(json!({ "daily_budget": 0 })).await,
        "daily_budget must be a number between 0.01 and 1000000"
    );
    assert_eq!(google.fake.requests().len(), 0);
}

#[tokio::test]
async fn describes_enabling_a_campaign_by_the_budget_it_frees() {
    let google = Google::new(FakeGoogleAds::responding(|request| {
        let mut paused = fixtures::campaign();
        paused["campaign"]["status"] = json!("PAUSED");
        request
            .query()
            .contains(" FROM campaign ")
            .then(|| google_json(&json!({ "results": [paused] }), 200))
    }))
    .await;

    let summary = google
        .describe(
            "set_campaign_status",
            json!({ "customer_id": GOOGLE_ADS_CUSTOMER, "campaign_id": 111, "status": "ENABLED" }),
            &[],
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        summary.title,
        "Enable the campaign \"Spring sale\", which can spend €2.50 a day"
    );
    assert!(
        summary
            .details
            .contains(&ApprovalDetail::new("Status", "Enabled").replacing("Paused")),
        "{:?}",
        summary.details
    );
}

#[tokio::test]
async fn removes_a_location_by_what_the_campaign_says_it_is_never_an_exclusion() {
    let criterion = |id: &str, negative: bool| {
        json!({
            "campaignCriterion": {
                "resourceName": format!("{}/campaignCriteria/111~{id}", customer()),
                "criterionId": id,
                "type": "LOCATION",
                "negative": negative,
                "location": { "geoTargetConstant": format!("geoTargetConstants/{id}") },
            },
        })
    };
    let google = Google::new(FakeGoogleAds::responding(move |request| {
        let query = request.query();
        if query.contains("FROM geo_target_constant") {
            let places: Vec<Value> = [("2250", "France"), ("2056", "Belgium")]
                .into_iter()
                .filter(|(id, _)| query.contains(id))
                .map(|(id, name)| json!({ "geoTargetConstant": { "id": id, "canonicalName": name } }))
                .collect();
            return Some(google_json(&json!({ "results": places }), 200));
        }
        query.contains("FROM campaign_criterion").then(|| {
            google_json(
                &json!({ "results": [criterion("2250", true), criterion("2056", false)] }),
                200,
            )
        })
    }))
    .await;
    let targeting = |location: &str| json!({ "customer_id": GOOGLE_ADS_CUSTOMER, "campaign_id": "111", "remove_location_ids": [location] });

    // The campaign excludes France: taking that away would open France to its ads.
    let exclusion = refused(
        google
            .call("update_campaign_targeting", targeting("2250"), &[])
            .await,
    );
    assert!(
        exclusion.contains("The campaign 111 excludes France"),
        "{exclusion}"
    );
    assert!(
        exclusion.contains("pass its criterion ID 2250 in remove_criterion_ids"),
        "{exclusion}"
    );
    assert!(google.fake.mutations().is_empty());

    let removed = google
        .call("update_campaign_targeting", targeting("2056"), &[])
        .await
        .unwrap();
    assert_eq!(
        removed,
        json!({ "campaign_id": "111", "added": 0, "removed": 1 })
    );
    assert_eq!(
        google.fake.mutations(),
        [json!([{
            "campaignCriterionOperation": { "remove": format!("{}/campaignCriteria/111~2056", customer()) },
        }])]
    );
}

#[tokio::test]
async fn adds_keywords_to_an_ad_group_and_returns_their_ids() {
    let google = Google::new(FakeGoogleAds::new()).await;

    let data = google
        .call(
            "add_keywords",
            json!({
                "customer_id": GOOGLE_ADS_CUSTOMER,
                "ad_group_id": "333",
                "keywords": [
                    { "text": " running shoes ", "match_type": "PHRASE", "max_cpc": 0.8 },
                    { "text": "trail shoes", "match_type": "EXACT" },
                ],
            }),
            &[],
        )
        .await
        .unwrap();

    assert_eq!(
        data["keywords"],
        json!([
            { "criterion_id": "9000", "text": "running shoes", "match_type": "PHRASE" },
            { "criterion_id": "9001", "text": "trail shoes", "match_type": "EXACT" },
        ])
    );
    let ad_group = format!("{}/adGroups/333", customer());
    let expected = json!([[
        {
            "adGroupCriterionOperation": {
                "create": {
                    "adGroup": ad_group,
                    "keyword": { "text": "running shoes", "matchType": "PHRASE" },
                    "status": "ENABLED",
                    "cpcBidMicros": "800000",
                },
            },
        },
        {
            "adGroupCriterionOperation": {
                "create": {
                    "adGroup": ad_group,
                    "keyword": { "text": "trail shoes", "matchType": "EXACT" },
                    "status": "ENABLED",
                },
            },
        },
    ]]);
    assert_eq!(
        json!(google.fake.mutations()).to_string(),
        expected.to_string()
    );

    let unmatched = google
        .call(
            "add_keywords",
            json!({ "customer_id": GOOGLE_ADS_CUSTOMER, "ad_group_id": "333", "keywords": [{ "text": "running shoes" }] }),
            &[],
        )
        .await;
    let unmatched = refused(unmatched);
    assert!(
        unmatched.contains("keywords must be a list of 1 to 100 keywords such as"),
        "{unmatched}"
    );
}

#[tokio::test]
async fn creates_a_search_ad_from_its_headlines_and_descriptions() {
    let google = Google::new(FakeGoogleAds::new()).await;
    let ad = |headlines: Value| {
        json!({
            "customer_id": GOOGLE_ADS_CUSTOMER,
            "ad_group_id": "333",
            "headlines": headlines,
            "descriptions": ["Light shoes for every road.", "Returns are free for 30 days."],
            "final_url": "https://acme.example/shoes",
            "path1": "shoes",
        })
    };

    let data = google
        .call(
            "create_responsive_search_ad",
            ad(json!([
                "Running shoes",
                "Free delivery",
                "Shop the spring sale"
            ])),
            &[],
        )
        .await
        .unwrap();
    assert_eq!(
        data,
        json!({ "ad_id": "9000", "ad_group_id": "333", "status": "ENABLED" })
    );
    assert_eq!(
        google.fake.mutations()[0][0]["adGroupAdOperation"]["create"]["ad"],
        json!({
            "finalUrls": ["https://acme.example/shoes"],
            "responsiveSearchAd": {
                "headlines": [
                    { "text": "Running shoes" },
                    { "text": "Free delivery" },
                    { "text": "Shop the spring sale" },
                ],
                "descriptions": [
                    { "text": "Light shoes for every road." },
                    { "text": "Returns are free for 30 days." },
                ],
                "path1": "shoes",
            },
        })
    );

    let short = google
        .call(
            "create_responsive_search_ad",
            ad(json!([
                "Running shoes",
                "A headline that runs well past the thirty characters"
            ])),
            &[],
        )
        .await;
    assert_eq!(
        refused(short),
        "headlines must be a list of 3 to 15 headlines of at most 30 characters each"
    );
}

#[tokio::test]
async fn turns_an_uploaded_image_into_an_asset_and_says_what_it_can_be_used_as() {
    let google = Google::new(FakeGoogleAds::new()).await;
    let context = google.context(&[]);

    let link = google
        .call(
            "create_image_upload_link",
            json!({ "filename": "banner.png" }),
            &[],
        )
        .await
        .unwrap();
    let url = url::Url::parse(link["url"].as_str().unwrap()).unwrap();
    let prefix = format!("http://localhost:3333/uploads/{}/", context.mcp_id);
    assert!(url.as_str().starts_with(&prefix), "{url}");
    assert_eq!(link["max_bytes"], 5_242_880);
    let upload_id = link["upload_id"].as_str().unwrap();

    // The route that takes the file asks the MCP what the link refers to, then keeps the file.
    let reference = decode_file_reference(
        url.path()
            .strip_prefix(prefix.trim_start_matches("http://localhost:3333"))
            .unwrap(),
    )
    .unwrap();
    let target = provider(&google.definition)
        .upload_target(reference, context.clone())
        .await
        .unwrap();
    assert_eq!(
        target,
        BuiltinUploadTarget {
            id: upload_id.to_owned(),
            filename: "banner.png".to_owned(),
            content_type: None,
            max_bytes: 5_242_880,
        }
    );
    let image = png(1200, 628);
    google
        .env
        .uploads
        .save(context.mcp_id, &target, chunks(image.clone()))
        .await
        .unwrap();

    let data = google
        .call(
            "create_image_asset",
            json!({ "customer_id": GOOGLE_ADS_CUSTOMER, "upload_id": upload_id, "name": "Spring banner" }),
            &[],
        )
        .await
        .unwrap();

    assert_eq!(
        data,
        json!({
            "asset_id": "9000",
            "name": "Spring banner",
            "width": 1200,
            "height": 628,
            "shape": "landscape",
            "large_enough": true,
        })
    );
    assert_eq!(
        google.fake.mutations(),
        [json!([{
            "assetOperation": {
                "create": {
                    "name": "Spring banner",
                    "type": "IMAGE",
                    "imageAsset": { "data": STANDARD.encode(&image) },
                },
            },
        }])]
    );

    let missing = google
        .call(
            "create_image_asset",
            json!({
                "customer_id": GOOGLE_ADS_CUSTOMER,
                "upload_id": "11111111-2222-4333-8444-555555555555",
                "name": "Nothing",
            }),
            &[],
        )
        .await;
    let missing = refused(missing);
    assert!(
        missing.contains("No file is uploaded as \"11111111-2222-4333-8444-555555555555\""),
        "{missing}"
    );
}

#[tokio::test]
async fn only_puts_images_of_the_right_shape_in_a_display_ad() {
    let image = |id: &str, width: u32, height: u32| {
        json!({
            "asset": {
                "resourceName": format!("{}/assets/{id}", customer()),
                "id": id,
                "name": format!("Image {id}"),
                "type": "IMAGE",
                "imageAsset": { "fullSize": { "widthPixels": width.to_string(), "heightPixels": height.to_string() } },
            },
        })
    };
    let google = Google::new(FakeGoogleAds::responding(move |request| {
        request.query().contains(" FROM asset ").then(|| {
            google_json(
                &json!({ "results": [image("71", 1200, 628), image("72", 600, 600)] }),
                200,
            )
        })
    }))
    .await;
    let ad = |landscape: &str, square: &str| {
        json!({
            "customer_id": GOOGLE_ADS_CUSTOMER,
            "ad_group_id": "333",
            "marketing_image_asset_ids": [landscape],
            "square_marketing_image_asset_ids": [square],
            "headlines": ["Running shoes"],
            "long_headline": "Light running shoes for every road",
            "descriptions": ["Returns are free for 30 days."],
            "business_name": "Acme Shoes",
            "final_url": "https://acme.example/shoes",
        })
    };

    let created = google
        .call("create_responsive_display_ad", ad("71", "72"), &[])
        .await
        .unwrap();
    assert_eq!(created["ad_id"], "9000");
    let display =
        &google.fake.mutations()[0][0]["adGroupAdOperation"]["create"]["ad"]["responsiveDisplayAd"];
    assert_eq!(
        display["marketingImages"],
        json!([{ "asset": format!("{}/assets/71", customer()) }])
    );
    assert_eq!(
        display["longHeadline"],
        json!({ "text": "Light running shoes for every road" })
    );

    let swapped = google
        .call("create_responsive_display_ad", ad("72", "71"), &[])
        .await;
    assert_eq!(
        refused(swapped),
        "marketing_image_asset_ids takes landscape (1.91:1) images of at least 600×314 pixels, and the image asset 72 is 600×600"
    );
    assert_eq!(google.fake.mutations().len(), 1);
}

/// The headers the TypeScript sends, and the one it must never send.
const HEADERS: [&str; 5] = [
    "accept",
    "authorization",
    "login-customer-id",
    "content-type",
    "developer-token",
];

/// Describe or run one call of the fixture, and say how it differs from what the TypeScript did.
async fn replay(core: &TestCore, case: &Value, mode: &str) -> Result<(), String> {
    let name = format!("{} ({mode})", case["name"].as_str().unwrap());
    let expected = &case[mode];
    let definition = mymcps_google_ads::definition();

    // Google answers the reports the call asks for, then the change itself.
    let mut answers = case["responses"].as_array().unwrap().clone();
    answers.extend(expected.get("answer").cloned());
    let google = ScriptedGoogle::new(&answers);
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
    let tool = provider(&definition)
        .tool(case["tool"].as_str().unwrap())
        .unwrap();
    let arguments = case["arguments"].as_object().unwrap().clone();

    // As text: the keys in their order, and every number as JavaScript writes it.
    let outcome = if mode == "run" {
        tool.run(arguments, context)
            .await
            .map(|result| Some(result.to_string()))
    } else {
        tool.describe_call(arguments, context)
            .await
            .map(|summary| summary.map(|summary| serde_json::to_string(&summary).unwrap()))
    };

    // Checking a change sends the very change, and says not to make it.
    let mut sent: Vec<(&Value, String)> = case["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|request| (request, request["body"].as_str().unwrap().to_owned()))
        .collect();
    if let Some(change) = case.get("change") {
        let body = change["body"].as_str().unwrap();
        sent.push((
            change,
            if mode == "run" {
                body.to_owned()
            } else {
                format!("{},\"validateOnly\":true}}", &body[..body.len() - 1])
            },
        ));
    }
    let requests = google.requests();
    if requests.len() != sent.len() || google.unasked() != 0 {
        return Err(format!(
            "{name}: {} requests instead of {}, {} answers left",
            requests.len(),
            sent.len(),
            google.unasked()
        ));
    }
    for (index, (request, (expected, body))) in requests.iter().zip(&sent).enumerate() {
        let said = format!("{name}: request {index}");
        if request.method.as_str() != expected["method"] || request.url.as_str() != expected["url"]
        {
            return Err(format!(
                "{said} is {} {}, not {} {}",
                request.method, request.url, expected["method"], expected["url"]
            ));
        }
        for header in HEADERS {
            if json!(request.header(header)) != case["headers"][header] {
                return Err(format!(
                    "{said} has {header}: {:?}, not {}",
                    request.header(header),
                    case["headers"][header]
                ));
            }
        }
        if request.text() != *body {
            return Err(format!(
                "{said} sends\n  {}\ninstead of\n  {body}",
                request.text()
            ));
        }
    }

    let answered = if mode == "run" { "result" } else { "summary" };
    match (outcome, expected.get(answered), expected.get("error")) {
        (Ok(written), Some(expected), _) => {
            if json!(written) == *expected {
                Ok(())
            } else {
                Err(format!(
                    "{name} answers\n  {}\ninstead of\n  {}",
                    written.unwrap_or_else(|| "nothing".to_owned()),
                    expected.as_str().unwrap_or("nothing")
                ))
            }
        }
        (Ok(written), None, _) => Err(format!(
            "{name} answers\n  {written:?}\ninstead of failing with {}",
            expected["error"]
        )),
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
        (Err(error), expected, None) => Err(format!(
            "{name} fails with\n  {error}\ninstead of answering {expected:?}"
        )),
    }
}

#[tokio::test]
async fn changes_accounts_as_the_typescript_does() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 190);
    let core = TestCore::new().await;
    let uploads = UploadStore::new(&core);
    let mcp_id = google_ads_context(BuiltinEnv::new(core.core.clone()), &[]).mcp_id;

    let mut differences = Vec::new();
    for case in cases {
        if let Some(upload) = case.get("upload") {
            let target = BuiltinUploadTarget {
                id: upload["id"].as_str().unwrap().to_owned(),
                filename: upload["filename"].as_str().unwrap().to_owned(),
                content_type: upload
                    .get("contentType")
                    .map(|content_type| content_type.as_str().unwrap().to_owned()),
                max_bytes: 5_242_880,
            };
            let bytes = STANDARD.decode(upload["bytes"].as_str().unwrap()).unwrap();
            uploads.save(mcp_id, &target, chunks(bytes)).await.unwrap();
        }
        // A call is described before anyone is asked, then run once it is approved.
        differences.extend(replay(&core, case, "describe").await.err());
        differences.extend(replay(&core, case, "run").await.err());
        uploads.remove_all(mcp_id).await.unwrap();
    }
    assert!(
        differences.is_empty(),
        "{} of {} calls differ:\n\n{}",
        differences.len(),
        cases.len() * 2,
        differences.join("\n\n")
    );

    // Every tool that changes an account is described and run at least once,
    // and refused at least once.
    let definition = mymcps_google_ads::definition();
    for tool in definition.tools().iter().filter(|tool| tool.write) {
        let of_tool = || cases.iter().filter(|case| case["tool"] == tool.name);
        assert!(
            of_tool().any(|case| case["run"].get("error").is_some()),
            "{} is never refused",
            tool.name
        );
        // Each link create_image_upload_link hands out is another: they are compared below.
        if tool.name == "create_image_upload_link" {
            continue;
        }
        assert!(
            of_tool().any(|case| case["describe"]["summary"].is_string()),
            "{} is never described",
            tool.name
        );
        assert!(
            of_tool().any(|case| case["run"]["result"].is_string()),
            "{} is never run",
            tool.name
        );
    }
}

/// What a tool of the MCP `mcp_id` runs with.
fn context_of(google: &Google, mcp_id: i64) -> Arc<BuiltinToolContext> {
    Arc::new(BuiltinToolContext {
        mcp_id,
        ..(*google.context(&[])).clone()
    })
}

#[tokio::test]
async fn hands_out_links_as_the_typescript_does() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let links = fixture["links"].as_array().unwrap();
    assert_eq!(links.len(), 3);
    let google = Google::new(FakeGoogleAds::new()).await;
    let context = context_of(&google, 7);
    let tool = provider(&google.definition)
        .tool("create_image_upload_link")
        .unwrap();

    for expected in links {
        let arguments = expected["arguments"].as_object().unwrap().clone();
        let said = format!("the link of {}", expected["arguments"]);
        // A link is nothing a person approves: there is nothing to say of it but its arguments.
        assert_eq!(
            tool.describe_call(arguments.clone(), context.clone())
                .await
                .unwrap(),
            None,
            "{said}"
        );

        let before = Utc::now();
        let mut link = tool.run(arguments, context.clone()).await.unwrap();
        let upload_id = link["upload_id"].as_str().unwrap().to_owned();
        let url = url::Url::parse(link["url"].as_str().unwrap()).unwrap();
        let expires_at = link["expires_at"].as_str().unwrap().to_owned();

        // A new upload ID for every link, which is what names the stored file.
        assert!(is_builtin_upload_id(&upload_id), "{said}: {upload_id}");
        assert_eq!(
            url.origin().ascii_serialization(),
            expected["origin"],
            "{said}"
        );
        let encoded = url
            .path()
            .strip_prefix(expected["path"].as_str().unwrap())
            .unwrap_or_else(|| panic!("{said}: {url}"));
        let reference = decode_file_reference(encoded).unwrap();
        assert_eq!(
            reference.to_string().replace(&upload_id, "{ID}"),
            expected["reference"],
            "{said}"
        );
        let query: Vec<String> = url
            .query_pairs()
            .map(|(name, _)| name.into_owned())
            .collect();
        assert_eq!(json!(query), expected["query"], "{said}");

        // The signature is for this path, to send a file to and nothing else.
        let signature = url
            .query_pairs()
            .next()
            .map(|(_, value)| value.into_owned());
        let core = &google.env.core;
        assert!(has_valid_signature(
            core,
            url.path(),
            BUILTIN_UPLOAD_PURPOSE,
            signature.as_deref()
        ));
        assert!(!has_valid_signature(
            core,
            url.path(),
            BUILTIN_FILE_PURPOSE,
            signature.as_deref()
        ));

        // `2026-10-07T18:30:00.000Z`, the minutes asked for from now.
        let expires = DateTime::parse_from_rfc3339(&expires_at).unwrap();
        assert_eq!(expires_at.len(), 24, "{said}: {expires_at}");
        assert!(expires_at.ends_with('Z'), "{said}: {expires_at}");
        let minutes = (expires.with_timezone(&Utc) - before).num_seconds() as f64 / 60.0;
        assert_eq!(json!(minutes.round() as i64), expected["minutes"], "{said}");
        assert_eq!(expected["expires"], true);

        // The file sent to the link is kept under the upload ID, as what the link says it is.
        let target = provider(&google.definition)
            .upload_target(reference.clone(), context.clone())
            .await
            .unwrap();
        assert_eq!(target.id, upload_id, "{said}");
        assert_eq!(json!(target.filename), link["filename"], "{said}");
        assert_eq!(
            json!(target.content_type),
            reference
                .get("content_type")
                .cloned()
                .unwrap_or(Value::Null),
            "{said}"
        );
        assert_eq!(target.max_bytes, 5_242_880, "{said}");

        for (key, placeholder) in [
            ("upload_id", "{ID}"),
            ("url", "{URL}"),
            ("expires_at", "{EXPIRES}"),
        ] {
            link[key] = json!(placeholder);
        }
        assert_eq!(
            link.to_string(),
            expected["result"].as_str().unwrap(),
            "{said}"
        );
    }
    assert!(google.fake.requests().is_empty());
}

#[tokio::test]
async fn a_link_needs_the_public_address_of_the_instance() {
    let core = TestCore::with_config(|config| config.app_url = None).await;
    let context = google_ads_context(BuiltinEnv::new(core.core.clone()), &[]);
    let definition = mymcps_google_ads::definition();
    let link = provider(&definition)
        .tool("create_image_upload_link")
        .unwrap()
        .run(
            json!({ "filename": "banner.png" })
                .as_object()
                .unwrap()
                .clone(),
            context,
        )
        .await;
    assert_eq!(
        refused(link),
        "File links need the public address of this MyMCPs instance. An administrator must set APP_URL."
    );
}

#[tokio::test]
async fn reads_what_an_upload_link_refers_to_as_the_typescript_does() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let references = fixture["references"].as_array().unwrap();
    assert_eq!(references.len(), 23);
    let google = Google::new(FakeGoogleAds::new()).await;

    for expected in references {
        // No reference at all is `undefined`, which a link never decodes to: `null` stands for it.
        let reference = expected["reference"]
            .as_array()
            .unwrap()
            .first()
            .cloned()
            .unwrap_or(Value::Null);
        let said = format!("the reference {reference}");
        let target = provider(&google.definition)
            .upload_target(reference, google.context(&[]))
            .await;
        match expected.get("error") {
            Some(message) => assert_eq!(refused(target), message.as_str().unwrap(), "{said}"),
            None => {
                let target = target.unwrap();
                let mut read = json!({ "id": target.id, "filename": target.filename });
                if let Some(content_type) = target.content_type {
                    read["contentType"] = json!(content_type);
                }
                read["maxBytes"] = json!(target.max_bytes);
                // As text: the fields in the order the TypeScript writes them.
                assert_eq!(read.to_string(), expected["target"].to_string(), "{said}");
            }
        }
    }
    // What a link refers to is read without asking Google anything.
    assert!(google.fake.requests().is_empty());
}
