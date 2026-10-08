//! What Google Ads says of an account, a campaign, an ad group, a keyword, an
//! ad, a language, a place and an image, against what the TypeScript reads
//! from the same answers.
//!
//! `fixtures/lookups.json` holds the calls, run by Node through
//! `app/services/builtin/google_ads/lookup.ts` in front of a Google that
//! answers from a script: the queries each made and the facts it returned,
//! or why it failed. The TypeScript tests reach these functions through the
//! write tools. The script that wrote the fixture is not part of the
//! repository, since it runs Node on the TypeScript app.
//!
//! The facts are compared as the JSON the TypeScript holds. A field it holds
//! as `undefined` is written `"undefined"` on both sides: the text a missing
//! name is here, and what stands for an amount or a day that is not set.

mod support;

use mymcps_builtin::{BuiltinEnv, BuiltinResult, BuiltinToolContext};
use mymcps_core::TestCore;
use mymcps_google_ads::lookup::{
    AccountFacts, AdFacts, AdGroupFacts, CampaignFacts, ImageAssetFacts, KeywordFacts,
    LanguageFacts, LocationFacts, account_facts, ad_facts, ad_group_facts, campaign_facts,
    image_assets_by_id, keyword_facts, languages_by_code, locations_by_id,
};
use mymcps_google_ads::testing::{GOOGLE_ADS_CUSTOMER, google_ads_context};
use mymcps_vine as vine;
use serde_json::{Value, json};
use support::ScriptedGoogle;

const FIXTURE: &str = include_str!("fixtures/lookups.json");

fn amount(amount: Option<f64>) -> Value {
    amount.map_or_else(|| json!("undefined"), vine::js::number)
}

fn text(text: &Option<String>) -> Value {
    json!(text.as_deref().unwrap_or("undefined"))
}

fn account(facts: &AccountFacts) -> Value {
    json!({
        "customer": facts.customer,
        "label": facts.label,
        "currency": facts.currency,
        "timeZone": facts.time_zone,
    })
}

fn campaign(facts: &CampaignFacts) -> Value {
    json!({
        "resourceName": facts.resource_name,
        "id": facts.id,
        "name": facts.name,
        "status": facts.status,
        "channel": facts.channel,
        "biddingStrategy": facts.bidding_strategy,
        "startDate": text(&facts.start_date),
        "endDate": text(&facts.end_date),
        "searchPartners": facts.search_partners,
        "displayNetwork": facts.display_network,
        "maxCpc": amount(facts.max_cpc),
        "targetCpa": amount(facts.target_cpa),
        "targetRoas": amount(facts.target_roas),
        "budget": {
            "resourceName": facts.budget.resource_name,
            "amount": vine::js::number(facts.budget.amount),
            "campaigns": vine::js::number(facts.budget.campaigns),
        },
        "account": account(&facts.account),
    })
}

fn ad_group(facts: &AdGroupFacts) -> Value {
    json!({
        "resourceName": facts.resource_name,
        "id": facts.id,
        "name": facts.name,
        "status": facts.status,
        "maxCpc": amount(facts.max_cpc),
        "campaign": {
            "id": facts.campaign.id,
            "name": facts.campaign.name,
            "status": facts.campaign.status,
            "channel": facts.campaign.channel,
        },
        "account": account(&facts.account),
    })
}

fn keyword(facts: &KeywordFacts) -> Value {
    json!({
        "resourceName": facts.resource_name,
        "text": facts.text,
        "matchType": facts.match_type,
        "negative": facts.negative,
        "status": facts.status,
        "maxCpc": amount(facts.max_cpc),
        "adGroup": ad_group(&facts.ad_group),
    })
}

fn ad(facts: &AdFacts) -> Value {
    json!({
        "resourceName": facts.resource_name,
        "status": facts.status,
        "type": facts.ad_type,
        "headline": text(&facts.headline),
        "adGroup": ad_group(&facts.ad_group),
    })
}

fn language(facts: &LanguageFacts) -> Value {
    json!({ "id": facts.id, "code": facts.code, "name": facts.name })
}

fn location(facts: &LocationFacts) -> Value {
    json!({ "id": facts.id, "name": facts.name })
}

fn image(facts: &ImageAssetFacts) -> Value {
    json!({
        "id": facts.id,
        "resourceName": facts.resource_name,
        "name": facts.name,
        "width": vine::js::number(facts.width),
        "height": vine::js::number(facts.height),
    })
}

/// Make the call the fixture names, and write its facts as the TypeScript holds them.
async fn look_up(
    context: &BuiltinToolContext,
    lookup: &str,
    arguments: &[Value],
) -> BuiltinResult<Value> {
    let customer = GOOGLE_ADS_CUSTOMER;
    let id = |index: usize| arguments[index].as_str().unwrap();
    let list = || -> Vec<String> {
        arguments[0]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item.as_str().unwrap().to_owned())
            .collect()
    };
    Ok(match lookup {
        "accountFacts" => account(&account_facts(context, customer).await?),
        "campaignFacts" => campaign(&campaign_facts(context, customer, id(0)).await?),
        "adGroupFacts" => ad_group(&ad_group_facts(context, customer, id(0)).await?),
        "keywordFacts" => keyword(&keyword_facts(context, customer, id(0), id(1)).await?),
        "adFacts" => ad(&ad_facts(context, customer, id(0), id(1)).await?),
        "languagesByCode" => languages_by_code(context, customer, &list())
            .await?
            .iter()
            .map(language)
            .collect(),
        "locationsById" => locations_by_id(context, customer, &list())
            .await?
            .iter()
            .map(location)
            .collect(),
        "imageAssetsById" => image_assets_by_id(context, customer, &list())
            .await?
            .iter()
            .map(image)
            .collect(),
        other => panic!("no lookup {other}"),
    })
}

#[tokio::test]
async fn reads_what_google_ads_says_as_the_typescript_does() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let lookups = fixture["lookups"].as_array().unwrap();
    assert_eq!(lookups.len(), 28);
    let core = TestCore::new().await;

    for case in lookups {
        let name = case["name"].as_str().unwrap();
        let google = ScriptedGoogle::new(case["responses"].as_array().unwrap());
        let context = google_ads_context(
            BuiltinEnv::new(core.core.clone()).with_fetcher(google.fetcher.clone()),
            &[],
        );

        let outcome = look_up(
            &context,
            case["lookup"].as_str().unwrap(),
            case["arguments"].as_array().unwrap(),
        )
        .await;

        let requests = google.requests();
        let expected = case["requests"].as_array().unwrap();
        assert_eq!(requests.len(), expected.len(), "requests of {name}");
        assert_eq!(google.unasked(), 0, "answers of {name}");
        for (request, expected) in requests.iter().zip(expected) {
            assert_eq!(request.method.as_str(), expected["method"], "{name}");
            assert_eq!(request.url.as_str(), expected["url"], "{name}");
            // To the character: the query is what Google is asked.
            assert_eq!(request.text(), expected["body"].as_str().unwrap(), "{name}");
        }
        match case.get("error") {
            Some(message) => {
                let error = outcome.unwrap_err();
                assert!(error.is_tool_error(), "{name}: {error}");
                assert_eq!(error.to_string(), message.as_str().unwrap(), "{name}");
            }
            None => assert_eq!(outcome.unwrap(), case["facts"], "{name}"),
        }
    }
}
