//! The tools that read an account: what it holds, how it performs, and what
//! was changed in it.

use std::sync::Arc;

use chrono::{Duration, Utc};
use mymcps_builtin::arguments::{NO_ARGUMENTS_VALIDATOR, NoArguments};
use mymcps_builtin::{BuiltinError, BuiltinResult, BuiltinTool, BuiltinToolContext};
use mymcps_vine as vine;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::api::{
    GoogleAdsRequest, GoogleAdsRow, allowed_customers, customer_of, google_ads_request,
    search_google_ads,
};
use crate::format::{customer_label, customer_number, from_micros, language_code, percent};
use crate::js::{self, Object};
use crate::lookup::languages_by_code;
use crate::validators::{
    CAMPAIGN_VALIDATOR, GET_PERFORMANCE_VALIDATOR, GOOGLE_ADS_ASSET_TYPES, GOOGLE_ADS_DATE_RANGES,
    GOOGLE_ADS_PERFORMANCE_LEVELS, GOOGLE_ADS_SEGMENTS, KEYWORD_IDEAS_VALIDATOR,
    LIST_AD_GROUPS_VALIDATOR, LIST_ASSETS_VALIDATOR, LIST_CAMPAIGNS_VALIDATOR,
    LIST_CHANGES_VALIDATOR, LIST_IN_CAMPAIGN_VALIDATOR, RUN_QUERY_VALIDATOR,
    SEARCH_LOCATIONS_VALIDATOR, limits,
};

const MONEY: &str = "Amounts are in the account's currency, which each result names.";
const DEFAULT_ROWS: usize = 100;
const DEFAULT_DATE_RANGE: &str = "LAST_30_DAYS";
const MAX_LISTED_ACCOUNTS: usize = 50;
const MAX_LISTED_CLIENTS: usize = 200;
const MAX_ROWS: usize = limits::ROWS as usize;

/// Averages are worked out from these, so that no unit is left to guess.
const METRICS: &str = "metrics.impressions, metrics.clicks, metrics.cost_micros, metrics.ctr, metrics.conversions, metrics.conversions_value";

type Tool = BuiltinTool<BuiltinToolContext>;

/// `{ ...a, ...b }`: the properties of a tool's arguments, in the order they are written.
pub(crate) fn properties(parts: impl IntoIterator<Item = Value>) -> Value {
    let mut merged = Map::new();
    for part in parts {
        if let Value::Object(part) = part {
            merged.extend(part);
        }
    }
    Value::Object(merged)
}

/// The arguments of a tool as the agent reads them.
pub(crate) fn input_schema(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

pub(crate) fn customer_id_property() -> Value {
    json!({
        "customer_id": {
            "type": "string",
            "description": "Google Ads account ID, such as 123-456-7890, as returned by list_accounts.",
        },
    })
}

fn period_properties() -> Value {
    json!({
        "date_range": {
            "type": "string",
            "enum": GOOGLE_ADS_DATE_RANGES,
            "default": DEFAULT_DATE_RANGE,
            "description": "The days the figures cover, in the account's time zone. LAST_n_DAYS ranges end yesterday.",
        },
        "start_date": {
            "type": "string",
            "description": "First day of a custom period, such as 2026-01-01. Set it with end_date.",
        },
        "end_date": {
            "type": "string",
            "description": "Last day of a custom period, such as 2026-01-31. Set it with start_date.",
        },
    })
}

fn limit_property(what: &str) -> Value {
    json!({
        "limit": {
            "type": "integer",
            "minimum": 1,
            "maximum": limits::ROWS,
            "default": DEFAULT_ROWS,
            "description": format!("Most {what} to return."),
        },
    })
}

fn campaign_filter_property() -> Value {
    json!({ "campaign_id": { "type": "string", "description": "Only what belongs to this campaign." } })
}

fn ad_group_filter_property() -> Value {
    json!({ "ad_group_id": { "type": "string", "description": "Only what belongs to this ad group." } })
}

#[derive(Debug, Deserialize)]
struct Period {
    date_range: Option<String>,
    start_date: Option<String>,
    end_date: Option<String>,
}

/// The days a report covers, as a GAQL condition and as the result says them back.
struct Days {
    condition: String,
    period: String,
}

fn period_of(dates: &Period) -> BuiltinResult<Days> {
    match (dates.start_date.as_deref(), dates.end_date.as_deref()) {
        (None, None) => {
            let named = dates.date_range.as_deref().unwrap_or(DEFAULT_DATE_RANGE);
            Ok(Days {
                condition: format!("segments.date DURING {named}"),
                period: named.to_owned(),
            })
        }
        (Some(start), Some(end)) if start > end => {
            Err(BuiltinError::tool("start_date must not be after end_date"))
        }
        (Some(start), Some(end)) => Ok(Days {
            condition: format!("segments.date BETWEEN '{start}' AND '{end}'"),
            period: format!("{start} to {end}"),
        }),
        _ => Err(BuiltinError::tool(
            "Set both start_date and end_date, or neither and use date_range",
        )),
    }
}

fn round(value: f64) -> f64 {
    js::math_round(value * 100.0) / 100.0
}

/// The figures of a row, in whole units of the account's currency.
fn figures(metrics: Option<&Value>) -> Object {
    let clicks = js::number_or(js::get(metrics, "clicks"), 0.0);
    let cost = from_micros(js::get(metrics, "costMicros")).unwrap_or(0.0);
    let conversions = js::number_or(js::get(metrics, "conversions"), 0.0);
    let per = |count: f64| {
        if count > 0.0 {
            js::json_number(round(cost / count))
        } else {
            Value::Null
        }
    };
    Object::new()
        .number(
            "impressions",
            js::number_or(js::get(metrics, "impressions"), 0.0),
        )
        .number("clicks", clicks)
        .number("cost", round(cost))
        .number(
            "ctr_percent",
            percent(js::get(metrics, "ctr")).unwrap_or(0.0),
        )
        .set("average_cpc", per(clicks))
        .number("conversions", round(conversions))
        .number(
            "conversion_value",
            round(js::number_or(js::get(metrics, "conversionsValue"), 0.0)),
        )
        .set("cost_per_conversion", per(conversions))
}

fn filters(conditions: impl IntoIterator<Item = Option<String>>) -> String {
    conditions
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn currency_of(rows: &[GoogleAdsRow]) -> Value {
    js::or_null(js::get(
        rows.first().and_then(|row| row.get("customer")),
        "currencyCode",
    ))
}

/// `timestamp?.slice(0, 10) ?? null`: the day of a `yyyy-MM-dd HH:mm:ss` timestamp.
fn day(timestamp: Option<&Value>) -> BuiltinResult<Value> {
    Ok(js::slice_of(timestamp, 10, "a date that is not text")?.map_or(Value::Null, Value::from))
}

/// An amount Google answers 0 for when it is not set: `fromMicros(micros) || null`.
fn set_amount(micros: Option<&Value>) -> Value {
    from_micros(micros)
        .filter(|amount| *amount != 0.0)
        .map_or(Value::Null, js::json_number)
}

fn texts(assets: Option<&Value>) -> BuiltinResult<Value> {
    let mut texts = Vec::new();
    for asset in js::items(assets, "texts that are not a list")? {
        if asset.is_null() {
            return Err(js::unreadable("a text that is missing"));
        }
        texts.extend(
            js::get(asset, "text")
                .filter(|text| js::truthy(Some(text)))
                .cloned(),
        );
    }
    Ok(Value::Array(texts))
}

/// One account, with what tells whether it is a manager and what agents call it by.
struct Account {
    customer_id: String,
    manager: bool,
    row: Object,
}

fn account_row(account: Option<&Value>) -> Account {
    let customer_id = customer_label(&js::string(js::get(account, "id")));
    let manager = js::truthy(js::get(account, "manager"));
    let row = Object::new()
        .text("customer_id", customer_id.clone())
        .set("name", js::or_null(js::get(account, "descriptiveName")))
        .set("currency", js::or_null(js::get(account, "currencyCode")))
        .set("time_zone", js::or_null(js::get(account, "timeZone")))
        .flag("manager", manager)
        .flag("test_account", js::truthy(js::get(account, "testAccount")))
        .set("status", js::or_null(js::get(account, "status")));
    Account {
        customer_id,
        manager,
        row,
    }
}

/// An account as `list_accounts` returns it, and the ID it is filtered by.
type Listed = (String, Value);

async fn open_account(
    context: &BuiltinToolContext,
    customer: &str,
    accounts: &mut Vec<Listed>,
    clients: &mut Vec<Listed>,
) -> BuiltinResult<()> {
    let found = search_google_ads(
        context,
        customer,
        "SELECT customer.id, customer.descriptive_name, customer.currency_code, customer.time_zone, customer.manager, customer.test_account, customer.status FROM customer LIMIT 1",
        1,
    )
    .await?;
    let known = js::defined(found.rows.first().and_then(|row| row.get("customer")));
    let account = match known {
        Some(known) => account_row(Some(known)),
        None => account_row(Some(&json!({ "id": customer }))),
    };
    accounts.push((account.customer_id.clone(), account.row.into_value()));

    if account.manager {
        let managed = search_google_ads(
            context,
            customer,
            "SELECT customer_client.id, customer_client.descriptive_name, customer_client.currency_code, customer_client.time_zone, customer_client.manager, customer_client.test_account, customer_client.status FROM customer_client WHERE customer_client.level = 1 AND customer_client.status = 'ENABLED'",
            MAX_LISTED_CLIENTS,
        )
        .await?;
        for row in &managed.rows {
            let client = account_row(js::defined(row.get("customerClient")));
            let listed = client.row.text("manager_id", account.customer_id.clone());
            clients.push((client.customer_id, listed.into_value()));
        }
    }
    Ok(())
}

async fn list_accounts(context: &BuiltinToolContext) -> BuiltinResult<Value> {
    let allowed = allowed_customers(context);
    let answer = google_ads_request(
        context,
        "/customers:listAccessibleCustomers",
        GoogleAdsRequest::get(),
    )
    .await?;
    let names: &[Value] = match (&answer, js::get(&answer, "resourceNames")) {
        (Value::Null, _) | (_, Some(Value::Null)) => {
            return Err(js::unreadable("no list of accounts"));
        }
        (_, None) => &[],
        (_, Some(Value::Array(names))) => names,
        (_, Some(_)) => return Err(js::unreadable("accounts that are not a list")),
    };
    let mut direct = names
        .iter()
        .map(|name| name.as_str().map(|name| name.replacen("customers/", "", 1)))
        .collect::<Option<Vec<String>>>()
        .ok_or_else(|| js::unreadable("an account without a resource name"))?;
    direct.truncate(MAX_LISTED_ACCOUNTS);

    let mut accounts: Vec<Listed> = Vec::new();
    let mut clients: Vec<Listed> = Vec::new();
    for customer in &direct {
        match open_account(context, customer, &mut accounts, &mut clients).await {
            Ok(()) => {}
            // One account that cannot be opened must not hide the others.
            Err(error) if error.is_tool_error() => {
                let customer_id = customer_label(customer);
                let failed = Object::new()
                    .text("customer_id", customer_id.clone())
                    .text("error", error.to_string());
                accounts.push((customer_id, failed.into_value()));
            }
            Err(error) => return Err(error),
        }
    }

    let usable = |listed: Vec<Listed>| -> Value {
        listed
            .into_iter()
            .filter(|(customer_id, _)| {
                allowed
                    .as_ref()
                    .is_none_or(|allowed| allowed.contains(&customer_number(customer_id).as_str()))
            })
            .map(|(_, account)| account)
            .collect()
    };
    let manager = context
        .settings
        .get("loginCustomerId")
        .filter(|manager| !manager.is_empty());
    Ok(Object::new()
        .set("accounts", usable(accounts))
        .set("client_accounts", usable(clients))
        .set(
            "manager_account_id",
            manager.map_or(Value::Null, |manager| Value::from(customer_label(manager))),
        )
        .set(
            "limited_to",
            allowed.as_ref().map_or(Value::Null, |allowed| {
                allowed.iter().map(|id| customer_label(id)).collect()
            }),
        )
        .into_value())
}

fn list_accounts_tool() -> Tool {
    BuiltinTool::new(
        "list_accounts",
        "List the Google Ads accounts this MCP can use: the ones the connected Google sign-in opens directly, and the client accounts of those that are manager accounts. Campaigns live in client accounts, never in a manager account. Each account comes with its currency and time zone, which every amount and date of the other tools follows.",
        json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        &NO_ARGUMENTS_VALIDATOR,
        |_: NoArguments, context: Arc<BuiltinToolContext>| async move { list_accounts(&context).await },
    )
}

#[derive(Debug, Deserialize)]
struct ListCampaigns {
    customer_id: String,
    status: Option<String>,
    #[serde(flatten)]
    dates: Period,
    limit: Option<usize>,
}

fn list_campaigns_tool() -> Tool {
    BuiltinTool::new(
        "list_campaigns",
        format!(
            "List the campaigns of an account with their status, type, bidding strategy, daily budget, and figures over a period: impressions, clicks, cost, click-through rate, average cost per click, conversions, and conversion value. Removed campaigns are left out. {MONEY}"
        ),
        input_schema(
            properties([
                customer_id_property(),
                json!({
                    "status": {
                        "type": "string",
                        "enum": ["ENABLED", "PAUSED", "ALL"],
                        "default": "ALL",
                        "description": "Only campaigns with this status. ALL is enabled and paused ones.",
                    },
                }),
                period_properties(),
                limit_property("campaigns"),
            ]),
            &["customer_id"],
        ),
        &LIST_CAMPAIGNS_VALIDATOR,
        |input: ListCampaigns, context: Arc<BuiltinToolContext>| async move {
            let status = input.status.as_deref().unwrap_or("ALL");
            let limit = input.limit.unwrap_or(DEFAULT_ROWS);
            let customer = customer_of(&context, &input.customer_id)?;
            let Days { condition, period } = period_of(&input.dates)?;
            let found = search_google_ads(
                &context,
                &customer,
                &format!(
                    "SELECT campaign.id, campaign.name, campaign.status, campaign.primary_status, campaign.advertising_channel_type, campaign.bidding_strategy_type, campaign.start_date_time, campaign.end_date_time, campaign_budget.amount_micros, campaign_budget.reference_count, customer.currency_code, {METRICS} FROM campaign WHERE {} ORDER BY metrics.cost_micros DESC LIMIT {}",
                    filters([
                        Some(if status == "ALL" {
                            "campaign.status != 'REMOVED'".to_owned()
                        } else {
                            format!("campaign.status = '{status}'")
                        }),
                        Some(condition),
                    ]),
                    limit + 1,
                ),
                limit,
            )
            .await?;
            let campaigns = found
                .rows
                .iter()
                .map(|row| {
                    let campaign = js::resource(row, "campaign")?;
                    let budget = row.get("campaignBudget");
                    Ok(Object::new()
                        .text("id", js::string(js::get(campaign, "id")))
                        .field("name", js::get(campaign, "name"))
                        .field("status", js::get(campaign, "status"))
                        .set("serving", js::or_null(js::get(campaign, "primaryStatus")))
                        .field("type", js::get(campaign, "advertisingChannelType"))
                        .field("bidding_strategy", js::get(campaign, "biddingStrategyType"))
                        .set(
                            "daily_budget",
                            from_micros(js::get(budget, "amountMicros"))
                                .map_or(Value::Null, js::json_number),
                        )
                        .number(
                            "budget_shared_by",
                            js::number_or(js::get(budget, "referenceCount"), 1.0),
                        )
                        .set("start_date", day(js::get(campaign, "startDateTime"))?)
                        .set("end_date", day(js::get(campaign, "endDateTime"))?)
                        .spread(figures(row.get("metrics")))
                        .into_value())
                })
                .collect::<BuiltinResult<Vec<Value>>>()?;
            Ok(Object::new()
                .set("currency", currency_of(&found.rows))
                .text("period", period)
                .set("campaigns", Value::Array(campaigns))
                .flag("truncated", found.truncated)
                .into_value())
        },
    )
}

#[derive(Debug, Deserialize)]
struct GetCampaign {
    customer_id: String,
    campaign_id: String,
}

async fn get_campaign(input: GetCampaign, context: &BuiltinToolContext) -> BuiltinResult<Value> {
    let campaign_id = &input.campaign_id;
    let customer = customer_of(context, &input.customer_id)?;
    let found = search_google_ads(
        context,
        &customer,
        &format!(
            "SELECT campaign.id, campaign.name, campaign.status, campaign.primary_status, campaign.primary_status_reasons, campaign.advertising_channel_type, campaign.bidding_strategy_type, campaign.start_date_time, campaign.end_date_time, campaign.network_settings.target_google_search, campaign.network_settings.target_search_network, campaign.network_settings.target_content_network, campaign.target_spend.cpc_bid_ceiling_micros, campaign.maximize_conversions.target_cpa_micros, campaign.maximize_conversion_value.target_roas, campaign.contains_eu_political_advertising, campaign.optimization_score, campaign_budget.id, campaign_budget.amount_micros, campaign_budget.reference_count, customer.currency_code FROM campaign WHERE campaign.id = {campaign_id} LIMIT 1"
        ),
        1,
    )
    .await?;
    let Some(row) = found.rows.first() else {
        return Err(BuiltinError::tool(format!(
            "There is no campaign {campaign_id} in the Google Ads account {}",
            customer_label(&customer)
        )));
    };

    let criteria = search_google_ads(
        context,
        &customer,
        &format!(
            "SELECT campaign_criterion.criterion_id, campaign_criterion.type, campaign_criterion.negative, campaign_criterion.display_name, campaign_criterion.keyword.text, campaign_criterion.keyword.match_type FROM campaign_criterion WHERE campaign.id = {campaign_id} AND campaign_criterion.type IN ('LOCATION', 'LANGUAGE', 'KEYWORD') AND campaign_criterion.status != 'REMOVED' LIMIT {}",
            limits::ROWS
        ),
        MAX_ROWS,
    )
    .await?;
    let ad_groups = search_google_ads(
        context,
        &customer,
        &format!(
            "SELECT ad_group.id, ad_group.name, ad_group.status, ad_group.type, ad_group.cpc_bid_micros FROM ad_group WHERE campaign.id = {campaign_id} AND ad_group.status != 'REMOVED' LIMIT {}",
            limits::ROWS
        ),
        MAX_ROWS,
    )
    .await?;

    let campaign = js::resource(row, "campaign")?;
    let budget = row.get("campaignBudget");
    let networks = js::get(campaign, "networkSettings");
    let criteria = criteria
        .rows
        .iter()
        .map(|row| js::resource(row, "campaignCriterion"))
        .collect::<BuiltinResult<Vec<&Value>>>()?;
    let of_type = |kind: &str, describe: fn(&Value, Object) -> Object| -> Value {
        criteria
            .iter()
            .filter(|criterion| js::get(**criterion, "type").and_then(Value::as_str) == Some(kind))
            .map(|criterion| {
                let listed = Object::new().text(
                    "criterion_id",
                    js::string(js::get(*criterion, "criterionId")),
                );
                describe(criterion, listed).into_value()
            })
            .collect()
    };
    let ad_groups = ad_groups
        .rows
        .iter()
        .map(|row| {
            let ad_group = js::resource(row, "adGroup")?;
            Ok(Object::new()
                .text("id", js::string(js::get(ad_group, "id")))
                .field("name", js::get(ad_group, "name"))
                .field("status", js::get(ad_group, "status"))
                .field("type", js::get(ad_group, "type"))
                .set("max_cpc", set_amount(js::get(ad_group, "cpcBidMicros")))
                .into_value())
        })
        .collect::<BuiltinResult<Vec<Value>>>()?;

    Ok(Object::new()
        .set(
            "currency",
            js::or_null(js::get(row.get("customer"), "currencyCode")),
        )
        .text("id", js::string(js::get(campaign, "id")))
        .field("name", js::get(campaign, "name"))
        .field("status", js::get(campaign, "status"))
        .set("serving", js::or_null(js::get(campaign, "primaryStatus")))
        .set(
            "serving_reasons",
            js::defined(js::get(campaign, "primaryStatusReasons"))
                .cloned()
                .unwrap_or_else(|| json!([])),
        )
        .field("type", js::get(campaign, "advertisingChannelType"))
        .set("start_date", day(js::get(campaign, "startDateTime"))?)
        .set("end_date", day(js::get(campaign, "endDateTime"))?)
        .set(
            "daily_budget",
            from_micros(js::get(budget, "amountMicros")).map_or(Value::Null, js::json_number),
        )
        .number(
            "budget_shared_by",
            js::number_or(js::get(budget, "referenceCount"), 1.0),
        )
        .set(
            "bidding",
            Object::new()
                .field("strategy", js::get(campaign, "biddingStrategyType"))
                .set(
                    "max_cpc",
                    set_amount(js::get(
                        js::get(campaign, "targetSpend"),
                        "cpcBidCeilingMicros",
                    )),
                )
                .set(
                    "target_cpa",
                    set_amount(js::get(
                        js::get(campaign, "maximizeConversions"),
                        "targetCpaMicros",
                    )),
                )
                .set(
                    "target_roas",
                    js::truthy_or_null(js::get(
                        js::get(campaign, "maximizeConversionValue"),
                        "targetRoas",
                    )),
                )
                .into_value(),
        )
        .set(
            "networks",
            Object::new()
                .flag(
                    "google_search",
                    js::truthy(js::get(networks, "targetGoogleSearch")),
                )
                .flag(
                    "search_partners",
                    js::truthy(js::get(networks, "targetSearchNetwork")),
                )
                .flag(
                    "display_network",
                    js::truthy(js::get(networks, "targetContentNetwork")),
                )
                .into_value(),
        )
        .flag(
            "eu_political_ads",
            js::get(campaign, "containsEuPoliticalAdvertising").and_then(Value::as_str)
                == Some("CONTAINS_EU_POLITICAL_ADVERTISING"),
        )
        .set(
            "optimization_score",
            js::or_null(js::get(campaign, "optimizationScore")),
        )
        .set(
            "locations",
            of_type("LOCATION", |criterion, listed| {
                listed
                    .set("name", js::or_null(js::get(criterion, "displayName")))
                    .flag("excluded", js::truthy(js::get(criterion, "negative")))
            }),
        )
        .set(
            "languages",
            of_type("LANGUAGE", |criterion, listed| {
                listed.set("name", js::or_null(js::get(criterion, "displayName")))
            }),
        )
        .set(
            "negative_keywords",
            of_type("KEYWORD", |criterion, listed| {
                let keyword = js::get(criterion, "keyword");
                listed
                    .set("text", js::or_null(js::get(keyword, "text")))
                    .set("match_type", js::or_null(js::get(keyword, "matchType")))
            }),
        )
        .set("ad_groups", Value::Array(ad_groups))
        .into_value())
}

fn get_campaign_tool() -> Tool {
    BuiltinTool::new(
        "get_campaign",
        format!(
            "Get one campaign in full: settings, budget, bidding, networks, the locations and languages it targets, its negative keywords, and its ad groups. Criterion IDs are the ones update_campaign_targeting removes. {MONEY}"
        ),
        input_schema(
            properties([
                customer_id_property(),
                json!({ "campaign_id": { "type": "string", "description": "Campaign ID, as returned by list_campaigns." } }),
            ]),
            &["customer_id", "campaign_id"],
        ),
        &CAMPAIGN_VALIDATOR,
        |input: GetCampaign, context: Arc<BuiltinToolContext>| async move {
            get_campaign(input, &context).await
        },
    )
}

#[derive(Debug, Deserialize)]
struct ListAdGroups {
    customer_id: String,
    campaign_id: Option<String>,
    #[serde(flatten)]
    dates: Period,
    limit: Option<usize>,
}

fn list_ad_groups_tool() -> Tool {
    BuiltinTool::new(
        "list_ad_groups",
        format!(
            "List the ad groups of an account, or of one campaign, with their status, default bid, and figures over a period. Removed ad groups are left out. {MONEY}"
        ),
        input_schema(
            properties([
                customer_id_property(),
                campaign_filter_property(),
                period_properties(),
                limit_property("ad groups"),
            ]),
            &["customer_id"],
        ),
        &LIST_AD_GROUPS_VALIDATOR,
        |input: ListAdGroups, context: Arc<BuiltinToolContext>| async move {
            let limit = input.limit.unwrap_or(DEFAULT_ROWS);
            let customer = customer_of(&context, &input.customer_id)?;
            let Days { condition, period } = period_of(&input.dates)?;
            let found = search_google_ads(
                &context,
                &customer,
                &format!(
                    "SELECT ad_group.id, ad_group.name, ad_group.status, ad_group.type, ad_group.cpc_bid_micros, campaign.id, campaign.name, customer.currency_code, {METRICS} FROM ad_group WHERE {} ORDER BY metrics.cost_micros DESC LIMIT {}",
                    filters([
                        Some("ad_group.status != 'REMOVED'".to_owned()),
                        input.campaign_id.as_ref().map(|campaign_id| format!("campaign.id = {campaign_id}")),
                        Some(condition),
                    ]),
                    limit + 1,
                ),
                limit,
            )
            .await?;
            let ad_groups = found
                .rows
                .iter()
                .map(|row| {
                    let ad_group = js::resource(row, "adGroup")?;
                    let campaign = js::resource(row, "campaign")?;
                    Ok(Object::new()
                        .text("id", js::string(js::get(ad_group, "id")))
                        .field("name", js::get(ad_group, "name"))
                        .field("status", js::get(ad_group, "status"))
                        .field("type", js::get(ad_group, "type"))
                        .set("max_cpc", set_amount(js::get(ad_group, "cpcBidMicros")))
                        .text("campaign_id", js::string(js::get(campaign, "id")))
                        .field("campaign", js::get(campaign, "name"))
                        .spread(figures(row.get("metrics")))
                        .into_value())
                })
                .collect::<BuiltinResult<Vec<Value>>>()?;
            Ok(Object::new()
                .set("currency", currency_of(&found.rows))
                .text("period", period)
                .set("ad_groups", Value::Array(ad_groups))
                .flag("truncated", found.truncated)
                .into_value())
        },
    )
}

/// What a list is narrowed to, and the days its figures cover.
#[derive(Debug, Deserialize)]
struct ListInCampaign {
    customer_id: String,
    campaign_id: Option<String>,
    ad_group_id: Option<String>,
    #[serde(flatten)]
    dates: Period,
    limit: Option<usize>,
}

impl ListInCampaign {
    fn campaign_filter(&self) -> Option<String> {
        self.campaign_id
            .as_ref()
            .map(|campaign_id| format!("campaign.id = {campaign_id}"))
    }

    fn ad_group_filter(&self) -> Option<String> {
        self.ad_group_id
            .as_ref()
            .map(|ad_group_id| format!("ad_group.id = {ad_group_id}"))
    }
}

fn list_in_campaign_schema(what: &str) -> Value {
    input_schema(
        properties([
            customer_id_property(),
            campaign_filter_property(),
            ad_group_filter_property(),
            period_properties(),
            limit_property(what),
        ]),
        &["customer_id"],
    )
}

/// Where a row of an ad group belongs: `ad_group_id`, `ad_group`, `campaign_id`, `campaign`.
fn ad_group_columns(row: &GoogleAdsRow) -> BuiltinResult<Object> {
    let ad_group = js::resource(row, "adGroup")?;
    let campaign = js::resource(row, "campaign")?;
    Ok(Object::new()
        .text("ad_group_id", js::string(js::get(ad_group, "id")))
        .field("ad_group", js::get(ad_group, "name"))
        .text("campaign_id", js::string(js::get(campaign, "id")))
        .field("campaign", js::get(campaign, "name")))
}

fn ad_row(row: &GoogleAdsRow) -> BuiltinResult<Value> {
    let ad_group_ad = js::resource(row, "adGroupAd")?;
    let ad = js::get(ad_group_ad, "ad");
    let policy = js::get(ad_group_ad, "policySummary");
    let search = js::get(ad, "responsiveSearchAd");
    let display = js::get(ad, "responsiveDisplayAd");
    let shown = js::defined(search).or(display);

    let mut listed = Object::new()
        .text("id", js::string(js::get(ad, "id")))
        .field("type", js::get(ad, "type"))
        .field("status", js::get(ad_group_ad, "status"))
        .set(
            "final_url",
            js::first(js::get(ad, "finalUrls")).unwrap_or(Value::Null),
        )
        .set("headlines", texts(js::get(shown, "headlines"))?);
    if js::truthy(display) {
        listed = listed.set(
            "long_headline",
            js::or_null(js::get(js::get(display, "longHeadline"), "text")),
        );
    }
    listed = listed.set("descriptions", texts(js::get(shown, "descriptions"))?);
    if js::truthy(search) {
        let path: Vec<String> = [js::get(search, "path1"), js::get(search, "path2")]
            .into_iter()
            .flatten()
            .filter(|part| js::truthy(Some(part)))
            .map(vine::js::to_string)
            .collect();
        listed = listed.text("path", path.join("/"));
    }
    if js::truthy(display) {
        listed = listed.set(
            "business_name",
            js::or_null(js::get(display, "businessName")),
        );
    }

    let policy_topics: Vec<Value> = js::items(
        js::get(policy, "policyTopicEntries"),
        "policy topics that are not a list",
    )?
    .iter()
    .map(|entry| {
        if entry.is_null() {
            return Err(js::unreadable("a policy topic that is missing"));
        }
        Ok(Value::from(format!(
            "{} ({})",
            js::string(js::get(entry, "topic")),
            js::string(js::get(entry, "type"))
        )))
    })
    .collect::<BuiltinResult<_>>()?;
    Ok(listed
        .set("approval", js::or_null(js::get(policy, "approvalStatus")))
        .set("review", js::or_null(js::get(policy, "reviewStatus")))
        .set("policy_topics", Value::Array(policy_topics))
        .set(
            "ad_strength",
            js::or_null(js::get(ad_group_ad, "adStrength")),
        )
        .spread(ad_group_columns(row)?)
        .spread(figures(row.get("metrics")))
        .into_value())
}

fn list_ads_tool() -> Tool {
    BuiltinTool::new(
        "list_ads",
        format!(
            "List the ads of an account, a campaign, or an ad group with their texts, landing page, status, Google's review of them (approval, policy topics, ad strength), and figures over a period. Removed ads are left out. {MONEY}"
        ),
        list_in_campaign_schema("ads"),
        &LIST_IN_CAMPAIGN_VALIDATOR,
        |input: ListInCampaign, context: Arc<BuiltinToolContext>| async move {
            let limit = input.limit.unwrap_or(DEFAULT_ROWS);
            let customer = customer_of(&context, &input.customer_id)?;
            let Days { condition, period } = period_of(&input.dates)?;
            let found = search_google_ads(
                &context,
                &customer,
                &format!(
                    "SELECT ad_group_ad.ad.id, ad_group_ad.ad.type, ad_group_ad.ad.final_urls, ad_group_ad.ad.responsive_search_ad.headlines, ad_group_ad.ad.responsive_search_ad.descriptions, ad_group_ad.ad.responsive_search_ad.path1, ad_group_ad.ad.responsive_search_ad.path2, ad_group_ad.ad.responsive_display_ad.headlines, ad_group_ad.ad.responsive_display_ad.long_headline, ad_group_ad.ad.responsive_display_ad.descriptions, ad_group_ad.ad.responsive_display_ad.business_name, ad_group_ad.status, ad_group_ad.ad_strength, ad_group_ad.policy_summary.approval_status, ad_group_ad.policy_summary.review_status, ad_group_ad.policy_summary.policy_topic_entries, ad_group.id, ad_group.name, campaign.id, campaign.name, customer.currency_code, {METRICS} FROM ad_group_ad WHERE {} ORDER BY metrics.impressions DESC LIMIT {}",
                    filters([
                        Some("ad_group_ad.status != 'REMOVED'".to_owned()),
                        input.campaign_filter(),
                        input.ad_group_filter(),
                        Some(condition),
                    ]),
                    limit + 1,
                ),
                limit,
            )
            .await?;
            let ads = found
                .rows
                .iter()
                .map(ad_row)
                .collect::<BuiltinResult<Vec<Value>>>()?;
            Ok(Object::new()
                .set("currency", currency_of(&found.rows))
                .text("period", period)
                .set("ads", Value::Array(ads))
                .flag("truncated", found.truncated)
                .into_value())
        },
    )
}

fn keyword_row(row: &GoogleAdsRow) -> BuiltinResult<Value> {
    let criterion = js::resource(row, "adGroupCriterion")?;
    let keyword = js::get(criterion, "keyword");
    let bid = from_micros(js::get(criterion, "cpcBidMicros"))
        .filter(|bid| *bid != 0.0)
        .or_else(|| {
            from_micros(js::get(criterion, "effectiveCpcBidMicros")).filter(|bid| *bid != 0.0)
        });
    Ok(Object::new()
        .text(
            "criterion_id",
            js::string(js::get(criterion, "criterionId")),
        )
        .set("text", js::or_null(js::get(keyword, "text")))
        .set("match_type", js::or_null(js::get(keyword, "matchType")))
        .field("status", js::get(criterion, "status"))
        .set("max_cpc", bid.map_or(Value::Null, js::json_number))
        .set(
            "quality_score",
            js::or_null(js::get(js::get(criterion, "qualityInfo"), "qualityScore")),
        )
        .set(
            "approval",
            js::or_null(js::get(criterion, "approvalStatus")),
        )
        .spread(ad_group_columns(row)?)
        .spread(figures(row.get("metrics")))
        .into_value())
}

fn list_keywords_tool() -> Tool {
    BuiltinTool::new(
        "list_keywords",
        format!(
            "List the keywords of an account, a campaign, or an ad group with their match type, status, bid, quality score, and figures over a period. Negative and removed keywords are left out: get_campaign lists the negative keywords of a campaign. {MONEY}"
        ),
        list_in_campaign_schema("keywords"),
        &LIST_IN_CAMPAIGN_VALIDATOR,
        |input: ListInCampaign, context: Arc<BuiltinToolContext>| async move {
            let limit = input.limit.unwrap_or(DEFAULT_ROWS);
            let customer = customer_of(&context, &input.customer_id)?;
            let Days { condition, period } = period_of(&input.dates)?;
            let found = search_google_ads(
                &context,
                &customer,
                &format!(
                    "SELECT ad_group_criterion.criterion_id, ad_group_criterion.keyword.text, ad_group_criterion.keyword.match_type, ad_group_criterion.status, ad_group_criterion.cpc_bid_micros, ad_group_criterion.effective_cpc_bid_micros, ad_group_criterion.quality_info.quality_score, ad_group_criterion.approval_status, ad_group.id, ad_group.name, campaign.id, campaign.name, customer.currency_code, {METRICS} FROM keyword_view WHERE {} ORDER BY metrics.cost_micros DESC LIMIT {}",
                    filters([
                        Some("ad_group_criterion.status != 'REMOVED'".to_owned()),
                        input.campaign_filter(),
                        input.ad_group_filter(),
                        Some(condition),
                    ]),
                    limit + 1,
                ),
                limit,
            )
            .await?;
            let keywords = found
                .rows
                .iter()
                .map(keyword_row)
                .collect::<BuiltinResult<Vec<Value>>>()?;
            Ok(Object::new()
                .set("currency", currency_of(&found.rows))
                .text("period", period)
                .set("keywords", Value::Array(keywords))
                .flag("truncated", found.truncated)
                .into_value())
        },
    )
}

fn list_search_terms_tool() -> Tool {
    BuiltinTool::new(
        "list_search_terms",
        format!(
            "List what people actually searched for before seeing the ads of an account, a campaign, or an ad group, with the figures of each search term over a period. Use it to find keywords to add and negative keywords to exclude. {MONEY}"
        ),
        list_in_campaign_schema("search terms"),
        &LIST_IN_CAMPAIGN_VALIDATOR,
        |input: ListInCampaign, context: Arc<BuiltinToolContext>| async move {
            let limit = input.limit.unwrap_or(DEFAULT_ROWS);
            let customer = customer_of(&context, &input.customer_id)?;
            let Days { condition, period } = period_of(&input.dates)?;
            let found = search_google_ads(
                &context,
                &customer,
                &format!(
                    "SELECT search_term_view.search_term, search_term_view.status, ad_group.id, ad_group.name, campaign.id, campaign.name, customer.currency_code, {METRICS} FROM search_term_view WHERE {} ORDER BY metrics.impressions DESC LIMIT {}",
                    filters([input.campaign_filter(), input.ad_group_filter(), Some(condition)]),
                    limit + 1,
                ),
                limit,
            )
            .await?;
            let search_terms = found
                .rows
                .iter()
                .map(|row| {
                    let view = js::resource(row, "searchTermView")?;
                    Ok(Object::new()
                        .field("search_term", js::get(view, "searchTerm"))
                        // Whether the term is already a keyword, or already excluded.
                        .field("status", js::get(view, "status"))
                        .spread(ad_group_columns(row)?)
                        .spread(figures(row.get("metrics")))
                        .into_value())
                })
                .collect::<BuiltinResult<Vec<Value>>>()?;
            Ok(Object::new()
                .set("currency", currency_of(&found.rows))
                .text("period", period)
                .set("search_terms", Value::Array(search_terms))
                .flag("truncated", found.truncated)
                .into_value())
        },
    )
}

/// What each row of `get_performance` is about: the resource to select from, and the fields that name it.
fn level_of(level: &str) -> (&'static str, &'static str) {
    match level {
        "account" => ("customer", "customer.id, customer.descriptive_name"),
        "ad_group" => (
            "ad_group",
            "ad_group.id, ad_group.name, ad_group.status, campaign.id, campaign.name",
        ),
        _ => ("campaign", "campaign.id, campaign.name, campaign.status"),
    }
}

/// The field a breakdown of `get_performance` selects.
fn segment_of(segment: &str) -> &'static str {
    match segment {
        "date" => "segments.date",
        "week" => "segments.week",
        "month" => "segments.month",
        "device" => "segments.device",
        _ => "segments.ad_network_type",
    }
}

#[derive(Debug, Deserialize)]
struct GetPerformance {
    customer_id: String,
    level: Option<String>,
    segment: Option<String>,
    campaign_id: Option<String>,
    #[serde(flatten)]
    dates: Period,
    limit: Option<usize>,
}

async fn get_performance(
    input: GetPerformance,
    context: &BuiltinToolContext,
) -> BuiltinResult<Value> {
    let level = input.level.as_deref().unwrap_or("campaign");
    let segment = input.segment.as_deref();
    let limit = input.limit.unwrap_or(DEFAULT_ROWS);
    let customer = customer_of(context, &input.customer_id)?;
    if input.campaign_id.is_some() && level == "account" {
        return Err(BuiltinError::tool(
            "campaign_id narrows the campaign and ad_group levels only",
        ));
    }
    let Days { condition, period } = period_of(&input.dates)?;
    let (from, fields) = level_of(level);
    let breakdown = segment.map(segment_of);
    // Days, weeks, and months read in order. The other breakdowns by what costs most.
    let timeline = breakdown.filter(|_| matches!(segment, Some("date" | "week" | "month")));
    let found = search_google_ads(
        context,
        &customer,
        &format!(
            "SELECT {fields}, {}customer.currency_code, {METRICS} FROM {from} WHERE {} ORDER BY {} LIMIT {}",
            breakdown.map(|breakdown| format!("{breakdown}, ")).unwrap_or_default(),
            filters([
                input.campaign_id.as_ref().map(|campaign_id| format!("campaign.id = {campaign_id}")),
                Some(condition),
            ]),
            timeline.unwrap_or("metrics.cost_micros DESC"),
            limit + 1,
        ),
        limit,
    )
    .await?;

    let rows: Vec<Value> = found
        .rows
        .iter()
        .map(|row| {
            let mut listed = Object::new();
            if let Some(segment) = segment {
                let field = if segment == "network" {
                    "adNetworkType"
                } else {
                    segment
                };
                listed = listed.set(segment, js::or_null(js::get(row.get("segments"), field)));
            }
            if level == "account" {
                listed = listed.set(
                    "account",
                    js::or_null(js::get(row.get("customer"), "descriptiveName")),
                );
            }
            if let Some(campaign) = row
                .get("campaign")
                .filter(|campaign| js::truthy(Some(campaign)))
            {
                listed = listed
                    .text("campaign_id", js::string(js::get(campaign, "id")))
                    .field("campaign", js::get(campaign, "name"));
            }
            if let Some(ad_group) = row
                .get("adGroup")
                .filter(|ad_group| js::truthy(Some(ad_group)))
            {
                listed = listed
                    .text("ad_group_id", js::string(js::get(ad_group, "id")))
                    .field("ad_group", js::get(ad_group, "name"));
            }
            listed.spread(figures(row.get("metrics"))).into_value()
        })
        .collect();
    Ok(Object::new()
        .set("currency", currency_of(&found.rows))
        .text("period", period)
        .text("level", level)
        .set("rows", Value::Array(rows))
        .flag("truncated", found.truncated)
        .into_value())
}

fn get_performance_tool() -> Tool {
    BuiltinTool::new(
        "get_performance",
        format!(
            "Get the figures of an account, of its campaigns, or of its ad groups over a period, optionally broken down by day, week, month, device, or network. Rows without any impression are left out of a breakdown. {MONEY}"
        ),
        input_schema(
            properties([
                customer_id_property(),
                json!({
                    "level": {
                        "type": "string",
                        "enum": GOOGLE_ADS_PERFORMANCE_LEVELS,
                        "default": "campaign",
                        "description": "What each row is about.",
                    },
                    "segment": {
                        "type": "string",
                        "enum": GOOGLE_ADS_SEGMENTS,
                        "description": "Break each row down by this. Left out, a row covers the whole period.",
                    },
                    "campaign_id": {
                        "type": "string",
                        "description": "Only this campaign. Not with the account level.",
                    },
                }),
                period_properties(),
                limit_property("rows"),
            ]),
            &["customer_id"],
        ),
        &GET_PERFORMANCE_VALIDATOR,
        |input: GetPerformance, context: Arc<BuiltinToolContext>| async move {
            get_performance(input, &context).await
        },
    )
}

#[derive(Debug, Deserialize)]
struct ListAssets {
    customer_id: String,
    r#type: Option<String>,
    limit: Option<usize>,
}

fn asset_row(row: &GoogleAdsRow) -> BuiltinResult<Value> {
    let asset = js::resource(row, "asset")?;
    let image = js::get(asset, "imageAsset");
    let mut listed = Object::new()
        .text("id", js::string(js::get(asset, "id")))
        .field("type", js::get(asset, "type"))
        .set("name", js::or_null(js::get(asset, "name")));
    if js::truthy(image) {
        let size = js::get(image, "fullSize");
        listed = listed
            .number("width", js::number_or(js::get(size, "widthPixels"), 0.0))
            .number("height", js::number_or(js::get(size, "heightPixels"), 0.0))
            .number("bytes", js::number_or(js::get(image, "fileSize"), 0.0))
            .set("url", js::or_null(js::get(size, "url")));
    } else {
        let text = js::defined(js::get(js::get(asset, "textAsset"), "text"))
            .or_else(|| js::defined(js::get(js::get(asset, "sitelinkAsset"), "linkText")))
            .or_else(|| js::get(js::get(asset, "calloutAsset"), "calloutText"));
        listed = listed.set("text", js::or_null(text));
    }
    if let Some(final_url) = js::first(js::get(asset, "finalUrls")) {
        listed = listed.set("final_url", final_url);
    }
    Ok(listed.into_value())
}

fn list_assets_tool() -> Tool {
    BuiltinTool::new(
        "list_assets",
        "List the assets of an account: images with their size in pixels and a link to view them, and the texts, sitelinks, and callouts ads can show. Image asset IDs are what create_responsive_display_ad and add_campaign_images take.",
        input_schema(
            properties([
                customer_id_property(),
                json!({
                    "type": {
                        "type": "string",
                        "enum": GOOGLE_ADS_ASSET_TYPES,
                        "default": "IMAGE",
                        "description": "Only assets of this type. ALL is every type listed here.",
                    },
                }),
                limit_property("assets"),
            ]),
            &["customer_id"],
        ),
        &LIST_ASSETS_VALIDATOR,
        |input: ListAssets, context: Arc<BuiltinToolContext>| async move {
            let kind = input.r#type.as_deref().unwrap_or("IMAGE");
            let limit = input.limit.unwrap_or(DEFAULT_ROWS);
            let customer = customer_of(&context, &input.customer_id)?;
            let kinds: Vec<String> = if kind == "ALL" {
                GOOGLE_ADS_ASSET_TYPES
                    .iter()
                    .filter(|name| **name != "ALL")
                    .map(|name| format!("'{name}'"))
                    .collect()
            } else {
                vec![format!("'{kind}'")]
            };
            let found = search_google_ads(
                &context,
                &customer,
                &format!(
                    "SELECT asset.id, asset.name, asset.type, asset.final_urls, asset.image_asset.full_size.url, asset.image_asset.full_size.width_pixels, asset.image_asset.full_size.height_pixels, asset.image_asset.file_size, asset.text_asset.text, asset.sitelink_asset.link_text, asset.callout_asset.callout_text FROM asset WHERE asset.type IN ({}) LIMIT {}",
                    kinds.join(", "),
                    limit + 1,
                ),
                limit,
            )
            .await?;
            let assets = found
                .rows
                .iter()
                .map(asset_row)
                .collect::<BuiltinResult<Vec<Value>>>()?;
            Ok(Object::new()
                .set("assets", Value::Array(assets))
                .flag("truncated", found.truncated)
                .into_value())
        },
    )
}

#[derive(Debug, Deserialize)]
struct ListChanges {
    customer_id: String,
    days: Option<i64>,
    limit: Option<usize>,
}

/// `name?.split('/').pop() ?? null`: the ID a resource name ends with.
fn last_part(resource_name: Option<&Value>) -> BuiltinResult<Value> {
    match js::defined(resource_name) {
        None => Ok(Value::Null),
        Some(Value::String(name)) => Ok(Value::from(name.rsplit('/').next().unwrap_or_default())),
        Some(_) => Err(js::unreadable("a resource name that is not text")),
    }
}

fn change_row(row: &GoogleAdsRow) -> BuiltinResult<Value> {
    let change = js::resource(row, "changeEvent")?;
    let fields: Vec<Value> =
        match js::get(change, "changedFields").filter(|fields| js::truthy(Some(fields))) {
            Some(fields) => vine::js::to_string(fields)
                .split(',')
                .map(Value::from)
                .collect(),
            None => Vec::new(),
        };
    Ok(Object::new()
        .field("at", js::get(change, "changeDateTime"))
        .field("resource", js::get(change, "changeResourceType"))
        .field("operation", js::get(change, "resourceChangeOperation"))
        .set("fields", Value::Array(fields))
        .set("by", js::or_null(js::get(change, "userEmail")))
        .set("through", js::or_null(js::get(change, "clientType")))
        .set("campaign_id", last_part(js::get(change, "campaign"))?)
        .set("ad_group_id", last_part(js::get(change, "adGroup"))?)
        // As Google gives them: amounts ending in Micros are millionths of the currency.
        .set("before", js::or_null(js::get(change, "oldResource")))
        .set("after", js::or_null(js::get(change, "newResource")))
        .into_value())
}

fn list_changes_tool() -> Tool {
    BuiltinTool::new(
        "list_changes",
        "List what was changed in an account lately, newest first: when, by whom, from which tool (the Google Ads website, the API, a script, a recommendation), on which campaign or ad group, and which fields. Google keeps 30 days of changes, and lists a change a few minutes after it was made.",
        input_schema(
            properties([
                customer_id_property(),
                json!({
                    "days": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": limits::CHANGE_DAYS,
                        "default": 7,
                        "description": "How many days back to look, today included.",
                    },
                }),
                limit_property("changes"),
            ]),
            &["customer_id"],
        ),
        &LIST_CHANGES_VALIDATOR,
        |input: ListChanges, context: Arc<BuiltinToolContext>| async move {
            let days = input.days.unwrap_or(7);
            let limit = input.limit.unwrap_or(DEFAULT_ROWS);
            let customer = customer_of(&context, &input.customer_id)?;
            // Google refuses a start more than 30 days old. A day ahead covers the
            // accounts whose time zone is already tomorrow.
            let now = Utc::now();
            let from = (now - Duration::days(days - 1)).format("%Y-%m-%d");
            let to = (now + Duration::days(1)).format("%Y-%m-%d");
            let found = search_google_ads(
                &context,
                &customer,
                &format!(
                    "SELECT change_event.change_date_time, change_event.change_resource_type, change_event.resource_change_operation, change_event.changed_fields, change_event.client_type, change_event.user_email, change_event.campaign, change_event.ad_group, change_event.old_resource, change_event.new_resource FROM change_event WHERE change_event.change_date_time >= '{from}' AND change_event.change_date_time <= '{to}' ORDER BY change_event.change_date_time DESC LIMIT {}",
                    limit + 1,
                ),
                limit,
            )
            .await?;
            let changes = found
                .rows
                .iter()
                .map(change_row)
                .collect::<BuiltinResult<Vec<Value>>>()?;
            Ok(Object::new()
                .set("changes", Value::Array(changes))
                .flag("truncated", found.truncated)
                .into_value())
        },
    )
}

#[derive(Debug, Deserialize)]
struct SearchLocations {
    names: Vec<String>,
    country_code: Option<String>,
    locale: Option<String>,
}

/// The list an answer holds under `key`, or none when it has no such key:
/// `const { [key]: list = [] } = answer`.
fn listed<'a>(answer: &'a Value, key: &str) -> BuiltinResult<&'a [Value]> {
    match (answer, js::get(answer, key)) {
        (Value::Null, _) => Err(js::unreadable("nothing")),
        (_, None) => Ok(&[]),
        (_, Some(Value::Array(items))) => Ok(items),
        (_, Some(_)) => Err(js::unreadable(&format!("{key} that are not a list"))),
    }
}

fn search_locations_tool() -> Tool {
    BuiltinTool::new(
        "search_locations",
        "Find the locations Google Ads can target by name: countries, regions, cities, postal codes. Returns the location IDs that create_campaign and update_campaign_targeting take, with how many people each reaches.",
        input_schema(
            json!({
                "names": {
                    "type": "array",
                    "items": { "type": "string", "maxLength": 80 },
                    "minItems": 1,
                    "maxItems": limits::LOCATION_NAMES,
                    "description": "Place names to look for, such as [\"France\", \"Lyon\"].",
                },
                "country_code": {
                    "type": "string",
                    "description": "Only places in this country, as a two-letter code such as FR.",
                },
                "locale": {
                    "type": "string",
                    "default": "en",
                    "description": "Language the names are written in, as a two-letter code such as fr.",
                },
            }),
            &["names"],
        ),
        &SEARCH_LOCATIONS_VALIDATOR,
        |input: SearchLocations, context: Arc<BuiltinToolContext>| async move {
            let mut body = Map::new();
            body.insert(
                "locale".to_owned(),
                Value::from(input.locale.as_deref().unwrap_or("en").to_lowercase()),
            );
            if let Some(country_code) = &input.country_code {
                body.insert(
                    "countryCode".to_owned(),
                    Value::from(country_code.to_uppercase()),
                );
            }
            body.insert("locationNames".to_owned(), json!({ "names": input.names }));
            let answer = google_ads_request(
                &context,
                "/geoTargetConstants:suggest",
                GoogleAdsRequest::post(Value::Object(body)),
            )
            .await?;

            let locations: Vec<Value> = listed(&answer, "geoTargetConstantSuggestions")?
                .iter()
                .map(|suggestion| {
                    if suggestion.is_null() {
                        return Err(js::unreadable("a suggestion that is missing"));
                    }
                    let place = js::get(suggestion, "geoTargetConstant");
                    let name = js::defined(js::get(place, "canonicalName"))
                        .or_else(|| js::get(place, "name"));
                    let reach = js::get(suggestion, "reach");
                    Ok(Object::new()
                        .text("id", js::string(js::get(place, "id")))
                        .set("name", js::or_null(name))
                        .set("type", js::or_null(js::get(place, "targetType")))
                        .set("country_code", js::or_null(js::get(place, "countryCode")))
                        .set("status", js::or_null(js::get(place, "status")))
                        .set(
                            "reach",
                            reach.map_or(Value::Null, |reach| {
                                js::json_number(vine::js::to_number(Some(reach)))
                            }),
                        )
                        .set("searched", js::or_null(js::get(suggestion, "searchTerm")))
                        .into_value())
                })
                .collect::<BuiltinResult<_>>()?;
            Ok(Object::new()
                .set("locations", Value::Array(locations))
                .into_value())
        },
    )
}

#[derive(Debug, Deserialize)]
struct KeywordIdeas {
    customer_id: String,
    keywords: Option<Vec<String>>,
    url: Option<String>,
    location_ids: Option<Vec<String>>,
    language: Option<String>,
    limit: Option<usize>,
}

async fn generate_keyword_ideas(
    input: KeywordIdeas,
    context: &BuiltinToolContext,
) -> BuiltinResult<Value> {
    let limit = input.limit.unwrap_or(DEFAULT_ROWS);
    let customer = customer_of(context, &input.customer_id)?;
    let spoken = languages_by_code(
        context,
        &customer,
        &[language_code(input.language.as_deref().unwrap_or("en"))],
    )
    .await?
    .into_iter()
    .next()
    .ok_or_else(|| js::unreadable("no language"))?;

    let locations: Vec<String> = input
        .location_ids
        .iter()
        .flatten()
        .map(|id| format!("geoTargetConstants/{id}"))
        .collect();
    let mut body = Map::new();
    body.insert(
        "language".to_owned(),
        Value::from(format!("languageConstants/{}", spoken.id)),
    );
    body.insert("geoTargetConstants".to_owned(), json!(locations));
    body.insert(
        "keywordPlanNetwork".to_owned(),
        Value::from("GOOGLE_SEARCH"),
    );
    body.insert("pageSize".to_owned(), json!(limit));
    let url = input.url.as_deref().filter(|url| !url.is_empty());
    match (&input.keywords, url) {
        (Some(keywords), Some(url)) => {
            body.insert(
                "keywordAndUrlSeed".to_owned(),
                json!({ "keywords": keywords, "url": url }),
            );
        }
        (Some(keywords), None) => {
            body.insert("keywordSeed".to_owned(), json!({ "keywords": keywords }));
        }
        (None, url) => {
            // Without an address, `JSON.stringify` writes an empty seed.
            let seed = url.map_or_else(|| json!({}), |url| json!({ "url": url }));
            body.insert("urlSeed".to_owned(), seed);
        }
    }
    let answer = google_ads_request(
        context,
        &format!("/customers/{customer}:generateKeywordIdeas"),
        GoogleAdsRequest::post(Value::Object(body)),
    )
    .await?;

    let ideas: Vec<Value> = listed(&answer, "results")?
        .iter()
        .take(limit)
        .map(|idea| {
            if idea.is_null() {
                return Err(js::unreadable("an idea that is missing"));
            }
            let metrics = js::get(idea, "keywordIdeaMetrics");
            let bid = |micros: &str| {
                from_micros(js::get(metrics, micros)).map_or(Value::Null, js::json_number)
            };
            Ok(Object::new()
                .field("text", js::get(idea, "text"))
                .number(
                    "average_monthly_searches",
                    js::number_or(js::get(metrics, "avgMonthlySearches"), 0.0),
                )
                .set("competition", js::or_null(js::get(metrics, "competition")))
                .set(
                    "competition_index",
                    js::get(metrics, "competitionIndex").map_or(Value::Null, |index| {
                        js::json_number(vine::js::to_number(Some(index)))
                    }),
                )
                .set("low_top_of_page_bid", bid("lowTopOfPageBidMicros"))
                .set("high_top_of_page_bid", bid("highTopOfPageBidMicros"))
                .into_value())
        })
        .collect::<BuiltinResult<_>>()?;
    Ok(Object::new()
        .text("language", spoken.name)
        .set("ideas", Value::Array(ideas))
        .into_value())
}

fn generate_keyword_ideas_tool() -> Tool {
    BuiltinTool::new(
        "generate_keyword_ideas",
        format!(
            "Get keyword ideas from Google's Keyword Planner for seed keywords, a web page, or both: each idea with its average monthly searches, competition, and the range of bids that reach the top of the page. Google only answers this for Cloud projects with Basic or Standard access. {MONEY}"
        ),
        input_schema(
            properties([
                customer_id_property(),
                json!({
                    "keywords": {
                        "type": "array",
                        "items": { "type": "string", "maxLength": limits::KEYWORD_LENGTH },
                        "minItems": 1,
                        "maxItems": limits::SEED_KEYWORDS,
                        "description": "Words or phrases to start from.",
                    },
                    "url": { "type": "string", "description": "A web page to draw ideas from." },
                    "location_ids": {
                        "type": "array",
                        "items": { "type": "string" },
                        "minItems": 1,
                        "maxItems": limits::LOCATIONS,
                        "description": "Only searches from these locations, by the IDs search_locations returns.",
                    },
                    "language": {
                        "type": "string",
                        "default": "en",
                        "description": "Language of the searches, as a code such as en, fr, or pt_BR.",
                    },
                }),
                limit_property("ideas"),
            ]),
            &["customer_id"],
        ),
        &KEYWORD_IDEAS_VALIDATOR,
        |input: KeywordIdeas, context: Arc<BuiltinToolContext>| async move {
            generate_keyword_ideas(input, &context).await
        },
    )
}

#[derive(Debug, Deserialize)]
struct RunQuery {
    customer_id: String,
    query: String,
    limit: Option<usize>,
}

fn run_query_tool() -> Tool {
    BuiltinTool::new(
        "run_query",
        "Run a read-only Google Ads Query Language (GAQL) query for what the other tools do not cover, such as conversion actions, recommendations, audiences, or figures by hour. Rows come back as Google returns them: field names in camelCase, identifiers and counts as text, and amounts ending in Micros in millionths of the account currency (2500000 is 2.50). A query cannot change anything.",
        input_schema(
            properties([
                customer_id_property(),
                json!({
                    "query": {
                        "type": "string",
                        "maxLength": limits::QUERY_LENGTH,
                        "description": "A GAQL query, such as: SELECT campaign.name, metrics.clicks FROM campaign WHERE segments.date DURING LAST_7_DAYS AND campaign.status = 'ENABLED' ORDER BY metrics.clicks DESC",
                    },
                }),
                limit_property("rows"),
            ]),
            &["customer_id", "query"],
        ),
        &RUN_QUERY_VALIDATOR,
        |input: RunQuery, context: Arc<BuiltinToolContext>| async move {
            let limit = input.limit.unwrap_or(DEFAULT_ROWS);
            let customer = customer_of(&context, &input.customer_id)?;
            let found = search_google_ads(&context, &customer, &input.query, limit).await?;
            let rows: Vec<Value> = found.rows.into_iter().map(Value::Object).collect();
            Ok(Object::new()
                .set("rows", Value::Array(rows))
                .flag("truncated", found.truncated)
                .into_value())
        },
    )
}

/// The read tools, in the order agents list them. (`googleAdsReadTools`)
pub fn google_ads_read_tools() -> Vec<BuiltinTool<BuiltinToolContext>> {
    vec![
        list_accounts_tool(),
        list_campaigns_tool(),
        get_campaign_tool(),
        list_ad_groups_tool(),
        list_ads_tool(),
        list_keywords_tool(),
        list_search_terms_tool(),
        get_performance_tool(),
        list_assets_tool(),
        list_changes_tool(),
        search_locations_tool(),
        generate_keyword_ideas_tool(),
        run_query_tool(),
    ]
}
