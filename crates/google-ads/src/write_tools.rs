//! The tools that change an account: campaigns, budgets, bidding, targeting,
//! ad groups, keywords, ads, and images.
//!
//! The port of `app/services/builtin/google_ads/write_tools.ts`. Each tool
//! but the one that hands out upload links is a `write_tool`: it reads its
//! arguments into a `Plan`, which holds the changes Google Ads is sent and
//! what the person asked to approve them reads.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use chrono::Utc;
use mymcps_builtin::arguments::to_iso;
use mymcps_builtin::file_link::builtin_upload_url;
use mymcps_builtin::tool_input::tool_input;
use mymcps_builtin::upload_store::BUILTIN_UPLOAD_MINUTES;
use mymcps_builtin::{
    ApprovalDetail, ApprovalSummary, BuiltinError, BuiltinResult, BuiltinTool, BuiltinToolContext,
    BuiltinUploadTarget,
};
use mymcps_vine as vine;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use uuid::Uuid;
use vine::Validator;

use crate::api::{
    GoogleAdsOperation, GoogleAdsRow, customer_of, mutate_google_ads, resource_id,
    search_google_ads,
};
use crate::format::{money, to_fixed, to_micros};
use crate::images::{IMAGE_SHAPES, SQUARE_LOGO_MIN_PIXELS, image_info, image_shape};
use crate::js::{self, Object};
use crate::lookup::{
    AccountFacts, AdGroupFacts, ImageAssetFacts, account_facts, ad_facts, ad_group_facts,
    campaign_facts, image_assets_by_id, keyword_facts, languages_by_code, locations_by_id,
};
use crate::read_tools::{customer_id_property, input_schema, properties};
use crate::validators::{
    ADD_CAMPAIGN_IMAGES_VALIDATOR, ADD_KEYWORDS_VALIDATOR, CREATE_AD_GROUP_VALIDATOR,
    CREATE_CAMPAIGN_VALIDATOR, CREATE_DISPLAY_AD_VALIDATOR, CREATE_IMAGE_ASSET_VALIDATOR,
    CREATE_IMAGE_UPLOAD_LINK_VALIDATOR, CREATE_SEARCH_AD_VALIDATOR, GOOGLE_ADS_BIDDING_STRATEGIES,
    GOOGLE_ADS_CHANNELS, GOOGLE_ADS_MATCH_TYPES, GOOGLE_ADS_STATUSES, GoogleAdsKeyword,
    IMAGE_UPLOAD_REFERENCE_VALIDATOR, SET_AD_STATUS_VALIDATOR, SET_CAMPAIGN_STATUS_VALIDATOR,
    UPDATE_AD_GROUP_VALIDATOR, UPDATE_CAMPAIGN_BUDGET_VALIDATOR,
    UPDATE_CAMPAIGN_TARGETING_VALIDATOR, UPDATE_CAMPAIGN_VALIDATOR, UPDATE_KEYWORD_VALIDATOR,
    limits,
};

const MONEY: &str = "Amounts are in the account's currency.";
const DEFAULT_LINK_MINUTES: i64 = 15;
const MAX_IMAGE_MEGABYTES: u64 = limits::IMAGE_BYTES / 1_048_576;
/// Google never bills more in a month than the daily budget times this.
const DAYS_IN_A_MONTH: f64 = 30.4;
/// How many of a long list a person is shown one by one before being sent to the arguments.
const MAX_LISTED: usize = 30;
/// One page of a report, which holds the locations and languages of any campaign.
const MAX_CAMPAIGN_TARGETS: usize = 10_000;

type Tool = BuiltinTool<BuiltinToolContext>;

/// The resource name each operation produced, in their order.
type Created = [Option<String>];

/// What a tool is about to do: the changes Google Ads is sent, what the person
/// asked to approve them reads, and what the agent gets back. Both the changes
/// and the summary come from the same reading of the arguments.
struct Plan {
    customer: String,
    operations: Vec<GoogleAdsOperation>,
    summary: ApprovalSummary,
    result: Box<dyn FnOnce(&Created) -> Value + Send>,
}

/// A tool that changes an account. Before anyone is asked to approve a call,
/// Google checks the very changes it would make, without making them.
fn write_tool<I, F, Fut>(
    name: &'static str,
    description: impl Into<String>,
    input_schema: Value,
    input: &'static LazyLock<Validator>,
    plan: F,
) -> Tool
where
    I: DeserializeOwned + Send + 'static,
    F: Fn(I, Arc<BuiltinToolContext>) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = BuiltinResult<Plan>> + Send + 'static,
{
    let described = plan.clone();
    BuiltinTool::new(
        name,
        description,
        input_schema,
        input,
        move |input: I, context: Arc<BuiltinToolContext>| {
            let planned = plan(input, context.clone());
            async move {
                let Plan {
                    customer,
                    operations,
                    result,
                    ..
                } = planned.await?;
                let created = mutate_google_ads(&context, &customer, &operations, false).await?;
                Ok(result(&created))
            }
        },
    )
    .write()
    .describe(move |input: I, context: Arc<BuiltinToolContext>| {
        let planned = described(input, context.clone());
        async move {
            let Plan {
                customer,
                operations,
                summary,
                ..
            } = planned.await?;
            mutate_google_ads(&context, &customer, &operations, true).await?;
            Ok(Some(summary))
        }
    })
}

fn campaign_id_property() -> Value {
    json!({ "campaign_id": { "type": "string", "description": "Campaign ID, as returned by list_campaigns." } })
}

fn ad_group_id_property() -> Value {
    json!({ "ad_group_id": { "type": "string", "description": "Ad group ID, as returned by list_ad_groups." } })
}

fn bidding_properties() -> Value {
    json!({
        "max_cpc": {
            "type": "number",
            "minimum": 0.01,
            "maximum": js::json_number(limits::BID),
            "description": "With MAXIMIZE_CLICKS only: the most a click may cost.",
        },
        "target_cpa": {
            "type": "number",
            "minimum": 0.01,
            "maximum": js::json_number(limits::BID),
            "description": "With MAXIMIZE_CONVERSIONS only: the cost per conversion to aim at.",
        },
        "target_roas": {
            "type": "number",
            "minimum": 0.01,
            "maximum": js::json_number(limits::TARGET_ROAS),
            "description": "With MAXIMIZE_CONVERSION_VALUE only: the return on ad spend to aim at, as a ratio. 4 is 400%.",
        },
    })
}

/// A bid in the account's currency, as a tool describes the argument.
fn bid_property(description: &str) -> Value {
    json!({
        "type": "number",
        "minimum": 0.01,
        "maximum": js::json_number(limits::BID),
        "description": description,
    })
}

fn keyword_list_property(what: &str) -> Value {
    json!({
        "type": "array",
        "items": {
            "type": "object",
            "properties": {
                "text": { "type": "string", "maxLength": limits::KEYWORD_LENGTH },
                "match_type": { "type": "string", "enum": GOOGLE_ADS_MATCH_TYPES },
                "max_cpc": { "type": "number", "minimum": 0.01, "maximum": js::json_number(limits::BID) },
            },
            "required": ["text", "match_type"],
        },
        "minItems": 1,
        "maxItems": limits::KEYWORDS,
        "description": what,
    })
}

fn id_list_property(max: usize, description: &str) -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "minItems": 1, "maxItems": max, "description": description })
}

fn text_list_property(min: usize, max: usize, length: usize, description: &str) -> Value {
    json!({
        "type": "array",
        "items": { "type": "string", "maxLength": length },
        "minItems": min,
        "maxItems": max,
        "description": description,
    })
}

fn status_word(status: &str) -> &str {
    match status {
        "ENABLED" => "Enabled",
        "PAUSED" => "Paused",
        "REMOVED" => "Removed",
        other => other,
    }
}

fn match_word(match_type: &str) -> Option<&'static str> {
    Some(match match_type {
        "EXACT" => "exact match",
        "PHRASE" => "phrase match",
        "BROAD" => "broad match",
        _ => return None,
    })
}

fn row(label: impl Into<String>, value: impl Into<String>) -> ApprovalDetail {
    ApprovalDetail::new(label, value)
}

/// A row that says what a value replaces, when it replaces another one.
fn change(label: &str, value: impl Into<String>, before: impl Into<String>) -> ApprovalDetail {
    let (value, before) = (value.into(), before.into());
    if before == value {
        row(label, value)
    } else {
        row(label, value).replacing(before)
    }
}

fn account_row(account: &AccountFacts) -> ApprovalDetail {
    row("Account", account.label.clone())
}

fn campaign_row(name: &str, status: &str) -> ApprovalDetail {
    row(
        "Campaign",
        format!("{name} ({})", status_word(status).to_lowercase()),
    )
}

fn ad_group_rows(ad_group: &AdGroupFacts) -> Vec<ApprovalDetail> {
    vec![
        account_row(&ad_group.account),
        campaign_row(&ad_group.campaign.name, &ad_group.campaign.status),
        row(
            "Ad group",
            format!(
                "{} ({})",
                ad_group.name,
                status_word(&ad_group.status).to_lowercase()
            ),
        ),
    ]
}

/// Each item on a row of its own, up to a number a person still reads.
fn listed(label: &str, values: &[String]) -> Vec<ApprovalDetail> {
    let mut rows: Vec<ApprovalDetail> = values
        .iter()
        .take(MAX_LISTED)
        .enumerate()
        .map(|(index, value)| {
            if values.len() == 1 {
                row(label, value.clone())
            } else {
                row(format!("{label} {}", index + 1), value.clone())
            }
        })
        .collect();
    if values.len() > MAX_LISTED {
        rows.push(row(
            "More",
            format!(
                "{} more, listed in the exact arguments below",
                values.len() - MAX_LISTED
            ),
        ));
    }
    rows
}

fn daily(amount: f64, currency: &str) -> String {
    format!("{} a day", money(amount, currency))
}

fn monthly_row(amount: f64, currency: &str) -> ApprovalDetail {
    row(
        "Most it can cost in a month",
        money(amount * DAYS_IN_A_MONTH, currency),
    )
}

fn keyword_label(keyword: &GoogleAdsKeyword, currency: &str) -> String {
    let mut parts = vec![
        format!("\"{}\"", keyword.text),
        match_word(keyword.match_type.as_str())
            .unwrap_or_default()
            .to_owned(),
    ];
    if let Some(max_cpc) = keyword.max_cpc {
        parts.push(format!("at most {} a click", money(max_cpc, currency)));
    }
    parts.retain(|part| !part.is_empty());
    parts.join(" · ")
}

fn day(date: &str, end_of_day: bool) -> String {
    format!(
        "{date} {}",
        if end_of_day { "23:59:59" } else { "00:00:00" }
    )
}

/// A number as a template string writes it.
fn written(number: f64) -> String {
    vine::js::number_to_string(number)
}

/// The one target each bidding strategy takes, as the agent passed them or
/// as the campaign has them.
#[derive(Debug, Clone, Copy, Default)]
struct BiddingTargets {
    max_cpc: Option<f64>,
    target_cpa: Option<f64>,
    target_roas: Option<f64>,
}

impl BiddingTargets {
    fn of(&self, name: &str) -> Option<f64> {
        match name {
            "max_cpc" => self.max_cpc,
            "target_cpa" => self.target_cpa,
            "target_roas" => self.target_roas,
            _ => None,
        }
    }

    fn any(&self) -> bool {
        self.max_cpc.is_some() || self.target_cpa.is_some() || self.target_roas.is_some()
    }
}

/// A way to bid that these tools set. (`GOOGLE_ADS_BIDDING_STRATEGIES`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum BiddingStrategy {
    MaximizeClicks,
    MaximizeConversions,
    MaximizeConversionValue,
    ManualCpc,
}

impl BiddingStrategy {
    fn named(name: &str) -> Option<Self> {
        Some(match name {
            "MAXIMIZE_CLICKS" => Self::MaximizeClicks,
            "MAXIMIZE_CONVERSIONS" => Self::MaximizeConversions,
            "MAXIMIZE_CONVERSION_VALUE" => Self::MaximizeConversionValue,
            "MANUAL_CPC" => Self::ManualCpc,
            _ => return None,
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::MaximizeClicks => "MAXIMIZE_CLICKS",
            Self::MaximizeConversions => "MAXIMIZE_CONVERSIONS",
            Self::MaximizeConversionValue => "MAXIMIZE_CONVERSION_VALUE",
            Self::ManualCpc => "MANUAL_CPC",
        }
    }

    /// The target the strategy takes.
    fn own(self) -> Option<&'static str> {
        match self {
            Self::MaximizeClicks => Some("max_cpc"),
            Self::MaximizeConversions => Some("target_cpa"),
            Self::MaximizeConversionValue => Some("target_roas"),
            Self::ManualCpc => None,
        }
    }
}

fn bidding_word(strategy: &str) -> &str {
    match strategy {
        "MAXIMIZE_CLICKS" | "TARGET_SPEND" => "Maximize clicks",
        "MAXIMIZE_CONVERSIONS" => "Maximize conversions",
        "MAXIMIZE_CONVERSION_VALUE" => "Maximize conversion value",
        "MANUAL_CPC" => "Manual cost per click",
        other => other,
    }
}

fn bidding_label(strategy: &str, targets: BiddingTargets, currency: &str) -> String {
    let target = if let Some(max_cpc) = targets.max_cpc {
        Some(format!("at most {} a click", money(max_cpc, currency)))
    } else if let Some(target_cpa) = targets.target_cpa {
        Some(format!(
            "aiming at {} a conversion",
            money(target_cpa, currency)
        ))
    } else {
        targets.target_roas.map(|target_roas| {
            format!(
                "aiming at a return of {}%",
                written(js::math_round(target_roas * 100.0))
            )
        })
    };
    [Some(bidding_word(strategy).to_owned()), target]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// How a campaign bids, as the fields Google Ads takes and the path that
/// names them in an update.
struct Bidding {
    fields: Object,
    mask: &'static str,
}

/// How a campaign bids, as the fields Google Ads takes and the paths that
/// name them in an update. Each strategy has one target, and takes no other.
fn bidding_of(strategy: BiddingStrategy, targets: BiddingTargets) -> BuiltinResult<Bidding> {
    let own = strategy.own();
    let foreign = ["max_cpc", "target_cpa", "target_roas"]
        .into_iter()
        .find(|name| Some(*name) != own && targets.of(name).is_some());
    if let Some(foreign) = foreign {
        return Err(BuiltinError::tool(
            if strategy == BiddingStrategy::ManualCpc && foreign == "max_cpc" {
                "With MANUAL_CPC, bids are set on ad groups and keywords: leave max_cpc out here and set it with create_ad_group or add_keywords".to_owned()
            } else {
                format!(
                    "{foreign} does not go with {}{}",
                    strategy.as_str(),
                    own.map(|own| format!(", which takes {own}"))
                        .unwrap_or_default()
                )
            },
        ));
    }

    Ok(match strategy {
        BiddingStrategy::MaximizeClicks => Bidding {
            fields: Object::new().set(
                "targetSpend",
                match targets.max_cpc {
                    Some(max_cpc) => Object::new().text("cpcBidCeilingMicros", to_micros(max_cpc)),
                    None => Object::new(),
                }
                .into_value(),
            ),
            mask: "target_spend.cpc_bid_ceiling_micros",
        },
        BiddingStrategy::MaximizeConversions => Bidding {
            fields: Object::new().set(
                "maximizeConversions",
                match targets.target_cpa {
                    Some(target_cpa) => {
                        Object::new().text("targetCpaMicros", to_micros(target_cpa))
                    }
                    None => Object::new(),
                }
                .into_value(),
            ),
            mask: "maximize_conversions.target_cpa_micros",
        },
        BiddingStrategy::MaximizeConversionValue => Bidding {
            fields: Object::new().set(
                "maximizeConversionValue",
                match targets.target_roas {
                    Some(target_roas) => Object::new().number("targetRoas", target_roas),
                    None => Object::new(),
                }
                .into_value(),
            ),
            mask: "maximize_conversion_value.target_roas",
        },
        BiddingStrategy::ManualCpc => Bidding {
            fields: Object::new().set("manualCpc", json!({ "enhancedCpcEnabled": false })),
            mask: "manual_cpc.enhanced_cpc_enabled",
        },
    })
}

fn no_languages_for_search() -> BuiltinError {
    BuiltinError::tool(
        "Google no longer takes languages for Search campaigns: their ads follow the language of their own text and landing page. Leave the languages out.",
    )
}

fn removed(what: &str, name: &str, rows: Vec<ApprovalDetail>) -> ApprovalSummary {
    ApprovalSummary {
        title: format!("Remove the {what} \"{name}\" for good"),
        details: rows,
        warnings: Some(vec![format!(
            "A removed {what} cannot be restored, and its statistics stop growing."
        )]),
    }
}

/// What an ad or a campaign uses an image as: a shape, and the smallest size
/// Google Ads takes it in for that use.
#[derive(Debug, Clone, Copy)]
struct ImageUse {
    shape: &'static str,
    min_width: u32,
    min_height: u32,
}

const LANDSCAPE: ImageUse = ImageUse {
    shape: "landscape",
    min_width: 600,
    min_height: 314,
};
const SQUARE: ImageUse = ImageUse {
    shape: "square",
    min_width: 300,
    min_height: 300,
};
const SQUARE_LOGO: ImageUse = ImageUse {
    shape: "square",
    min_width: SQUARE_LOGO_MIN_PIXELS,
    min_height: SQUARE_LOGO_MIN_PIXELS,
};
const WIDE_LOGO: ImageUse = ImageUse {
    shape: "wide_logo",
    min_width: 512,
    min_height: 128,
};

fn shape_name(asset: &ImageAssetFacts) -> Option<&'static str> {
    image_shape(asset.width, asset.height).map(|shape| shape.name)
}

/// The image Google Ads would take for `usage`, or the sentence that says why it does not.
fn require_image(asset: &ImageAssetFacts, argument: &str, usage: ImageUse) -> BuiltinResult<()> {
    let fits = shape_name(asset) == Some(usage.shape)
        && asset.width >= f64::from(usage.min_width)
        && asset.height >= f64::from(usage.min_height);
    if fits {
        return Ok(());
    }
    let label = IMAGE_SHAPES
        .iter()
        .find(|shape| shape.name == usage.shape)
        .map_or(usage.shape, |wanted| wanted.label);
    Err(BuiltinError::tool(format!(
        "{argument} takes {label} images of at least {}×{} pixels, and the image asset {} is {}×{}",
        usage.min_width,
        usage.min_height,
        asset.id,
        written(asset.width),
        written(asset.height),
    )))
}

fn image_label(asset: &ImageAssetFacts) -> String {
    let name = if asset.name.is_empty() {
        format!("Asset {}", asset.id)
    } else {
        asset.name.clone()
    };
    format!(
        "{name} ({}×{})",
        written(asset.width),
        written(asset.height)
    )
}

/// `resourceId(created[index])`: the ID of what an operation made, or `null`.
fn created_id(created: &Created, index: usize) -> Value {
    resource_id(created.get(index).and_then(Option::as_deref)).map_or(Value::Null, Value::from)
}

#[derive(Debug, Deserialize)]
struct ImageUploadReference {
    upload: String,
    filename: String,
    content_type: Option<String>,
}

/// The link an agent sends an image to. It outlives the call that made it, so
/// what it refers to is checked again when the file arrives. (`imageUpload`)
pub async fn image_upload(
    reference: Value,
    _context: Arc<BuiltinToolContext>,
) -> BuiltinResult<BuiltinUploadTarget> {
    let ImageUploadReference {
        upload,
        filename,
        content_type,
    } = tool_input(&IMAGE_UPLOAD_REFERENCE_VALIDATOR, &reference)?;
    Ok(BuiltinUploadTarget {
        id: upload,
        filename,
        content_type,
        max_bytes: limits::IMAGE_BYTES,
    })
}

#[derive(Debug, Deserialize)]
struct CreateCampaign {
    customer_id: String,
    name: String,
    channel: String,
    daily_budget: f64,
    bidding_strategy: BiddingStrategy,
    max_cpc: Option<f64>,
    target_cpa: Option<f64>,
    target_roas: Option<f64>,
    location_ids: Option<Vec<String>>,
    languages: Option<Vec<String>>,
    start_date: Option<String>,
    end_date: Option<String>,
    status: Option<String>,
    search_partners: Option<bool>,
    display_network: Option<bool>,
    eu_political_ads: Option<bool>,
}

async fn create_campaign(
    input: CreateCampaign,
    context: Arc<BuiltinToolContext>,
) -> BuiltinResult<Plan> {
    let customer = customer_of(&context, &input.customer_id)?;
    let is_search = input.channel == "SEARCH";
    if !is_search && (input.search_partners.is_some() || input.display_network.is_some()) {
        return Err(BuiltinError::tool(
            "search_partners and display_network are for Search campaigns only",
        ));
    }
    if is_search && input.languages.is_some() {
        return Err(no_languages_for_search());
    }
    if let (Some(start_date), Some(end_date)) = (&input.start_date, &input.end_date)
        && start_date > end_date
    {
        return Err(BuiltinError::tool("start_date must not be after end_date"));
    }

    let status = input.status.unwrap_or_else(|| "PAUSED".to_owned());
    let targets = BiddingTargets {
        max_cpc: input.max_cpc,
        target_cpa: input.target_cpa,
        target_roas: input.target_roas,
    };
    let bidding = bidding_of(input.bidding_strategy, targets)?;
    let account = account_facts(&context, &customer).await?;
    let locations = locations_by_id(
        &context,
        &customer,
        input.location_ids.as_deref().unwrap_or_default(),
    )
    .await?;
    let languages = languages_by_code(
        &context,
        &customer,
        input.languages.as_deref().unwrap_or_default(),
    )
    .await?;
    let search_partners = input.search_partners.unwrap_or(false);
    let display_network = input.display_network.unwrap_or(false);
    let eu_political_ads = input.eu_political_ads.unwrap_or(false);

    // Negative IDs stand for what this very request creates. No two may be alike.
    let budget = format!("customers/{customer}/campaignBudgets/-1");
    let campaign = format!("customers/{customer}/campaigns/-2");
    let mut created = Object::new()
        .text("resourceName", campaign.clone())
        .text("name", input.name.clone())
        // Left out, Google enables the campaign.
        .text("status", status.clone())
        .text("advertisingChannelType", input.channel.clone())
        .text("campaignBudget", budget.clone());
    // A Display campaign shows on the Display Network and nowhere
    // else: Google sets its networks itself.
    if is_search {
        created = created.set(
            "networkSettings",
            json!({
                "targetGoogleSearch": true,
                "targetSearchNetwork": search_partners,
                "targetContentNetwork": display_network,
                "targetPartnerSearchNetwork": false,
            }),
        );
    }
    created = created.spread(bidding.fields);
    if let Some(start_date) = &input.start_date {
        created = created.text("startDateTime", day(start_date, false));
    }
    if let Some(end_date) = &input.end_date {
        created = created.text("endDateTime", day(end_date, true));
    }
    // Google refuses a campaign that does not say.
    created = created.text(
        "containsEuPoliticalAdvertising",
        if eu_political_ads {
            "CONTAINS_EU_POLITICAL_ADVERTISING"
        } else {
            "DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING"
        },
    );

    let mut operations = vec![
        json!({
            "campaignBudgetOperation": {
                "create": {
                    "resourceName": budget,
                    "amountMicros": to_micros(input.daily_budget),
                    "deliveryMethod": "STANDARD",
                    // Left out, Google makes it a budget other campaigns can share.
                    "explicitlyShared": false,
                },
            },
        }),
        json!({ "campaignOperation": { "create": created.into_value() } }),
    ];
    operations.extend(locations.iter().map(|location| {
        json!({
            "campaignCriterionOperation": {
                "create": {
                    "campaign": campaign,
                    "location": { "geoTargetConstant": format!("geoTargetConstants/{}", location.id) },
                },
            },
        })
    }));
    operations.extend(languages.iter().map(|language| {
        json!({
            "campaignCriterionOperation": {
                "create": {
                    "campaign": campaign,
                    "language": { "languageConstant": format!("languageConstants/{}", language.id) },
                },
            },
        })
    }));

    let currency = account.currency.clone();
    let kind = if is_search { "Search" } else { "Display" };
    let budgeted = daily(input.daily_budget, &currency);
    let or = |joined: String, fallback: &str| {
        if joined.is_empty() {
            fallback.to_owned()
        } else {
            joined
        }
    };
    let location_names: Vec<&str> = locations
        .iter()
        .map(|location| location.name.as_str())
        .collect();
    let language_names: Vec<&str> = languages
        .iter()
        .map(|language| language.name.as_str())
        .collect();

    let mut details = vec![
        account_row(&account),
        row("Campaign name", input.name.clone()),
        row("Type", kind),
        row("Daily budget", budgeted.clone()),
        monthly_row(input.daily_budget, &currency),
        row(
            "Bidding",
            bidding_label(input.bidding_strategy.as_str(), targets, &currency),
        ),
        row("Locations", or(location_names.join("; "), "Every country")),
    ];
    if is_search {
        let mut networks = vec!["Google Search"];
        if search_partners {
            networks.push("search partners");
        }
        if display_network {
            networks.push("Display Network");
        }
        details.push(row("Networks", networks.join(", ")));
    } else {
        details.push(row(
            "Languages",
            or(language_names.join(", "), "Every language"),
        ));
    }
    details.push(row(
        "Runs",
        format!(
            "{} to {}",
            input.start_date.as_deref().unwrap_or("From today"),
            input.end_date.as_deref().unwrap_or("no end date"),
        ),
    ));
    details.push(row(
        "Status",
        if status == "ENABLED" {
            "Enabled: it spends as soon as it has approved ads"
        } else {
            "Paused: it spends nothing until it is enabled"
        },
    ));
    if eu_political_ads {
        details.push(row(
            "EU political advertising",
            "Declared: Google does not serve it in the EU",
        ));
    }

    let mut warnings = Vec::new();
    if status == "ENABLED" {
        warnings.push(format!(
            "The campaign is created enabled: it can spend {budgeted} without another approval."
        ));
    }
    if locations.is_empty() {
        warnings.push("No location is set: the ads can show in every country.".to_owned());
    }

    let (name, daily_budget) = (input.name, input.daily_budget);
    Ok(Plan {
        customer,
        operations,
        summary: ApprovalSummary {
            title: format!("Create the {kind} campaign \"{name}\" with a budget of {budgeted}"),
            details,
            warnings: Some(warnings),
        },
        result: Box::new(move |created| {
            let next = if status == "PAUSED" {
                "The campaign is paused. Add an ad group, ads, and keywords, then enable it with set_campaign_status."
            } else {
                "The campaign is enabled. It shows nothing until it has an ad group with approved ads."
            };
            Object::new()
                .set("campaign_id", created_id(created, 1))
                .text("name", name)
                .text("status", status)
                .number("daily_budget", daily_budget)
                .text("currency", currency)
                .text("next", next)
                .into_value()
        }),
    })
}

fn create_campaign_tool() -> Tool {
    write_tool(
        "create_campaign",
        format!(
            "Create a Search or Display campaign with its own daily budget, bidding strategy, and targeting. It is created paused unless status says otherwise, and needs an ad group, ads, and for Search keywords before it can show anything: add them with create_ad_group, add_keywords, and create_responsive_search_ad or create_responsive_display_ad, then enable it with set_campaign_status. {MONEY}"
        ),
        input_schema(
            properties([
                customer_id_property(),
                json!({
                    "name": {
                        "type": "string",
                        "maxLength": limits::NAME_LENGTH,
                        "description": "Campaign name. No other campaign of the account may have it.",
                    },
                    "channel": {
                        "type": "string",
                        "enum": GOOGLE_ADS_CHANNELS,
                        "description": "SEARCH shows text ads on Google search results. DISPLAY shows image ads on websites and apps.",
                    },
                    "daily_budget": {
                        "type": "number",
                        "minimum": 0.01,
                        "maximum": js::json_number(limits::DAILY_BUDGET),
                        "description": "Average amount to spend a day, such as 25 or 12.5. Google may spend up to twice that on a day, and never more than 30.4 times it in a month.",
                    },
                    "bidding_strategy": {
                        "type": "string",
                        "enum": GOOGLE_ADS_BIDDING_STRATEGIES,
                        "description": "MAXIMIZE_CLICKS needs no conversion tracking. MAXIMIZE_CONVERSIONS and MAXIMIZE_CONVERSION_VALUE need conversions to be tracked. MANUAL_CPC bids what the ad groups and keywords say.",
                    },
                }),
                bidding_properties(),
                json!({
                    "location_ids": id_list_property(
                        limits::LOCATIONS,
                        "Locations to show the ads in, by the IDs search_locations returns. Left out, the campaign targets every country.",
                    ),
                    "languages": id_list_property(
                        limits::LANGUAGES,
                        "Display campaigns only: languages of the people to reach, as codes such as en, fr, or pt_BR. Left out, every language.",
                    ),
                    "start_date": {
                        "type": "string",
                        "description": "First day the campaign may run, such as 2026-01-31, in the account's time zone.",
                    },
                    "end_date": {
                        "type": "string",
                        "description": "Last day the campaign runs. Left out, it has no end.",
                    },
                    "status": {
                        "type": "string",
                        "enum": ["PAUSED", "ENABLED"],
                        "default": "PAUSED",
                        "description": "ENABLED lets the campaign spend as soon as it has approved ads.",
                    },
                    "search_partners": {
                        "type": "boolean",
                        "default": false,
                        "description": "Search campaigns only: also show the ads on the sites of Google search partners.",
                    },
                    "display_network": {
                        "type": "boolean",
                        "default": false,
                        "description": "Search campaigns only: also show the ads on the Google Display Network.",
                    },
                    "eu_political_ads": {
                        "type": "boolean",
                        "default": false,
                        "description": "Declares that the campaign carries political advertising aimed at the European Union, which Google then does not serve there.",
                    },
                }),
            ]),
            &[
                "customer_id",
                "name",
                "channel",
                "daily_budget",
                "bidding_strategy",
            ],
        ),
        &CREATE_CAMPAIGN_VALIDATOR,
        create_campaign,
    )
    .asks_approval()
}

#[derive(Debug, Deserialize)]
struct UpdateCampaign {
    customer_id: String,
    campaign_id: String,
    name: Option<String>,
    bidding_strategy: Option<BiddingStrategy>,
    max_cpc: Option<f64>,
    target_cpa: Option<f64>,
    target_roas: Option<f64>,
    start_date: Option<String>,
    end_date: Option<String>,
    search_partners: Option<bool>,
    display_network: Option<bool>,
}

async fn update_campaign(
    input: UpdateCampaign,
    context: Arc<BuiltinToolContext>,
) -> BuiltinResult<Plan> {
    let customer = customer_of(&context, &input.customer_id)?;
    let campaign = campaign_facts(&context, &customer, &input.campaign_id).await?;
    let currency = &campaign.account.currency;
    let mut fields = Object::new();
    let mut mask: Vec<&'static str> = Vec::new();
    let mut rows: Vec<ApprovalDetail> = Vec::new();

    if let Some(name) = &input.name {
        fields = fields.text("name", name.clone());
        mask.push("name");
        rows.push(change("Name", name.clone(), campaign.name.clone()));
    }

    let targets = BiddingTargets {
        max_cpc: input.max_cpc,
        target_cpa: input.target_cpa,
        target_roas: input.target_roas,
    };
    let changes_bidding = input.bidding_strategy.is_some() || targets.any();
    if changes_bidding {
        let current = if campaign.bidding_strategy == "TARGET_SPEND" {
            "MAXIMIZE_CLICKS"
        } else {
            &campaign.bidding_strategy
        };
        let Some(strategy) = input
            .bidding_strategy
            .or_else(|| BiddingStrategy::named(current))
        else {
            return Err(BuiltinError::tool(format!(
                "This campaign bids with {}, which has no target to change here. Pass bidding_strategy to move it to one of: {}",
                campaign.bidding_strategy,
                GOOGLE_ADS_BIDDING_STRATEGIES.join(", ")
            )));
        };
        let bidding = bidding_of(strategy, targets)?;
        fields = fields.spread(bidding.fields);
        mask.push(bidding.mask);
        rows.push(change(
            "Bidding",
            bidding_label(strategy.as_str(), targets, currency),
            bidding_label(
                &campaign.bidding_strategy,
                BiddingTargets {
                    max_cpc: campaign.max_cpc,
                    target_cpa: campaign.target_cpa,
                    target_roas: campaign.target_roas,
                },
                currency,
            ),
        ));
    }

    let start_date = input.start_date.as_ref().or(campaign.start_date.as_ref());
    let end_date = input.end_date.as_ref().or(campaign.end_date.as_ref());
    if let (Some(start_date), Some(end_date)) = (start_date, end_date)
        && start_date > end_date
    {
        return Err(BuiltinError::tool(
            "The start date must not be after the end date",
        ));
    }
    if let Some(start_date) = &input.start_date {
        fields = fields.text("startDateTime", day(start_date, false));
        mask.push("start_date_time");
        rows.push(change(
            "First day",
            start_date.clone(),
            campaign.start_date.as_deref().unwrap_or("Not set"),
        ));
    }
    if let Some(end_date) = &input.end_date {
        fields = fields.text("endDateTime", day(end_date, true));
        mask.push("end_date_time");
        rows.push(change(
            "Last day",
            end_date.clone(),
            campaign.end_date.as_deref().unwrap_or("No end date"),
        ));
    }

    let mut networks = Object::new();
    let shown = |is_shown: bool| if is_shown { "Shown" } else { "Not shown" };
    if let Some(search_partners) = input.search_partners {
        networks = networks.flag("targetSearchNetwork", search_partners);
        mask.push("network_settings.target_search_network");
        rows.push(change(
            "Search partners",
            shown(search_partners),
            shown(campaign.search_partners),
        ));
    }
    if let Some(display_network) = input.display_network {
        networks = networks.flag("targetContentNetwork", display_network);
        mask.push("network_settings.target_content_network");
        rows.push(change(
            "Display Network",
            shown(display_network),
            shown(campaign.display_network),
        ));
    }
    if input.search_partners.is_some() || input.display_network.is_some() {
        if campaign.channel != "SEARCH" {
            return Err(BuiltinError::tool(
                "search_partners and display_network are for Search campaigns only",
            ));
        }
        fields = fields.set("networkSettings", networks.into_value());
    }

    if mask.is_empty() {
        return Err(BuiltinError::tool("Pass at least one setting to change"));
    }

    let mut details = vec![
        account_row(&campaign.account),
        campaign_row(&campaign.name, &campaign.status),
    ];
    details.extend(rows);
    let update = Object::new()
        .text("resourceName", campaign.resource_name.clone())
        .spread(fields);
    let campaign_id = campaign.id.clone();
    Ok(Plan {
        customer,
        operations: vec![json!({
            "campaignOperation": { "update": update.into_value(), "updateMask": mask.join(",") },
        })],
        summary: ApprovalSummary {
            title: format!("Change the settings of the campaign \"{}\"", campaign.name),
            details,
            warnings: Some(if changes_bidding && campaign.status == "ENABLED" {
                vec![
                    "The campaign is live: the new bidding applies at once, and Google relearns for a few days."
                        .to_owned(),
                ]
            } else {
                Vec::new()
            }),
        },
        result: Box::new(move |_| {
            Object::new()
                .text("campaign_id", campaign_id)
                .set("changed", json!(mask))
                .into_value()
        }),
    })
}

fn update_campaign_tool() -> Tool {
    write_tool(
        "update_campaign",
        format!(
            "Change the settings of a campaign: its name, its bidding strategy and target, its start and end dates, and for a Search campaign the extra networks it shows on. Only what you pass is changed. Its budget is changed with update_campaign_budget, its status with set_campaign_status, and its targeting with update_campaign_targeting. {MONEY}"
        ),
        input_schema(
            properties([
                customer_id_property(),
                campaign_id_property(),
                json!({
                    "name": { "type": "string", "maxLength": limits::NAME_LENGTH, "description": "New name." },
                    "bidding_strategy": {
                        "type": "string",
                        "enum": GOOGLE_ADS_BIDDING_STRATEGIES,
                        "description": "New bidding strategy. Left out, a target passed below applies to the current one.",
                    },
                }),
                bidding_properties(),
                json!({
                    "start_date": { "type": "string", "description": "New first day, such as 2026-01-31." },
                    "end_date": { "type": "string", "description": "New last day, such as 2026-03-31." },
                    "search_partners": {
                        "type": "boolean",
                        "description": "Search campaigns only: show the ads on the sites of Google search partners.",
                    },
                    "display_network": {
                        "type": "boolean",
                        "description": "Search campaigns only: show the ads on the Google Display Network.",
                    },
                }),
            ]),
            &["customer_id", "campaign_id"],
        ),
        &UPDATE_CAMPAIGN_VALIDATOR,
        update_campaign,
    )
    .asks_approval()
}

#[derive(Debug, Deserialize)]
struct SetCampaignStatus {
    customer_id: String,
    campaign_id: String,
    status: String,
}

async fn set_campaign_status(
    input: SetCampaignStatus,
    context: Arc<BuiltinToolContext>,
) -> BuiltinResult<Plan> {
    let customer = customer_of(&context, &input.customer_id)?;
    let campaign = campaign_facts(&context, &customer, &input.campaign_id).await?;
    let currency = &campaign.account.currency;
    let budget = daily(campaign.budget.amount, currency);
    let status = input.status;
    let mut rows = vec![
        account_row(&campaign.account),
        row("Campaign", campaign.name.clone()),
        change(
            "Status",
            status_word(&status),
            status_word(&campaign.status),
        ),
    ];

    let summary = match status.as_str() {
        "ENABLED" => {
            rows.push(row("Daily budget", budget.clone()));
            rows.push(monthly_row(campaign.budget.amount, currency));
            ApprovalSummary {
                title: format!(
                    "Enable the campaign \"{}\", which can spend {budget}",
                    campaign.name
                ),
                details: rows,
                warnings: Some(vec![format!(
                    "Once enabled, the campaign spends up to {budget} without another approval."
                )]),
            }
        }
        "PAUSED" => ApprovalSummary {
            title: format!("Pause the campaign \"{}\"", campaign.name),
            details: rows,
            warnings: Some(vec![
                "Its ads stop showing until it is enabled again.".to_owned(),
            ]),
        },
        _ => removed("campaign", &campaign.name, rows),
    };
    let operation = if status == "REMOVED" {
        json!({ "remove": campaign.resource_name })
    } else {
        json!({
            "update": { "resourceName": campaign.resource_name, "status": status },
            "updateMask": "status",
        })
    };
    let (campaign_id, name) = (campaign.id, campaign.name);
    Ok(Plan {
        customer,
        operations: vec![json!({ "campaignOperation": operation })],
        summary,
        result: Box::new(move |_| {
            Object::new()
                .text("campaign_id", campaign_id)
                .text("name", name)
                .text("status", status)
                .into_value()
        }),
    })
}

fn set_campaign_status_tool() -> Tool {
    write_tool(
        "set_campaign_status",
        "Enable, pause, or remove a campaign. An enabled campaign spends its daily budget as soon as it has approved ads. A removed campaign cannot be restored.",
        input_schema(
            properties([
                customer_id_property(),
                campaign_id_property(),
                json!({ "status": { "type": "string", "enum": GOOGLE_ADS_STATUSES } }),
            ]),
            &["customer_id", "campaign_id", "status"],
        ),
        &SET_CAMPAIGN_STATUS_VALIDATOR,
        set_campaign_status,
    )
    .asks_approval()
}

#[derive(Debug, Deserialize)]
struct UpdateCampaignBudget {
    customer_id: String,
    campaign_id: String,
    daily_budget: f64,
}

async fn update_campaign_budget(
    input: UpdateCampaignBudget,
    context: Arc<BuiltinToolContext>,
) -> BuiltinResult<Plan> {
    let amount = input.daily_budget;
    let customer = customer_of(&context, &input.customer_id)?;
    let campaign = campaign_facts(&context, &customer, &input.campaign_id).await?;
    let currency = campaign.account.currency.clone();
    let before = campaign.budget.amount;
    let times = if before > 0.0 { amount / before } else { 0.0 };

    let monthly = monthly_row(amount, &currency);
    let mut warnings = Vec::new();
    if times >= 2.0 {
        warnings.push(format!(
            "The new budget is {} times the current one.",
            written(vine::js::string_to_number(&to_fixed(times, 1)))
        ));
    }
    if campaign.budget.campaigns > 1.0 {
        warnings.push(format!(
            "This budget is shared by {} campaigns: the change applies to all of them.",
            written(campaign.budget.campaigns)
        ));
    }
    if campaign.status == "ENABLED" {
        warnings.push("The campaign is live: the new budget applies at once.".to_owned());
    }

    Ok(Plan {
        customer,
        operations: vec![json!({
            "campaignBudgetOperation": {
                "update": {
                    "resourceName": campaign.budget.resource_name,
                    "amountMicros": to_micros(amount),
                },
                "updateMask": "amount_micros",
            },
        })],
        summary: ApprovalSummary {
            title: format!(
                "Change the daily budget of the campaign \"{}\" from {} to {}",
                campaign.name,
                money(before, &currency),
                money(amount, &currency)
            ),
            details: vec![
                account_row(&campaign.account),
                campaign_row(&campaign.name, &campaign.status),
                change(
                    "Daily budget",
                    daily(amount, &currency),
                    daily(before, &currency),
                ),
                change(
                    &monthly.label,
                    monthly.value.clone(),
                    monthly_row(before, &currency).value,
                ),
            ],
            warnings: Some(warnings),
        },
        result: Box::new(move |_| {
            Object::new()
                .text("campaign_id", campaign.id)
                .text("name", campaign.name)
                .number("daily_budget", amount)
                .number("previous_daily_budget", before)
                .text("currency", currency)
                .into_value()
        }),
    })
}

fn update_campaign_budget_tool() -> Tool {
    write_tool(
        "update_campaign_budget",
        format!(
            "Set the average amount a campaign spends a day. Google may spend up to twice that on a day, and never more than 30.4 times it in a month. {MONEY}"
        ),
        input_schema(
            properties([
                customer_id_property(),
                campaign_id_property(),
                json!({
                    "daily_budget": {
                        "type": "number",
                        "minimum": 0.01,
                        "maximum": js::json_number(limits::DAILY_BUDGET),
                        "description": "New average amount to spend a day, such as 25 or 12.5.",
                    },
                }),
            ]),
            &["customer_id", "campaign_id", "daily_budget"],
        ),
        &UPDATE_CAMPAIGN_BUDGET_VALIDATOR,
        update_campaign_budget,
    )
    .asks_approval()
}

#[derive(Debug, Deserialize)]
struct UpdateCampaignTargeting {
    customer_id: String,
    campaign_id: String,
    add_location_ids: Option<Vec<String>>,
    remove_location_ids: Option<Vec<String>>,
    add_languages: Option<Vec<String>>,
    remove_languages: Option<Vec<String>>,
    add_negative_keywords: Option<Vec<GoogleAdsKeyword>>,
    remove_criterion_ids: Option<Vec<String>>,
}

/// `rows.map((row) => row.campaignCriterion!).find(matches)`. A row without
/// its criterion fails the search when it is reached, and not before.
fn find_criterion(
    rows: &[GoogleAdsRow],
    matches: impl Fn(&Value) -> bool,
) -> BuiltinResult<Option<&Value>> {
    for row in rows {
        let criterion = js::resource(row, "campaignCriterion")?;
        if matches(criterion) {
            return Ok(Some(criterion));
        }
    }
    Ok(None)
}

fn criterion_label(criterion: &Value) -> String {
    let kind_of = js::get(criterion, "type");
    let kind = js::string(kind_of).to_lowercase();
    let what = if kind_of.and_then(Value::as_str) == Some("KEYWORD") {
        "negative keyword".to_owned()
    } else if js::truthy(js::get(criterion, "negative")) {
        format!("excluded {kind}")
    } else {
        kind
    };
    let named = js::defined(js::get(js::get(criterion, "keyword"), "text"))
        .or_else(|| js::defined(js::get(criterion, "displayName")))
        .or_else(|| js::get(criterion, "criterionId"));
    format!("{} ({what})", js::string(named))
}

async fn update_campaign_targeting(
    input: UpdateCampaignTargeting,
    context: Arc<BuiltinToolContext>,
) -> BuiltinResult<Plan> {
    let customer = customer_of(&context, &input.customer_id)?;
    let campaign = campaign_facts(&context, &customer, &input.campaign_id).await?;
    if campaign.channel == "SEARCH" && input.add_languages.is_some() {
        return Err(no_languages_for_search());
    }
    let negatives = input.add_negative_keywords.unwrap_or_default();
    if negatives.iter().any(|keyword| keyword.max_cpc.is_some()) {
        return Err(BuiltinError::tool("A negative keyword takes no max_cpc"));
    }

    let added_locations = locations_by_id(
        &context,
        &customer,
        input.add_location_ids.as_deref().unwrap_or_default(),
    )
    .await?;
    let removed_locations = locations_by_id(
        &context,
        &customer,
        input.remove_location_ids.as_deref().unwrap_or_default(),
    )
    .await?;
    let added_languages = languages_by_code(
        &context,
        &customer,
        input.add_languages.as_deref().unwrap_or_default(),
    )
    .await?;
    let removed_languages = languages_by_code(
        &context,
        &customer,
        input.remove_languages.as_deref().unwrap_or_default(),
    )
    .await?;

    // What is removed is read from the campaign first: each criterion goes
    // by the name Google gives it, and only as what the agent said it is.
    let criterion_ids = input.remove_criterion_ids.unwrap_or_default();
    let criterion_fields = "campaign_criterion.resource_name, campaign_criterion.criterion_id, campaign_criterion.type, campaign_criterion.negative, campaign_criterion.display_name, campaign_criterion.keyword.text, campaign_criterion.location.geo_target_constant, campaign_criterion.language.language_constant";
    let targeted = if removed_locations.len() + removed_languages.len() > 0 {
        search_google_ads(
            &context,
            &customer,
            &format!(
                "SELECT {criterion_fields} FROM campaign_criterion WHERE campaign.id = {} AND campaign_criterion.type IN ('LOCATION', 'LANGUAGE')",
                campaign.id
            ),
            MAX_CAMPAIGN_TARGETS,
        )
        .await?
        .rows
    } else {
        Vec::new()
    };
    let named = if criterion_ids.is_empty() {
        Vec::new()
    } else {
        search_google_ads(
            &context,
            &customer,
            &format!(
                "SELECT {criterion_fields} FROM campaign_criterion WHERE campaign.id = {} AND campaign_criterion.criterion_id IN ({})",
                campaign.id,
                criterion_ids.join(", ")
            ),
            criterion_ids.len(),
        )
        .await?
        .rows
    };

    let not_targeted = |name: &str| {
        BuiltinError::tool(format!(
            "The campaign {} does not target {name}",
            campaign.id
        ))
    };
    let mut removals: Vec<&Value> = Vec::new();
    for location in &removed_locations {
        let constant = format!("geoTargetConstants/{}", location.id);
        let criterion = find_criterion(&targeted, |candidate| {
            js::get(js::get(candidate, "location"), "geoTargetConstant").and_then(Value::as_str)
                == Some(&constant)
        })?
        .ok_or_else(|| not_targeted(&location.name))?;
        // Removing an exclusion opens a place instead of closing one.
        if js::truthy(js::get(criterion, "negative")) {
            return Err(BuiltinError::tool(format!(
                "The campaign {} excludes {}, and removing that exclusion would let its ads show there. If that is what is wanted, pass its criterion ID {} in remove_criterion_ids.",
                campaign.id,
                location.name,
                js::string(js::get(criterion, "criterionId"))
            )));
        }
        removals.push(criterion);
    }
    for language in &removed_languages {
        let constant = format!("languageConstants/{}", language.id);
        let criterion = find_criterion(&targeted, |candidate| {
            js::get(js::get(candidate, "language"), "languageConstant").and_then(Value::as_str)
                == Some(&constant)
        })?
        .ok_or_else(|| not_targeted(&language.name))?;
        removals.push(criterion);
    }
    let mut other_criteria: Vec<&Value> = Vec::new();
    for id in &criterion_ids {
        let criterion = find_criterion(&named, |candidate| {
            js::string(js::get(candidate, "criterionId")) == *id
        })?
        .ok_or_else(|| {
            BuiltinError::tool(format!(
                "The campaign {} has no criterion {id}. get_campaign lists its criteria.",
                campaign.id
            ))
        })?;
        other_criteria.push(criterion);
    }
    removals.extend(&other_criteria);

    let create = |fields: Object| {
        let criterion = Object::new()
            .text("campaign", campaign.resource_name.clone())
            .spread(fields);
        json!({ "campaignCriterionOperation": { "create": criterion.into_value() } })
    };
    let mut operations: Vec<GoogleAdsOperation> = removals
        .iter()
        .map(|criterion| {
            let removal = Object::new().field("remove", js::get(*criterion, "resourceName"));
            json!({ "campaignCriterionOperation": removal.into_value() })
        })
        .collect();
    operations.extend(added_locations.iter().map(|location| {
        create(Object::new().set(
            "location",
            json!({ "geoTargetConstant": format!("geoTargetConstants/{}", location.id) }),
        ))
    }));
    operations.extend(added_languages.iter().map(|language| {
        create(Object::new().set(
            "language",
            json!({ "languageConstant": format!("languageConstants/{}", language.id) }),
        ))
    }));
    operations.extend(negatives.iter().map(|keyword| {
        create(Object::new().flag("negative", true).set(
            "keyword",
            json!({ "text": keyword.text, "matchType": keyword.match_type.as_str() }),
        ))
    }));
    if operations.is_empty() {
        return Err(BuiltinError::tool(
            "Pass at least one thing to add or remove",
        ));
    }

    let names = |names: Vec<&str>| names.join("; ");
    let mut details = vec![
        account_row(&campaign.account),
        campaign_row(&campaign.name, &campaign.status),
    ];
    if !added_locations.is_empty() {
        details.push(row(
            "Add locations",
            names(
                added_locations
                    .iter()
                    .map(|one| one.name.as_str())
                    .collect(),
            ),
        ));
    }
    if !removed_locations.is_empty() {
        details.push(row(
            "Remove locations",
            names(
                removed_locations
                    .iter()
                    .map(|one| one.name.as_str())
                    .collect(),
            ),
        ));
    }
    if !added_languages.is_empty() {
        details.push(row(
            "Add languages",
            names(
                added_languages
                    .iter()
                    .map(|one| one.name.as_str())
                    .collect(),
            ),
        ));
    }
    if !removed_languages.is_empty() {
        details.push(row(
            "Remove languages",
            names(
                removed_languages
                    .iter()
                    .map(|one| one.name.as_str())
                    .collect(),
            ),
        ));
    }
    let negative_labels: Vec<String> = negatives
        .iter()
        .map(|keyword| keyword_label(keyword, &campaign.account.currency))
        .collect();
    details.extend(listed("Add negative keyword", &negative_labels));
    let removed_labels: Vec<String> = other_criteria
        .iter()
        .map(|criterion| criterion_label(criterion))
        .collect();
    details.extend(listed("Remove", &removed_labels));

    let added = added_locations.len() + added_languages.len() + negatives.len();
    let taken_away = removed_locations.len() + removed_languages.len() + criterion_ids.len();
    let campaign_id = campaign.id.clone();
    Ok(Plan {
        customer,
        operations,
        summary: ApprovalSummary {
            title: format!("Change the targeting of the campaign \"{}\"", campaign.name),
            details,
            warnings: None,
        },
        result: Box::new(move |_| {
            Object::new()
                .text("campaign_id", campaign_id)
                .set("added", json!(added))
                .set("removed", json!(taken_away))
                .into_value()
        }),
    })
}

fn update_campaign_targeting_tool() -> Tool {
    write_tool(
        "update_campaign_targeting",
        "Change who a campaign reaches: add or remove locations, add or remove languages (Display campaigns only), add negative keywords that keep its ads away from searches, and remove any criterion by the ID get_campaign lists.",
        input_schema(
            properties([
                customer_id_property(),
                campaign_id_property(),
                json!({
                    "add_location_ids": id_list_property(
                        limits::LOCATIONS,
                        "Locations to add, by the IDs search_locations returns.",
                    ),
                    "remove_location_ids": id_list_property(
                        limits::LOCATIONS,
                        "Locations to stop targeting. An excluded location is not one: remove its exclusion with remove_criterion_ids.",
                    ),
                    "add_languages": id_list_property(
                        limits::LANGUAGES,
                        "Display campaigns only: languages to add, as codes such as en or fr.",
                    ),
                    "remove_languages": id_list_property(limits::LANGUAGES, "Languages to stop targeting."),
                    "add_negative_keywords": keyword_list_property(
                        "Searches the campaign must not show for. A negative keyword takes no max_cpc.",
                    ),
                    "remove_criterion_ids": id_list_property(
                        limits::CRITERIA,
                        "Criteria to remove, such as negative keywords, by the criterion IDs get_campaign returns.",
                    ),
                }),
            ]),
            &["customer_id", "campaign_id"],
        ),
        &UPDATE_CAMPAIGN_TARGETING_VALIDATOR,
        update_campaign_targeting,
    )
}

#[derive(Debug, Deserialize)]
struct CreateAdGroup {
    customer_id: String,
    campaign_id: String,
    name: String,
    max_cpc: Option<f64>,
    status: Option<String>,
}

async fn create_ad_group(
    input: CreateAdGroup,
    context: Arc<BuiltinToolContext>,
) -> BuiltinResult<Plan> {
    let customer = customer_of(&context, &input.customer_id)?;
    let campaign = campaign_facts(&context, &customer, &input.campaign_id).await?;
    let kind = match campaign.channel.as_str() {
        "SEARCH" => "SEARCH_STANDARD",
        "DISPLAY" => "DISPLAY_STANDARD",
        other => {
            return Err(BuiltinError::tool(format!(
                "This MCP creates ad groups in Search and Display campaigns, and the campaign {} is a {other} one",
                campaign.id
            )));
        }
    };

    let status = input.status.unwrap_or_else(|| "ENABLED".to_owned());
    let currency = &campaign.account.currency;
    let mut created = Object::new()
        .text("campaign", campaign.resource_name.clone())
        .text("name", input.name.clone())
        .text("status", status.clone())
        .text("type", kind);
    let mut details = vec![
        account_row(&campaign.account),
        campaign_row(&campaign.name, &campaign.status),
        row("Ad group name", input.name.clone()),
    ];
    if let Some(max_cpc) = input.max_cpc {
        created = created.text("cpcBidMicros", to_micros(max_cpc));
        details.push(row("Most a click may cost", money(max_cpc, currency)));
    }
    details.push(row("Status", status_word(&status)));

    let name = input.name;
    Ok(Plan {
        customer,
        operations: vec![json!({ "adGroupOperation": { "create": created.into_value() } })],
        summary: ApprovalSummary {
            title: format!(
                "Create the ad group \"{name}\" in the campaign \"{}\"",
                campaign.name
            ),
            details,
            warnings: None,
        },
        result: Box::new(move |created| {
            Object::new()
                .set("ad_group_id", created_id(created, 0))
                .text("name", name)
                .text("status", status)
                .into_value()
        }),
    })
}

fn create_ad_group_tool() -> Tool {
    write_tool(
        "create_ad_group",
        format!(
            "Create an ad group in a campaign, to hold ads and, in a Search campaign, the keywords they show for. {MONEY}"
        ),
        input_schema(
            properties([
                customer_id_property(),
                campaign_id_property(),
                json!({
                    "name": {
                        "type": "string",
                        "maxLength": limits::NAME_LENGTH,
                        "description": "Ad group name. No other ad group of the campaign may have it.",
                    },
                    "max_cpc": bid_property(
                        "The most a click may cost, for the keywords that set no bid of their own. Used by campaigns that bid with MANUAL_CPC.",
                    ),
                    "status": { "type": "string", "enum": ["ENABLED", "PAUSED"], "default": "ENABLED" },
                }),
            ]),
            &["customer_id", "campaign_id", "name"],
        ),
        &CREATE_AD_GROUP_VALIDATOR,
        create_ad_group,
    )
}

#[derive(Debug, Deserialize)]
struct UpdateAdGroup {
    customer_id: String,
    ad_group_id: String,
    name: Option<String>,
    max_cpc: Option<f64>,
    status: Option<String>,
}

async fn update_ad_group(
    input: UpdateAdGroup,
    context: Arc<BuiltinToolContext>,
) -> BuiltinResult<Plan> {
    let customer = customer_of(&context, &input.customer_id)?;
    let ad_group = ad_group_facts(&context, &customer, &input.ad_group_id).await?;
    let currency = &ad_group.account.currency;
    let mut details = vec![
        account_row(&ad_group.account),
        campaign_row(&ad_group.campaign.name, &ad_group.campaign.status),
        row("Ad group", ad_group.name.clone()),
    ];
    let ad_group_id = ad_group.id.clone();

    if input.status.as_deref() == Some("REMOVED") {
        if input.name.is_some() || input.max_cpc.is_some() {
            return Err(BuiltinError::tool(
                "An ad group that is removed takes no other change",
            ));
        }
        return Ok(Plan {
            customer,
            operations: vec![json!({ "adGroupOperation": { "remove": ad_group.resource_name } })],
            summary: removed("ad group", &ad_group.name, details),
            result: Box::new(move |_| {
                Object::new()
                    .text("ad_group_id", ad_group_id)
                    .text("status", "REMOVED")
                    .into_value()
            }),
        });
    }

    let mut fields = Object::new();
    let mut mask: Vec<&'static str> = Vec::new();
    if let Some(name) = &input.name {
        fields = fields.text("name", name.clone());
        mask.push("name");
        details.push(change("Name", name.clone(), ad_group.name.clone()));
    }
    if let Some(max_cpc) = input.max_cpc {
        fields = fields.text("cpcBidMicros", to_micros(max_cpc));
        mask.push("cpc_bid_micros");
        details.push(change(
            "Most a click may cost",
            money(max_cpc, currency),
            ad_group
                .max_cpc
                .map_or_else(|| "Not set".to_owned(), |before| money(before, currency)),
        ));
    }
    if let Some(status) = &input.status {
        fields = fields.text("status", status.clone());
        mask.push("status");
        details.push(change(
            "Status",
            status_word(status),
            status_word(&ad_group.status),
        ));
    }
    if mask.is_empty() {
        return Err(BuiltinError::tool("Pass at least one field to change"));
    }

    let update = Object::new()
        .text("resourceName", ad_group.resource_name.clone())
        .spread(fields);
    Ok(Plan {
        customer,
        operations: vec![json!({
            "adGroupOperation": { "update": update.into_value(), "updateMask": mask.join(",") },
        })],
        summary: ApprovalSummary {
            title: format!("Change the ad group \"{}\"", ad_group.name),
            details,
            warnings: None,
        },
        result: Box::new(move |_| {
            Object::new()
                .text("ad_group_id", ad_group_id)
                .set("changed", json!(mask))
                .into_value()
        }),
    })
}

fn update_ad_group_tool() -> Tool {
    write_tool(
        "update_ad_group",
        format!(
            "Change an ad group: its name, its default bid, or its status. Only what you pass is changed. A removed ad group cannot be restored. {MONEY}"
        ),
        input_schema(
            properties([
                customer_id_property(),
                ad_group_id_property(),
                json!({
                    "name": { "type": "string", "maxLength": limits::NAME_LENGTH, "description": "New name." },
                    "max_cpc": bid_property("New most a click may cost."),
                    "status": { "type": "string", "enum": GOOGLE_ADS_STATUSES },
                }),
            ]),
            &["customer_id", "ad_group_id"],
        ),
        &UPDATE_AD_GROUP_VALIDATOR,
        update_ad_group,
    )
}

#[derive(Debug, Deserialize)]
struct AddKeywords {
    customer_id: String,
    ad_group_id: String,
    keywords: Vec<GoogleAdsKeyword>,
    negative: Option<bool>,
}

async fn add_keywords(input: AddKeywords, context: Arc<BuiltinToolContext>) -> BuiltinResult<Plan> {
    let (keywords, negative) = (input.keywords, input.negative.unwrap_or(false));
    let customer = customer_of(&context, &input.customer_id)?;
    if negative && keywords.iter().any(|keyword| keyword.max_cpc.is_some()) {
        return Err(BuiltinError::tool("A negative keyword takes no max_cpc"));
    }
    let ad_group = ad_group_facts(&context, &customer, &input.ad_group_id).await?;
    let currency = &ad_group.account.currency;
    let what = format!(
        "{} {}{}",
        keywords.len(),
        if negative { "negative " } else { "" },
        if keywords.len() == 1 {
            "keyword"
        } else {
            "keywords"
        }
    );

    let operations = keywords
        .iter()
        .map(|keyword| {
            let mut created = Object::new()
                .text("adGroup", ad_group.resource_name.clone())
                .set(
                    "keyword",
                    json!({ "text": keyword.text, "matchType": keyword.match_type.as_str() }),
                );
            created = if negative {
                created.flag("negative", true)
            } else {
                created.text("status", "ENABLED")
            };
            if let Some(max_cpc) = keyword.max_cpc {
                created = created.text("cpcBidMicros", to_micros(max_cpc));
            }
            json!({ "adGroupCriterionOperation": { "create": created.into_value() } })
        })
        .collect();
    let labels: Vec<String> = keywords
        .iter()
        .map(|keyword| keyword_label(keyword, currency))
        .collect();
    let mut details = ad_group_rows(&ad_group);
    details.extend(listed(
        if negative {
            "Negative keyword"
        } else {
            "Keyword"
        },
        &labels,
    ));

    let ad_group_id = ad_group.id.clone();
    Ok(Plan {
        customer,
        operations,
        summary: ApprovalSummary {
            title: format!("Add {what} to the ad group \"{}\"", ad_group.name),
            details,
            warnings: None,
        },
        result: Box::new(move |created| {
            let keywords: Vec<Value> = keywords
                .iter()
                .enumerate()
                .map(|(index, keyword)| {
                    Object::new()
                        .set("criterion_id", created_id(created, index))
                        .text("text", keyword.text.clone())
                        .text("match_type", keyword.match_type.as_str())
                        .into_value()
                })
                .collect();
            Object::new()
                .text("ad_group_id", ad_group_id)
                .set("keywords", Value::Array(keywords))
                .into_value()
        }),
    })
}

fn add_keywords_tool() -> Tool {
    write_tool(
        "add_keywords",
        format!(
            "Add keywords to an ad group of a Search campaign: the searches its ads show for, or with negative the searches they must not show for. Every keyword says how closely a search has to match it. {MONEY}"
        ),
        input_schema(
            properties([
                customer_id_property(),
                ad_group_id_property(),
                json!({
                    "keywords": keyword_list_property(
                        "Keywords to add. EXACT matches searches with the same meaning, PHRASE searches that include that meaning, and BROAD searches related to it. max_cpc is the most a click may cost for this keyword, when the campaign bids with MANUAL_CPC.",
                    ),
                    "negative": {
                        "type": "boolean",
                        "default": false,
                        "description": "Add them as negative keywords, which take no max_cpc.",
                    },
                }),
            ]),
            &["customer_id", "ad_group_id", "keywords"],
        ),
        &ADD_KEYWORDS_VALIDATOR,
        add_keywords,
    )
}

#[derive(Debug, Deserialize)]
struct UpdateKeyword {
    customer_id: String,
    ad_group_id: String,
    criterion_id: String,
    max_cpc: Option<f64>,
    status: Option<String>,
}

async fn update_keyword(
    input: UpdateKeyword,
    context: Arc<BuiltinToolContext>,
) -> BuiltinResult<Plan> {
    let customer = customer_of(&context, &input.customer_id)?;
    let keyword =
        keyword_facts(&context, &customer, &input.ad_group_id, &input.criterion_id).await?;
    let currency = &keyword.ad_group.account.currency;
    let label = format!(
        "\"{}\" ({}{})",
        keyword.text,
        match_word(&keyword.match_type).unwrap_or(&keyword.match_type),
        if keyword.negative { ", negative" } else { "" }
    );
    let mut rows = ad_group_rows(&keyword.ad_group);
    rows.push(row("Keyword", label));
    let criterion_id = input.criterion_id;

    if input.status.as_deref() == Some("REMOVED") {
        if input.max_cpc.is_some() {
            return Err(BuiltinError::tool(
                "A keyword that is removed takes no other change",
            ));
        }
        return Ok(Plan {
            customer,
            operations: vec![
                json!({ "adGroupCriterionOperation": { "remove": keyword.resource_name } }),
            ],
            summary: removed("keyword", &keyword.text, rows),
            result: Box::new(move |_| {
                Object::new()
                    .text("criterion_id", criterion_id)
                    .text("status", "REMOVED")
                    .into_value()
            }),
        });
    }
    if keyword.negative {
        return Err(BuiltinError::tool("A negative keyword can only be removed"));
    }

    let mut fields = Object::new();
    let mut mask: Vec<&'static str> = Vec::new();
    if let Some(max_cpc) = input.max_cpc {
        fields = fields.text("cpcBidMicros", to_micros(max_cpc));
        mask.push("cpc_bid_micros");
        rows.push(change(
            "Most a click may cost",
            money(max_cpc, currency),
            keyword.max_cpc.map_or_else(
                || "The bid of its ad group".to_owned(),
                |before| money(before, currency),
            ),
        ));
    }
    if let Some(status) = &input.status {
        fields = fields.text("status", status.clone());
        mask.push("status");
        rows.push(change(
            "Status",
            status_word(status),
            status_word(&keyword.status),
        ));
    }
    let update = Object::new()
        .text("resourceName", keyword.resource_name.clone())
        .spread(fields);
    Ok(Plan {
        customer,
        operations: vec![json!({
            "adGroupCriterionOperation": { "update": update.into_value(), "updateMask": mask.join(",") },
        })],
        summary: ApprovalSummary {
            title: format!("Change the keyword \"{}\"", keyword.text),
            details: rows,
            warnings: None,
        },
        result: Box::new(move |_| {
            Object::new()
                .text("criterion_id", criterion_id)
                .set("changed", json!(mask))
                .into_value()
        }),
    })
}

fn update_keyword_tool() -> Tool {
    write_tool(
        "update_keyword",
        format!(
            "Change a keyword of an ad group: pause it, enable it, remove it, or set the most a click may cost for it. The text and match type of a keyword cannot be changed: remove it and add another. {MONEY}"
        ),
        input_schema(
            properties([
                customer_id_property(),
                ad_group_id_property(),
                json!({
                    "criterion_id": { "type": "string", "description": "Keyword ID, as returned by list_keywords." },
                    "max_cpc": bid_property("New most a click may cost."),
                    "status": { "type": "string", "enum": GOOGLE_ADS_STATUSES },
                }),
            ]),
            &["customer_id", "ad_group_id", "criterion_id"],
        ),
        &UPDATE_KEYWORD_VALIDATOR,
        update_keyword,
    )
}

/// `texts.map((text) => ({ text }))`.
fn text_assets(texts: &[String]) -> Value {
    texts.iter().map(|text| json!({ "text": text })).collect()
}

#[derive(Debug, Deserialize)]
struct CreateSearchAd {
    customer_id: String,
    ad_group_id: String,
    headlines: Vec<String>,
    descriptions: Vec<String>,
    final_url: String,
    path1: Option<String>,
    path2: Option<String>,
    status: Option<String>,
}

async fn create_search_ad(
    input: CreateSearchAd,
    context: Arc<BuiltinToolContext>,
) -> BuiltinResult<Plan> {
    let customer = customer_of(&context, &input.customer_id)?;
    let path1 = input.path1.as_deref().filter(|path| !path.is_empty());
    let path2 = input.path2.as_deref().filter(|path| !path.is_empty());
    if path2.is_some() && path1.is_none() {
        return Err(BuiltinError::tool(
            "path2 is the second part of the path: set path1 as well",
        ));
    }
    let ad_group = ad_group_facts(&context, &customer, &input.ad_group_id).await?;
    let status = input.status.unwrap_or_else(|| "ENABLED".to_owned());

    let mut texts = Object::new()
        .set("headlines", text_assets(&input.headlines))
        .set("descriptions", text_assets(&input.descriptions));
    if let Some(path1) = path1 {
        texts = texts.text("path1", path1);
    }
    if let Some(path2) = path2 {
        texts = texts.text("path2", path2);
    }

    let mut details = ad_group_rows(&ad_group);
    details.push(row("Opens", input.final_url.clone()));
    if let Some(path1) = path1 {
        let shown: Vec<&str> = [Some(path1), path2].into_iter().flatten().collect();
        details.push(row("Path shown", shown.join("/")));
    }
    details.extend(listed("Headline", &input.headlines));
    details.extend(listed("Description", &input.descriptions));
    details.push(row("Status", status_word(&status)));

    let ad_group_id = ad_group.id.clone();
    Ok(Plan {
        customer,
        operations: vec![json!({
            "adGroupAdOperation": {
                "create": {
                    "adGroup": ad_group.resource_name,
                    "status": status,
                    "ad": {
                        "finalUrls": [input.final_url],
                        "responsiveSearchAd": texts.into_value(),
                    },
                },
            },
        })],
        summary: ApprovalSummary {
            title: format!("Create a search ad in the ad group \"{}\"", ad_group.name),
            details,
            warnings: None,
        },
        result: Box::new(move |created| {
            Object::new()
                .set("ad_id", created_id(created, 0))
                .text("ad_group_id", ad_group_id)
                .text("status", status)
                .into_value()
        }),
    })
}

fn create_search_ad_tool() -> Tool {
    write_tool(
        "create_responsive_search_ad",
        "Create a text ad in an ad group of a Search campaign. Google shows up to 3 of the headlines and 2 of the descriptions at a time, in the combinations that perform best, so each must make sense on its own and none may repeat another. Google reviews an ad before it shows.",
        input_schema(
            properties([
                customer_id_property(),
                ad_group_id_property(),
                json!({
                    "headlines": text_list_property(
                        3,
                        limits::HEADLINES,
                        limits::HEADLINE_LENGTH,
                        "Headlines, all different.",
                    ),
                    "descriptions": text_list_property(
                        2,
                        limits::DESCRIPTIONS,
                        limits::DESCRIPTION_LENGTH,
                        "Descriptions, all different.",
                    ),
                    "final_url": { "type": "string", "description": "The page a click opens." },
                    "path1": {
                        "type": "string",
                        "description": format!(
                            "First part of the path shown after the domain, such as \"shoes\" in example.com/shoes/running. At most {} characters.",
                            limits::PATH_LENGTH
                        ),
                    },
                    "path2": { "type": "string", "description": "Second part of that path. Set it with path1." },
                    "status": { "type": "string", "enum": ["ENABLED", "PAUSED"], "default": "ENABLED" },
                }),
            ]),
            &[
                "customer_id",
                "ad_group_id",
                "headlines",
                "descriptions",
                "final_url",
            ],
        ),
        &CREATE_SEARCH_AD_VALIDATOR,
        create_search_ad,
    )
}

#[derive(Debug, Deserialize)]
struct CreateDisplayAd {
    customer_id: String,
    ad_group_id: String,
    marketing_image_asset_ids: Vec<String>,
    square_marketing_image_asset_ids: Vec<String>,
    square_logo_asset_ids: Option<Vec<String>>,
    wide_logo_asset_ids: Option<Vec<String>>,
    headlines: Vec<String>,
    long_headline: String,
    descriptions: Vec<String>,
    business_name: String,
    final_url: String,
    status: Option<String>,
}

/// `items.map((asset) => ({ asset: asset.resourceName }))`.
fn linked(items: &[&ImageAssetFacts]) -> Value {
    items
        .iter()
        .map(|asset| json!({ "asset": asset.resource_name }))
        .collect()
}

fn shown_images(label: &str, items: &[&ImageAssetFacts]) -> Option<ApprovalDetail> {
    let labels: Vec<String> = items.iter().map(|asset| image_label(asset)).collect();
    (!items.is_empty()).then(|| row(label, labels.join("; ")))
}

async fn create_display_ad(
    input: CreateDisplayAd,
    context: Arc<BuiltinToolContext>,
) -> BuiltinResult<Plan> {
    let customer = customer_of(&context, &input.customer_id)?;
    let square_logo_ids = input.square_logo_asset_ids.unwrap_or_default();
    let wide_logo_ids = input.wide_logo_asset_ids.unwrap_or_default();
    if input.marketing_image_asset_ids.len() + input.square_marketing_image_asset_ids.len()
        > limits::IMAGES
    {
        return Err(BuiltinError::tool(format!(
            "An ad takes at most {} landscape and square images together",
            limits::IMAGES
        )));
    }
    if square_logo_ids.len() + wide_logo_ids.len() > limits::LOGOS {
        return Err(BuiltinError::tool(format!(
            "An ad takes at most {} logos together",
            limits::LOGOS
        )));
    }

    let ad_group = ad_group_facts(&context, &customer, &input.ad_group_id).await?;
    let every_id = [
        input.marketing_image_asset_ids.as_slice(),
        input.square_marketing_image_asset_ids.as_slice(),
        square_logo_ids.as_slice(),
        wide_logo_ids.as_slice(),
    ]
    .concat();
    let assets = image_assets_by_id(&context, &customer, &every_id).await?;
    let by_id: HashMap<&str, &ImageAssetFacts> = assets
        .iter()
        .map(|asset| (asset.id.as_str(), asset))
        .collect();
    let images = |ids: &[String], argument: &str, usage: ImageUse| {
        ids.iter()
            .map(|id| {
                let asset = *by_id
                    .get(id.as_str())
                    .ok_or_else(|| js::unreadable("an image asset that was not asked for"))?;
                require_image(asset, argument, usage)?;
                Ok(asset)
            })
            .collect::<BuiltinResult<Vec<&ImageAssetFacts>>>()
    };
    let landscape = images(
        &input.marketing_image_asset_ids,
        "marketing_image_asset_ids",
        LANDSCAPE,
    )?;
    let square = images(
        &input.square_marketing_image_asset_ids,
        "square_marketing_image_asset_ids",
        SQUARE,
    )?;
    let square_logos = images(&square_logo_ids, "square_logo_asset_ids", SQUARE_LOGO)?;
    let wide_logos = images(&wide_logo_ids, "wide_logo_asset_ids", WIDE_LOGO)?;

    let status = input.status.unwrap_or_else(|| "ENABLED".to_owned());
    let mut display = Object::new()
        .set("marketingImages", linked(&landscape))
        .set("squareMarketingImages", linked(&square));
    if !square_logos.is_empty() {
        display = display.set("squareLogoImages", linked(&square_logos));
    }
    if !wide_logos.is_empty() {
        display = display.set("logoImages", linked(&wide_logos));
    }
    display = display
        .set("headlines", text_assets(&input.headlines))
        .set("longHeadline", json!({ "text": input.long_headline }))
        .set("descriptions", text_assets(&input.descriptions))
        .text("businessName", input.business_name.clone());

    let logos = [square_logos.as_slice(), wide_logos.as_slice()].concat();
    let mut details = ad_group_rows(&ad_group);
    details.push(row("Opens", input.final_url.clone()));
    details.push(row("Business name", input.business_name));
    details.extend(shown_images("Landscape images", &landscape));
    details.extend(shown_images("Square images", &square));
    details.extend(shown_images("Logos", &logos));
    details.extend(listed("Headline", &input.headlines));
    details.push(row("Long headline", input.long_headline));
    details.extend(listed("Description", &input.descriptions));
    details.push(row("Status", status_word(&status)));

    let ad_group_id = ad_group.id.clone();
    Ok(Plan {
        customer,
        operations: vec![json!({
            "adGroupAdOperation": {
                "create": {
                    "adGroup": ad_group.resource_name,
                    "status": status,
                    "ad": {
                        "finalUrls": [input.final_url],
                        "responsiveDisplayAd": display.into_value(),
                    },
                },
            },
        })],
        summary: ApprovalSummary {
            title: format!("Create a display ad in the ad group \"{}\"", ad_group.name),
            details,
            warnings: None,
        },
        result: Box::new(move |created| {
            Object::new()
                .set("ad_id", created_id(created, 0))
                .text("ad_group_id", ad_group_id)
                .text("status", status)
                .into_value()
        }),
    })
}

fn create_display_ad_tool() -> Tool {
    write_tool(
        "create_responsive_display_ad",
        "Create an image ad in an ad group of a Display campaign, from image assets and texts that Google assembles to fit each placement. It needs at least one landscape image (1.91:1, 600×314 pixels or more) and one square image (1:1, 300×300 or more): add images with create_image_upload_link and create_image_asset, or find them with list_assets. Google reviews an ad before it shows.",
        input_schema(
            properties([
                customer_id_property(),
                ad_group_id_property(),
                json!({
                    "marketing_image_asset_ids": id_list_property(
                        limits::IMAGES,
                        "Landscape images (1.91:1, at least 600×314), by asset ID.",
                    ),
                    "square_marketing_image_asset_ids": id_list_property(
                        limits::IMAGES,
                        "Square images (1:1, at least 300×300), by asset ID. With the landscape ones, at most 15.",
                    ),
                    "square_logo_asset_ids": id_list_property(
                        limits::LOGOS,
                        "Square logos (1:1, at least 128×128), by asset ID.",
                    ),
                    "wide_logo_asset_ids": id_list_property(
                        limits::LOGOS,
                        "Wide logos (4:1, at least 512×128), by asset ID. With the square ones, at most 5.",
                    ),
                    "headlines": text_list_property(
                        1,
                        limits::DISPLAY_TEXTS,
                        limits::HEADLINE_LENGTH,
                        "Short headlines.",
                    ),
                    "long_headline": {
                        "type": "string",
                        "maxLength": limits::LONG_HEADLINE_LENGTH,
                        "description": "The headline shown where there is room for a longer one.",
                    },
                    "descriptions": text_list_property(
                        1,
                        limits::DISPLAY_TEXTS,
                        limits::DESCRIPTION_LENGTH,
                        "Descriptions.",
                    ),
                    "business_name": {
                        "type": "string",
                        "maxLength": limits::BUSINESS_NAME_LENGTH,
                        "description": "The name of the advertiser, as the ad shows it.",
                    },
                    "final_url": { "type": "string", "description": "The page a click opens." },
                    "status": { "type": "string", "enum": ["ENABLED", "PAUSED"], "default": "ENABLED" },
                }),
            ]),
            &[
                "customer_id",
                "ad_group_id",
                "marketing_image_asset_ids",
                "square_marketing_image_asset_ids",
                "headlines",
                "long_headline",
                "descriptions",
                "business_name",
                "final_url",
            ],
        ),
        &CREATE_DISPLAY_AD_VALIDATOR,
        create_display_ad,
    )
}

#[derive(Debug, Deserialize)]
struct SetAdStatus {
    customer_id: String,
    ad_group_id: String,
    ad_id: String,
    status: String,
}

async fn set_ad_status(
    input: SetAdStatus,
    context: Arc<BuiltinToolContext>,
) -> BuiltinResult<Plan> {
    let SetAdStatus {
        customer_id,
        ad_group_id,
        ad_id,
        status,
    } = input;
    let customer = customer_of(&context, &customer_id)?;
    let ad = ad_facts(&context, &customer, &ad_group_id, &ad_id).await?;
    let name = ad.headline.clone().unwrap_or_else(|| format!("Ad {ad_id}"));
    let mut rows = ad_group_rows(&ad.ad_group);
    rows.push(row("Ad", format!("{name} (ad {ad_id})")));
    rows.push(change(
        "Status",
        status_word(&status),
        status_word(&ad.status),
    ));

    let (operation, summary) = if status == "REMOVED" {
        (
            json!({ "remove": ad.resource_name }),
            removed("ad", &name, rows),
        )
    } else {
        let verb = if status == "ENABLED" {
            "Enable"
        } else {
            "Pause"
        };
        (
            json!({
                "update": { "resourceName": ad.resource_name, "status": status },
                "updateMask": "status",
            }),
            ApprovalSummary {
                title: format!("{verb} the ad \"{name}\""),
                details: rows,
                warnings: None,
            },
        )
    };
    Ok(Plan {
        customer,
        operations: vec![json!({ "adGroupAdOperation": operation })],
        summary,
        result: Box::new(move |_| {
            Object::new()
                .text("ad_id", ad_id)
                .text("ad_group_id", ad_group_id)
                .text("status", status)
                .into_value()
        }),
    })
}

fn set_ad_status_tool() -> Tool {
    write_tool(
        "set_ad_status",
        "Enable, pause, or remove an ad. The texts and images of an ad cannot be changed here: create a new ad and pause or remove the old one. A removed ad cannot be restored.",
        input_schema(
            properties([
                customer_id_property(),
                ad_group_id_property(),
                json!({
                    "ad_id": { "type": "string", "description": "Ad ID, as returned by list_ads." },
                    "status": { "type": "string", "enum": GOOGLE_ADS_STATUSES },
                }),
            ]),
            &["customer_id", "ad_group_id", "ad_id", "status"],
        ),
        &SET_AD_STATUS_VALIDATOR,
        set_ad_status,
    )
}

#[derive(Debug, Deserialize)]
struct CreateImageUploadLink {
    filename: String,
    content_type: Option<String>,
    expires_in_minutes: Option<i64>,
}

fn create_image_upload_link_tool() -> Tool {
    BuiltinTool::new(
        "create_image_upload_link",
        format!(
            "Get a temporary link to upload one image, so that create_image_asset can add it to a Google Ads account. Send the file as the body of a PUT request to the link, for example with `curl -T banner.png \"<url>\"`, then pass `upload_id` to create_image_asset. The link takes one JPEG, PNG, or GIF file of at most {MAX_IMAGE_MEGABYTES} MB, without signing in, from anyone who has it, until it expires: use it yourself or give it to the user, and never post it anywhere else. An uploaded image can be used for {BUILTIN_UPLOAD_MINUTES} minutes."
        ),
        input_schema(
            json!({
                "filename": {
                    "type": "string",
                    "maxLength": limits::FILENAME_LENGTH,
                    "description": "Name of the file, such as banner.png, without a folder.",
                },
                "content_type": {
                    "type": "string",
                    "description": "Media type of the file, such as image/png. Defaults to the one of its extension.",
                },
                "expires_in_minutes": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": limits::LINK_MINUTES,
                    "default": DEFAULT_LINK_MINUTES,
                    "description": "How long the link takes a file.",
                },
            }),
            &["filename"],
        ),
        &CREATE_IMAGE_UPLOAD_LINK_VALIDATOR,
        |input: CreateImageUploadLink, context: Arc<BuiltinToolContext>| async move {
            let minutes = input.expires_in_minutes.unwrap_or(DEFAULT_LINK_MINUTES);
            let upload_id = Uuid::new_v4().to_string();
            let mut reference = Object::new()
                .text("upload", upload_id.clone())
                .text("filename", input.filename.clone());
            if let Some(content_type) = input.content_type {
                reference = reference.text("content_type", content_type);
            }
            let url = builtin_upload_url(
                &context.env.core,
                context.mcp_id,
                &reference.into_value(),
                Duration::from_secs(minutes.unsigned_abs() * 60),
            )?;
            Ok(Object::new()
                .text("upload_id", upload_id)
                .text("url", url)
                .text("method", "PUT")
                .text(
                    "expires_at",
                    to_iso(Utc::now() + chrono::Duration::minutes(minutes)),
                )
                .text("filename", input.filename)
                .set("max_bytes", json!(limits::IMAGE_BYTES))
                .into_value())
        },
    )
    .write()
}

#[derive(Debug, Deserialize)]
struct CreateImageAsset {
    customer_id: String,
    upload_id: String,
    name: String,
}

async fn create_image_asset(
    input: CreateImageAsset,
    context: Arc<BuiltinToolContext>,
) -> BuiltinResult<Plan> {
    let CreateImageAsset {
        customer_id,
        upload_id,
        name,
    } = input;
    let customer = customer_of(&context, &customer_id)?;
    let uploads = &context.env.uploads;
    let upload = uploads.find(context.mcp_id, &upload_id).await?;
    let bytes = match &upload {
        Some(_) => uploads.read(context.mcp_id, &upload_id).await?,
        None => None,
    };
    let (Some(upload), Some(bytes)) = (upload, bytes) else {
        return Err(BuiltinError::tool(format!(
            "No file is uploaded as \"{upload_id}\". Send the image to the link create_image_upload_link returned with this upload_id, then try again. An uploaded image can be used for {BUILTIN_UPLOAD_MINUTES} minutes."
        )));
    };
    let Some(image) = image_info(&bytes) else {
        return Err(BuiltinError::tool(format!(
            "{} is not a JPEG, PNG, or GIF image, which is what Google Ads takes",
            upload.filename
        )));
    };
    let Some(shape) = image_shape(f64::from(image.width), f64::from(image.height)) else {
        let labels: Vec<&str> = IMAGE_SHAPES.iter().map(|shape| shape.label).collect();
        return Err(BuiltinError::tool(format!(
            "{} is {}×{} pixels, a shape Google Ads has no use for. It takes {} images.",
            upload.filename,
            image.width,
            image.height,
            labels.join(", ")
        )));
    };

    let account = account_facts(&context, &customer).await?;
    let size = format!("{}×{} pixels", image.width, image.height);
    Ok(Plan {
        customer,
        operations: vec![json!({
            "assetOperation": {
                "create": { "name": name, "type": "IMAGE", "imageAsset": { "data": STANDARD.encode(&bytes) } },
            },
        })],
        summary: ApprovalSummary {
            title: format!(
                "Add the image \"{name}\" to the assets of {}",
                account.label
            ),
            details: vec![
                account_row(&account),
                row("Asset name", name.clone()),
                row(
                    "File",
                    format!(
                        "{} ({}, {} KB)",
                        upload.filename,
                        image.format.as_str().to_uppercase(),
                        upload.size.div_ceil(1000)
                    ),
                ),
                row("Size", format!("{size}, {}", shape.label)),
            ],
            warnings: Some(vec![
                "An asset cannot be deleted from Google Ads once it is added.".to_owned(),
            ]),
        },
        result: Box::new(move |created| {
            // Google also takes smaller squares as logos, and nothing below these sizes otherwise.
            let large_enough = if image.width >= shape.min_width && image.height >= shape.min_height
            {
                Value::Bool(true)
            } else {
                Value::from(format!(
                    "Below the {}×{} pixels Google Ads asks of {} images{}",
                    shape.min_width,
                    shape.min_height,
                    shape.label,
                    if shape.name == "square" && image.width >= SQUARE_LOGO_MIN_PIXELS {
                        ", but usable as a logo"
                    } else {
                        ""
                    }
                ))
            };
            Object::new()
                .set("asset_id", created_id(created, 0))
                .text("name", name)
                .set("width", json!(image.width))
                .set("height", json!(image.height))
                .text("shape", shape.name)
                .set("large_enough", large_enough)
                .into_value()
        }),
    })
}

fn create_image_asset_tool() -> Tool {
    write_tool(
        "create_image_asset",
        "Add an uploaded image to the assets of a Google Ads account, where ads can use it. Returns its asset ID and what Google Ads can use it as: a landscape image (1.91:1), a square image or logo (1:1), a wide logo (4:1), or a portrait image (4:5). An asset cannot be deleted or changed afterwards.",
        input_schema(
            properties([
                customer_id_property(),
                json!({
                    "upload_id": {
                        "type": "string",
                        "description": "The upload_id create_image_upload_link returned, once the file was sent to its link.",
                    },
                    "name": {
                        "type": "string",
                        "maxLength": limits::NAME_LENGTH,
                        "description": "Name of the asset in the account, such as \"Spring sale banner\". No other image asset may have it.",
                    },
                }),
            ]),
            &["customer_id", "upload_id", "name"],
        ),
        &CREATE_IMAGE_ASSET_VALIDATOR,
        create_image_asset,
    )
}

#[derive(Debug, Deserialize)]
struct AddCampaignImages {
    customer_id: String,
    campaign_id: String,
    asset_ids: Vec<String>,
}

async fn add_campaign_images(
    input: AddCampaignImages,
    context: Arc<BuiltinToolContext>,
) -> BuiltinResult<Plan> {
    let customer = customer_of(&context, &input.customer_id)?;
    let campaign = campaign_facts(&context, &customer, &input.campaign_id).await?;
    let assets = image_assets_by_id(&context, &customer, &input.asset_ids).await?;
    for asset in &assets {
        let usage = if shape_name(asset) == Some("landscape") {
            LANDSCAPE
        } else {
            SQUARE
        };
        require_image(asset, "asset_ids", usage)?;
    }

    let labels: Vec<String> = assets.iter().map(image_label).collect();
    let mut details = vec![
        account_row(&campaign.account),
        campaign_row(&campaign.name, &campaign.status),
    ];
    details.extend(listed("Image", &labels));
    let (campaign_id, asset_ids) = (campaign.id.clone(), input.asset_ids);
    Ok(Plan {
        customer,
        operations: assets
            .iter()
            .map(|asset| {
                json!({
                    "campaignAssetOperation": {
                        "create": {
                            "campaign": campaign.resource_name,
                            "asset": asset.resource_name,
                            "fieldType": "AD_IMAGE",
                        },
                    },
                })
            })
            .collect(),
        summary: ApprovalSummary {
            title: format!(
                "Show {} {} with the ads of the campaign \"{}\"",
                assets.len(),
                if assets.len() == 1 { "image" } else { "images" },
                campaign.name
            ),
            details,
            warnings: None,
        },
        result: Box::new(move |_| {
            Object::new()
                .text("campaign_id", campaign_id)
                .set("asset_ids", json!(asset_ids))
                .into_value()
        }),
    })
}

fn add_campaign_images_tool() -> Tool {
    write_tool(
        "add_campaign_images",
        "Show images beside the text ads of a Search campaign, from image assets of the account. They must be square (1:1, at least 300×300 pixels) or landscape (1.91:1, at least 600×314). Google reviews them, and only shows images for advertisers it has verified.",
        input_schema(
            properties([
                customer_id_property(),
                campaign_id_property(),
                json!({
                    "asset_ids": id_list_property(limits::IMAGES, "Image assets to show, by asset ID."),
                }),
            ]),
            &["customer_id", "campaign_id", "asset_ids"],
        ),
        &ADD_CAMPAIGN_IMAGES_VALIDATOR,
        add_campaign_images,
    )
}

/// The write tools, in the order agents list them. (`googleAdsWriteTools`)
pub fn google_ads_write_tools() -> Vec<BuiltinTool<BuiltinToolContext>> {
    vec![
        create_campaign_tool(),
        update_campaign_tool(),
        set_campaign_status_tool(),
        update_campaign_budget_tool(),
        update_campaign_targeting_tool(),
        create_ad_group_tool(),
        update_ad_group_tool(),
        add_keywords_tool(),
        update_keyword_tool(),
        create_search_ad_tool(),
        create_display_ad_tool(),
        set_ad_status_tool(),
        create_image_upload_link_tool(),
        create_image_asset_tool(),
        add_campaign_images_tool(),
    ]
}
