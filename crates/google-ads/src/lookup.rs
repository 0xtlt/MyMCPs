//! What Google Ads says about the things a call names, read before the call
//! changes them. The tools build their requests from it, and the people who
//! approve a call read it: names and current values never come from the agent.
//!
//! A text Google left out of a row reads `undefined` here, as the template
//! strings of the TypeScript wrote it. It does not happen for the fields
//! these queries select: Google Ads has them for every resource.

use std::collections::HashMap;

use mymcps_builtin::{BuiltinError, BuiltinResult, BuiltinToolContext};
use mymcps_vine as vine;
use serde_json::Value;

use crate::api::{GoogleAdsRow, search_google_ads};
use crate::format::{customer_label, from_micros};
use crate::js;

#[derive(Debug, Clone, PartialEq)]
pub struct AccountFacts {
    pub customer: String,
    /// As a person recognizes it: `Acme Shoes (123-456-7890)`.
    pub label: String,
    pub currency: String,
    pub time_zone: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CampaignFacts {
    pub resource_name: String,
    pub id: String,
    pub name: String,
    pub status: String,
    pub channel: String,
    pub bidding_strategy: String,
    /// A day as `YYYY-MM-DD`, in the account's time zone.
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub search_partners: bool,
    pub display_network: bool,
    pub max_cpc: Option<f64>,
    pub target_cpa: Option<f64>,
    pub target_roas: Option<f64>,
    pub budget: BudgetFacts,
    pub account: AccountFacts,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BudgetFacts {
    pub resource_name: String,
    /// A day's budget in the account's currency.
    pub amount: f64,
    /// How many campaigns spend from it.
    pub campaigns: f64,
}

/// What an ad group says of its campaign.
#[derive(Debug, Clone, PartialEq)]
pub struct AdGroupCampaign {
    pub id: String,
    pub name: String,
    pub status: String,
    pub channel: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdGroupFacts {
    pub resource_name: String,
    pub id: String,
    pub name: String,
    pub status: String,
    pub max_cpc: Option<f64>,
    pub campaign: AdGroupCampaign,
    pub account: AccountFacts,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeywordFacts {
    pub resource_name: String,
    pub text: String,
    pub match_type: String,
    pub negative: bool,
    pub status: String,
    pub max_cpc: Option<f64>,
    pub ad_group: AdGroupFacts,
}

const ACCOUNT_FIELDS: &str =
    "customer.id, customer.descriptive_name, customer.currency_code, customer.time_zone";
const CAMPAIGN_FIELDS: &str = "campaign.resource_name, campaign.id, campaign.name, campaign.status, campaign.advertising_channel_type";

fn ad_group_fields() -> String {
    format!(
        "ad_group.resource_name, ad_group.id, ad_group.name, ad_group.status, ad_group.cpc_bid_micros, {CAMPAIGN_FIELDS}, {ACCOUNT_FIELDS}"
    )
}

/// A text of a row.
fn text(value: Option<&Value>) -> String {
    js::string(value)
}

/// `value ?? fallback` for a text of a row.
fn text_or(value: Option<&Value>, fallback: &str) -> String {
    js::defined(value).map_or_else(|| fallback.to_owned(), vine::js::to_string)
}

/// An amount Google answers 0 for when it is not set.
fn set_amount(micros: Option<&Value>) -> Option<f64> {
    from_micros(micros).filter(|amount| *amount != 0.0)
}

fn account_of(customer: &str, row: &GoogleAdsRow) -> AccountFacts {
    let account = row.get("customer");
    let name = js::get(account, "descriptiveName");
    AccountFacts {
        customer: customer.to_owned(),
        label: if js::truthy(name) {
            format!("{} ({})", js::string(name), customer_label(customer))
        } else {
            customer_label(customer)
        },
        currency: text_or(js::get(account, "currencyCode"), "USD"),
        time_zone: text_or(js::get(account, "timeZone"), "UTC"),
    }
}

/// The day of a `yyyy-MM-dd HH:mm:ss` timestamp, which is how Google dates a campaign.
fn day_of(timestamp: Option<&Value>) -> BuiltinResult<Option<String>> {
    let day = js::slice_of(timestamp, 10, "a date that is not text")?;
    Ok(day.filter(|day| !day.is_empty()).map(str::to_owned))
}

fn ad_group_of(customer: &str, row: &GoogleAdsRow) -> BuiltinResult<AdGroupFacts> {
    let ad_group = js::resource(row, "adGroup")?;
    let campaign = js::resource(row, "campaign")?;
    Ok(AdGroupFacts {
        resource_name: text(js::get(ad_group, "resourceName")),
        id: text(js::get(ad_group, "id")),
        name: text(js::get(ad_group, "name")),
        status: text(js::get(ad_group, "status")),
        max_cpc: set_amount(js::get(ad_group, "cpcBidMicros")),
        campaign: AdGroupCampaign {
            id: text(js::get(campaign, "id")),
            name: text(js::get(campaign, "name")),
            status: text(js::get(campaign, "status")),
            channel: text(js::get(campaign, "advertisingChannelType")),
        },
        account: account_of(customer, row),
    })
}

async fn only(
    context: &BuiltinToolContext,
    customer: &str,
    query: &str,
    missing: &str,
) -> BuiltinResult<GoogleAdsRow> {
    let found = search_google_ads(context, customer, query, 1).await?;
    found.rows.into_iter().next().ok_or_else(|| {
        BuiltinError::tool(format!(
            "{missing} in the Google Ads account {}",
            customer_label(customer)
        ))
    })
}

pub async fn account_facts(
    context: &BuiltinToolContext,
    customer: &str,
) -> BuiltinResult<AccountFacts> {
    let row = only(
        context,
        customer,
        &format!("SELECT {ACCOUNT_FIELDS} FROM customer LIMIT 1"),
        "Nothing was found",
    )
    .await?;
    Ok(account_of(customer, &row))
}

pub async fn campaign_facts(
    context: &BuiltinToolContext,
    customer: &str,
    campaign_id: &str,
) -> BuiltinResult<CampaignFacts> {
    let row = only(
        context,
        customer,
        &format!(
            "SELECT {CAMPAIGN_FIELDS}, campaign.bidding_strategy_type, campaign.start_date_time, campaign.end_date_time, campaign.network_settings.target_search_network, campaign.network_settings.target_content_network, campaign.target_spend.cpc_bid_ceiling_micros, campaign.maximize_conversions.target_cpa_micros, campaign.maximize_conversion_value.target_roas, campaign.campaign_budget, campaign_budget.amount_micros, campaign_budget.reference_count, {ACCOUNT_FIELDS} FROM campaign WHERE campaign.id = {campaign_id} LIMIT 1"
        ),
        &format!("There is no campaign {campaign_id}"),
    )
    .await?;
    let campaign = js::resource(&row, "campaign")?;
    let budget = row.get("campaignBudget");
    let networks = js::get(campaign, "networkSettings");
    let target_roas = js::get(js::get(campaign, "maximizeConversionValue"), "targetRoas");
    Ok(CampaignFacts {
        resource_name: text(js::get(campaign, "resourceName")),
        id: text(js::get(campaign, "id")),
        name: text(js::get(campaign, "name")),
        status: text(js::get(campaign, "status")),
        channel: text(js::get(campaign, "advertisingChannelType")),
        bidding_strategy: text(js::get(campaign, "biddingStrategyType")),
        start_date: day_of(js::get(campaign, "startDateTime"))?,
        end_date: day_of(js::get(campaign, "endDateTime"))?,
        search_partners: js::truthy(js::get(networks, "targetSearchNetwork")),
        display_network: js::truthy(js::get(networks, "targetContentNetwork")),
        // Google answers 0 for a limit or a target that is not set.
        max_cpc: set_amount(js::get(
            js::get(campaign, "targetSpend"),
            "cpcBidCeilingMicros",
        )),
        target_cpa: set_amount(js::get(
            js::get(campaign, "maximizeConversions"),
            "targetCpaMicros",
        )),
        target_roas: target_roas
            .filter(|target| js::truthy(Some(target)))
            .map(|target| vine::js::to_number(Some(target))),
        budget: BudgetFacts {
            resource_name: text(js::get(campaign, "campaignBudget")),
            amount: from_micros(js::get(budget, "amountMicros")).unwrap_or(0.0),
            campaigns: js::number_or(js::get(budget, "referenceCount"), 1.0),
        },
        account: account_of(customer, &row),
    })
}

pub async fn ad_group_facts(
    context: &BuiltinToolContext,
    customer: &str,
    ad_group_id: &str,
) -> BuiltinResult<AdGroupFacts> {
    let row = only(
        context,
        customer,
        &format!(
            "SELECT {} FROM ad_group WHERE ad_group.id = {ad_group_id} LIMIT 1",
            ad_group_fields()
        ),
        &format!("There is no ad group {ad_group_id}"),
    )
    .await?;
    ad_group_of(customer, &row)
}

pub async fn keyword_facts(
    context: &BuiltinToolContext,
    customer: &str,
    ad_group_id: &str,
    criterion_id: &str,
) -> BuiltinResult<KeywordFacts> {
    let row = only(
        context,
        customer,
        &format!(
            "SELECT ad_group_criterion.resource_name, ad_group_criterion.keyword.text, ad_group_criterion.keyword.match_type, ad_group_criterion.negative, ad_group_criterion.status, ad_group_criterion.cpc_bid_micros, {} FROM ad_group_criterion WHERE ad_group.id = {ad_group_id} AND ad_group_criterion.criterion_id = {criterion_id} AND ad_group_criterion.type = 'KEYWORD' LIMIT 1",
            ad_group_fields()
        ),
        &format!("There is no keyword {criterion_id} in the ad group {ad_group_id}"),
    )
    .await?;
    let criterion = js::resource(&row, "adGroupCriterion")?;
    let keyword = js::get(criterion, "keyword");
    Ok(KeywordFacts {
        resource_name: text(js::get(criterion, "resourceName")),
        text: text_or(js::get(keyword, "text"), ""),
        match_type: text_or(js::get(keyword, "matchType"), ""),
        negative: js::truthy(js::get(criterion, "negative")),
        status: text(js::get(criterion, "status")),
        max_cpc: set_amount(js::get(criterion, "cpcBidMicros")),
        ad_group: ad_group_of(customer, &row)?,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdFacts {
    pub resource_name: String,
    pub status: String,
    /// `type` in the TypeScript: what kind of ad it is, such as `RESPONSIVE_SEARCH_AD`.
    pub ad_type: String,
    /// What the ad says first, to tell it from the others.
    pub headline: Option<String>,
    pub ad_group: AdGroupFacts,
}

pub async fn ad_facts(
    context: &BuiltinToolContext,
    customer: &str,
    ad_group_id: &str,
    ad_id: &str,
) -> BuiltinResult<AdFacts> {
    let row = only(
        context,
        customer,
        &format!(
            "SELECT ad_group_ad.resource_name, ad_group_ad.status, ad_group_ad.ad.type, ad_group_ad.ad.responsive_search_ad.headlines, ad_group_ad.ad.responsive_display_ad.headlines, {} FROM ad_group_ad WHERE ad_group.id = {ad_group_id} AND ad_group_ad.ad.id = {ad_id} LIMIT 1",
            ad_group_fields()
        ),
        &format!("There is no ad {ad_id} in the ad group {ad_group_id}"),
    )
    .await?;
    let ad_group_ad = js::resource(&row, "adGroupAd")?;
    let ad = js::get(ad_group_ad, "ad");
    let texts = js::defined(js::get(ad, "responsiveSearchAd"))
        .or_else(|| js::get(ad, "responsiveDisplayAd"));
    let first = js::get(texts, "headlines")
        .and_then(Value::as_array)
        .and_then(|headlines| headlines.first());
    Ok(AdFacts {
        resource_name: text(js::get(ad_group_ad, "resourceName")),
        status: text(js::get(ad_group_ad, "status")),
        ad_type: text(js::get(ad, "type")),
        headline: js::defined(js::get(first, "text")).map(vine::js::to_string),
        ad_group: ad_group_of(customer, &row)?,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanguageFacts {
    pub id: String,
    pub code: String,
    pub name: String,
}

/// The languages Google Ads knows by these codes, in the order given. Fails
/// for a code it does not know, naming it.
pub async fn languages_by_code(
    context: &BuiltinToolContext,
    customer: &str,
    codes: &[String],
) -> BuiltinResult<Vec<LanguageFacts>> {
    if codes.is_empty() {
        return Ok(Vec::new());
    }

    let quoted: Vec<String> = codes.iter().map(|code| format!("'{code}'")).collect();
    let found = search_google_ads(
        context,
        customer,
        &format!(
            "SELECT language_constant.id, language_constant.code, language_constant.name FROM language_constant WHERE language_constant.code IN ({})",
            quoted.join(", ")
        ),
        codes.len(),
    )
    .await?;
    let mut known = HashMap::new();
    for row in &found.rows {
        let language = js::resource(row, "languageConstant")?;
        let code = text(js::get(language, "code"));
        known.insert(
            code.to_lowercase(),
            LanguageFacts {
                id: text(js::get(language, "id")),
                code,
                name: text(js::get(language, "name")),
            },
        );
    }
    codes
        .iter()
        .map(|code| {
            known.get(&code.to_lowercase()).cloned().ok_or_else(|| {
                BuiltinError::tool(format!(
                    "Google Ads has no language with the code \"{code}\". Codes look like en, fr, or pt_BR."
                ))
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationFacts {
    pub id: String,
    pub name: String,
}

/// The places behind these location IDs, in the order given. Fails for an ID
/// Google Ads does not know, so nobody approves a place they cannot read.
pub async fn locations_by_id(
    context: &BuiltinToolContext,
    customer: &str,
    ids: &[String],
) -> BuiltinResult<Vec<LocationFacts>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let found = search_google_ads(
        context,
        customer,
        &format!(
            "SELECT geo_target_constant.id, geo_target_constant.canonical_name, geo_target_constant.name FROM geo_target_constant WHERE geo_target_constant.id IN ({})",
            ids.join(", ")
        ),
        ids.len(),
    )
    .await?;
    let mut known = HashMap::new();
    for row in &found.rows {
        let place = js::resource(row, "geoTargetConstant")?;
        let name = js::defined(js::get(place, "canonicalName")).or_else(|| js::get(place, "name"));
        known.insert(text(js::get(place, "id")), text(name));
    }
    ids.iter()
        .map(|id| match known.get(id).filter(|name| !name.is_empty()) {
            Some(name) => Ok(LocationFacts { id: id.clone(), name: name.clone() }),
            None => Err(BuiltinError::tool(format!(
                "Google Ads has no location with the ID {id}. Find location IDs with search_locations."
            ))),
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageAssetFacts {
    pub id: String,
    pub resource_name: String,
    pub name: String,
    pub width: f64,
    pub height: f64,
}

/// The image assets behind these IDs, in the order given. Fails for one that is not an image of the account.
pub async fn image_assets_by_id(
    context: &BuiltinToolContext,
    customer: &str,
    ids: &[String],
) -> BuiltinResult<Vec<ImageAssetFacts>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let mut unique: Vec<&str> = Vec::new();
    for id in ids {
        if !unique.contains(&id.as_str()) {
            unique.push(id);
        }
    }
    let found = search_google_ads(
        context,
        customer,
        &format!(
            "SELECT asset.resource_name, asset.id, asset.name, asset.type, asset.image_asset.full_size.width_pixels, asset.image_asset.full_size.height_pixels FROM asset WHERE asset.id IN ({}) AND asset.type = 'IMAGE'",
            unique.join(", ")
        ),
        unique.len(),
    )
    .await?;
    let mut known = HashMap::new();
    for row in &found.rows {
        let asset = js::resource(row, "asset")?;
        let size = js::get(js::get(asset, "imageAsset"), "fullSize");
        let id = text(js::get(asset, "id"));
        known.insert(
            id.clone(),
            ImageAssetFacts {
                id,
                resource_name: text(js::get(asset, "resourceName")),
                name: text_or(js::get(asset, "name"), ""),
                width: js::number_or(js::get(size, "widthPixels"), 0.0),
                height: js::number_or(js::get(size, "heightPixels"), 0.0),
            },
        );
    }
    ids.iter()
        .map(|id| {
            known.get(id).cloned().ok_or_else(|| {
                BuiltinError::tool(format!(
                    "There is no image asset {id} in the Google Ads account {}. List them with list_assets, or add one with create_image_asset.",
                    customer_label(customer)
                ))
            })
        })
        .collect()
}
