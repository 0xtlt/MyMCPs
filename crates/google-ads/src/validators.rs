//! Vine schemas for the built-in Google Ads MCP: the arguments of its tools,
//! what its upload links refer to, and the JSON Google answers a refused
//! request with. A schema lists the arguments in the order they are checked:
//! of several wrong ones, the agent is told about the first.
//!
//! The port of `app/validators/builtin_google_ads.ts`. Its first part holds
//! what the schemas are made of, for the read tools and the write tools
//! alike, then the schemas of the read tools and the ones for Google's
//! answers. The schemas of the write tools are at the end of the file.

use std::sync::{Arc, LazyLock};

use chrono::NaiveDate;
use mymcps_builtin::arguments::{
    TOOL_VINE, VineArgument, argument, blank_as_missing, boolean, choice, integer, is_blank, line,
    media_type, number, pattern, pattern_with, text, uploaded_file_name,
};
use mymcps_builtin::upload_store::is_builtin_upload_id;
use mymcps_vine as vine;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use vine::{JsRegex, Rule, Schema, Validator};

use crate::format::language_code;

/// The bounds the tools advertise, and the schemas below enforce.
/// (`GOOGLE_ADS_LIMITS`)
pub mod limits {
    pub const ROWS: i64 = 1000;
    pub const QUERY_LENGTH: usize = 10_000;
    pub const NAME_LENGTH: usize = 255;
    /// A daily budget or a bid above these is a typing mistake, in any currency Google bills in.
    pub const DAILY_BUDGET: f64 = 1_000_000.0;
    pub const BID: f64 = 10_000.0;
    pub const TARGET_ROAS: f64 = 1000.0;
    pub const KEYWORDS: usize = 100;
    pub const KEYWORD_LENGTH: usize = 80;
    pub const HEADLINES: usize = 15;
    pub const HEADLINE_LENGTH: usize = 30;
    pub const DESCRIPTIONS: usize = 4;
    pub const DESCRIPTION_LENGTH: usize = 90;
    pub const PATH_LENGTH: usize = 15;
    pub const DISPLAY_TEXTS: usize = 5;
    pub const LONG_HEADLINE_LENGTH: usize = 90;
    pub const BUSINESS_NAME_LENGTH: usize = 25;
    pub const IMAGES: usize = 15;
    pub const LOGOS: usize = 5;
    pub const LOCATIONS: usize = 100;
    pub const LANGUAGES: usize = 30;
    pub const LOCATION_NAMES: usize = 25;
    pub const SEED_KEYWORDS: usize = 20;
    pub const URL_LENGTH: usize = 2048;
    pub const CRITERIA: usize = 100;
    pub const CHANGE_DAYS: i64 = 30;
    pub const LINK_MINUTES: i64 = 60;
    pub const FILENAME_LENGTH: usize = 255;
    /// Google Ads takes images of up to 5120 KB.
    pub const IMAGE_BYTES: u64 = 5_242_880;
}

pub const GOOGLE_ADS_DATE_RANGES: [&str; 7] = [
    "TODAY",
    "YESTERDAY",
    "LAST_7_DAYS",
    "LAST_14_DAYS",
    "LAST_30_DAYS",
    "THIS_MONTH",
    "LAST_MONTH",
];

pub const GOOGLE_ADS_STATUSES: [&str; 3] = ["ENABLED", "PAUSED", "REMOVED"];
pub const GOOGLE_ADS_MATCH_TYPES: [&str; 3] = ["EXACT", "PHRASE", "BROAD"];
pub const GOOGLE_ADS_CHANNELS: [&str; 2] = ["SEARCH", "DISPLAY"];
pub const GOOGLE_ADS_BIDDING_STRATEGIES: [&str; 4] = [
    "MAXIMIZE_CLICKS",
    "MAXIMIZE_CONVERSIONS",
    "MAXIMIZE_CONVERSION_VALUE",
    "MANUAL_CPC",
];
pub const GOOGLE_ADS_PERFORMANCE_LEVELS: [&str; 3] = ["account", "campaign", "ad_group"];
pub const GOOGLE_ADS_SEGMENTS: [&str; 5] = ["date", "week", "month", "device", "network"];
pub const GOOGLE_ADS_ASSET_TYPES: [&str; 5] = ["IMAGE", "TEXT", "SITELINK", "CALLOUT", "ALL"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum GoogleAdsMatchType {
    Exact,
    Phrase,
    Broad,
}

impl GoogleAdsMatchType {
    /// As Google Ads names it: `EXACT`, `PHRASE` or `BROAD`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "EXACT",
            Self::Phrase => "PHRASE",
            Self::Broad => "BROAD",
        }
    }
}

/// A keyword as [`keywords`] hands it to a tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleAdsKeyword {
    pub text: String,
    pub match_type: GoogleAdsMatchType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cpc: Option<f64>,
}

fn js_regex(expression: &str, flags: &str) -> JsRegex {
    vine::js::regex(expression, flags).expect("static regex")
}

/// With or without its dashes, as Google Ads shows it.
pub fn customer_id() -> VineArgument {
    pattern(
        r"^\d{3}-?\d{3}-?\d{4}$",
        "a Google Ads account ID such as 123-456-7890",
    )
}

/// Identifiers are 64-bit numbers, which JSON numbers cannot all hold: they
/// are text here, and a number is taken as its digits.
pub fn id(what: &str) -> VineArgument {
    pattern(r"^\d{1,19}$", &format!("the numeric ID of {what}"))
}

/// An amount in the currency of the account.
pub fn amount(max: f64) -> VineArgument {
    number(0.01..=max)
}

fn calendar_date_rule() -> Rule {
    static WRITTEN: LazyLock<JsRegex> = LazyLock::new(|| js_regex(r"^\d{4}-\d{2}-\d{2}$", ""));
    vine::rule(|value, field| {
        let written = value.as_str().map(vine::js::trim).unwrap_or_default();
        let exists = WRITTEN.test(written) && {
            let mut parts = written
                .split('-')
                .map(|part| part.parse::<u32>().unwrap_or(0));
            let (year, month, day) = (parts.next(), parts.next(), parts.next());
            // JavaScript takes a year below 100 for one of the twentieth
            // century, which is then not the year that was written.
            year.zip(month)
                .zip(day)
                .is_some_and(|((year, month), day)| {
                    year >= 100
                        && i32::try_from(year)
                            .is_ok_and(|year| NaiveDate::from_ymd_opt(year, month, day).is_some())
                })
        };
        if !exists {
            field.report(
                "{{ field }} must be a date such as 2026-01-31",
                "calendarDate",
            );
            return;
        }
        field.mutate(written);
    })
    .json_schema(|schema| {
        schema.insert("type".to_owned(), json!("string"));
    })
}

/// A day in the account's time zone, as `YYYY-MM-DD`.
pub fn calendar_date() -> VineArgument {
    argument(calendar_date_rule()).parse(blank_as_missing)
}

/// Reads one item of a list: the item as the tool takes it, or `None` when
/// it is not one.
pub type ListItem = dyn Fn(&Value) -> Option<Value> + Send + Sync;

/// What a list is made of, and how it says that it is wrong.
#[derive(Clone)]
pub struct ListOptions {
    pub min: usize,
    pub max: usize,
    pub sentence: String,
    pub item: Arc<ListItem>,
    /// How one item reads in a JSON Schema.
    pub schema: Value,
}

/// One sentence whatever is wrong with the list, so one rule for all of it.
fn list_rule(options: ListOptions) -> Rule {
    let ListOptions {
        min,
        max,
        sentence,
        item,
        schema,
    } = options;
    vine::rule(move |value, field| {
        let items: Option<Vec<Value>> = value
            .as_array()
            .and_then(|items| items.iter().map(|one| item(one)).collect());
        match items.filter(|items| (min..=max).contains(&items.len())) {
            Some(items) => field.mutate(items),
            None => field.report(&sentence, "list"),
        }
    })
    .json_schema(move |described| {
        described.insert("type".to_owned(), json!("array"));
        described.insert("items".to_owned(), schema.clone());
        described.insert("minItems".to_owned(), json!(min));
        described.insert("maxItems".to_owned(), json!(max));
    })
}

/// A list checked as a whole. A single value is a list of one.
pub fn list(options: ListOptions) -> VineArgument {
    argument(list_rule(options)).parse(|value, _| {
        match value.filter(|value| !is_blank(Some(value)))? {
            Value::Array(items) => Some(Value::Array(items)),
            single => Some(Value::Array(vec![single])),
        }
    })
}

/// Text without the spaces around it, when it is one line of at most `max`
/// characters.
fn trimmed(value: Option<&Value>, max: usize) -> Option<&str> {
    let written = value.and_then(Value::as_str).map(vine::js::trim)?;
    // `/\p{Cc}/u`
    let fits = !written.is_empty()
        && vine::js::utf16_len(written) <= max
        && !written.chars().any(char::is_control);
    fits.then_some(written)
}

/// The items a list of [`texts`] may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextBounds {
    pub min: usize,
    pub max: usize,
    pub length: usize,
}

/// A list of short texts, such as headlines.
pub fn texts(what: &str, bounds: TextBounds) -> VineArgument {
    let TextBounds { min, max, length } = bounds;
    list(ListOptions {
        min,
        max,
        sentence: format!(
            "{{{{ field }}}} must be a list of {min} to {max} {what} of at most {length} characters each"
        ),
        item: Arc::new(move |value| trimmed(Some(value), length).map(Value::from)),
        schema: json!({ "type": "string", "maxLength": length }),
    })
}

/// A list of identifiers, each kept as text.
pub fn ids(what: &str, max: usize) -> VineArgument {
    static IDENTIFIER: LazyLock<JsRegex> = LazyLock::new(|| js_regex(r"^\d{1,19}$", ""));
    list(ListOptions {
        min: 1,
        max,
        sentence: format!("{{{{ field }}}} must be a list of 1 to {max} numeric IDs of {what}"),
        item: Arc::new(|value| {
            let written = match value {
                Value::Number(_) => vine::js::to_string(value),
                Value::String(text) => text.clone(),
                _ => return None,
            };
            let written = vine::js::trim(&written);
            IDENTIFIER.test(written).then(|| Value::from(written))
        }),
        schema: json!({ "type": "string" }),
    })
}

/// As Google Ads names languages: `en`, `fr`, and with a region `pt_BR` or `zh_CN`.
pub fn languages() -> VineArgument {
    static CODE: LazyLock<JsRegex> = LazyLock::new(|| js_regex(r"^[a-z]{2}([_-][a-z]{2})?$", "i"));
    let max = limits::LANGUAGES;
    list(ListOptions {
        min: 1,
        max,
        sentence: format!(
            "{{{{ field }}}} must be a list of 1 to {max} language codes such as en, fr, or pt_BR"
        ),
        item: Arc::new(|value| {
            let written = value.as_str()?;
            CODE.test(vine::js::trim(written))
                .then(|| Value::from(language_code(written)))
        }),
        schema: json!({ "type": "string" }),
    })
}

fn keyword(value: &Value) -> Option<Value> {
    let written = value.as_object()?;
    let text = trimmed(written.get("text"), limits::KEYWORD_LENGTH)?;
    let match_type = written.get("match_type").and_then(Value::as_str)?;
    if !GOOGLE_ADS_MATCH_TYPES.contains(&match_type) {
        return None;
    }
    let mut keyword = Map::new();
    keyword.insert("text".to_owned(), Value::from(text));
    keyword.insert("matchType".to_owned(), Value::from(match_type));

    let max_cpc = written.get("max_cpc");
    if !is_blank(max_cpc) {
        let bid = match max_cpc? {
            Value::String(quoted) => vine::js::string_to_number(quoted),
            other => vine::js::as_f64(other)?,
        };
        if !(0.01..=limits::BID).contains(&bid) {
            return None;
        }
        keyword.insert("maxCpc".to_owned(), vine::js::number(bid));
    }
    Some(Value::Object(keyword))
}

/// A list of keywords, each with its match type and an optional bid. The
/// tool receives [`GoogleAdsKeyword`]s.
pub fn keywords() -> VineArgument {
    let max = limits::KEYWORDS;
    list(ListOptions {
        min: 1,
        max,
        sentence: format!(
            "{{{{ field }}}} must be a list of 1 to {max} keywords such as {{\"text\": \"running shoes\", \"match_type\": \"PHRASE\"}}, with a text of at most {} characters, a match_type among {}, and an optional max_cpc between 0.01 and {}",
            limits::KEYWORD_LENGTH,
            GOOGLE_ADS_MATCH_TYPES.join(", "),
            limits::BID,
        ),
        item: Arc::new(keyword),
        schema: json!({ "type": "object" }),
    })
}

fn web_address_rule() -> Rule {
    vine::rule(|value, field| {
        let url = value
            .as_str()
            .and_then(|written| url::Url::parse(written).ok());
        if !url.is_some_and(|url| matches!(url.scheme(), "https" | "http")) {
            field.report(
                "{{ field }} must be a web address such as https://example.com/page",
                "webAddress",
            );
        }
    })
}

/// Where an ad sends people.
pub fn web_address() -> VineArgument {
    line(limits::URL_LENGTH).use_rule(web_address_rule())
}

/// One part of the path an ad shows after its domain: no space and no slash.
pub fn display_path() -> VineArgument {
    let length = limits::PATH_LENGTH;
    pattern_with(
        js_regex(&format!(r"^[^\s/]{{1,{length}}}$"), "u"),
        &format!("a display path of at most {length} characters, without spaces or slashes"),
    )
}

fn select_rule() -> Rule {
    static SELECT: LazyLock<JsRegex> = LazyLock::new(|| js_regex(r"^select\s", "i"));
    vine::rule(|value, field| {
        let query = value.as_str().map(vine::js::trim).unwrap_or_default();
        if !SELECT.test(query) {
            field.report(
                "{{ field }} must be a Google Ads Query Language query starting with SELECT",
                "select",
            );
            return;
        }
        field.mutate(query);
    })
}

/// For an argument that may only be left out when `other` is given.
pub fn required_unless_rule(other: &str, sentence: &str) -> Rule {
    let (other, sentence) = (other.to_owned(), sentence.to_owned());
    vine::rule(move |value, field| {
        let blank = !field.is_defined() || value.as_str() == Some("");
        if blank && is_blank(field.parent_get(&other)) {
            field.report(&sentence, "requiredUnless");
        }
    })
    .implicit()
}

/// The days the figures cover: a named range, or a first and a last day.
pub fn period() -> Vec<(String, Schema)> {
    vine::properties! {
        "date_range" => choice(GOOGLE_ADS_DATE_RANGES).optional(),
        "start_date" => calendar_date().optional(),
        "end_date" => calendar_date().optional(),
    }
}

/// How many rows a tool may return.
pub fn rows() -> VineArgument {
    integer(1..=limits::ROWS).optional()
}

/// The one target each bidding strategy takes.
pub fn bidding() -> Vec<(String, Schema)> {
    vine::properties! {
        "max_cpc" => amount(limits::BID).optional(),
        "target_cpa" => amount(limits::BID).optional(),
        "target_roas" => number(0.01..=limits::TARGET_ROAS).optional(),
    }
}

pub static LIST_CAMPAIGNS_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "status" => choice(["ENABLED", "PAUSED", "ALL"]).optional(),
        ..period(),
        "limit" => rows(),
    })
});

pub static CAMPAIGN_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "campaign_id" => id("a campaign"),
    })
});

/// What a list is narrowed to, and the days its figures cover.
pub static LIST_IN_CAMPAIGN_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "campaign_id" => id("a campaign").optional(),
        "ad_group_id" => id("an ad group").optional(),
        ..period(),
        "limit" => rows(),
    })
});

pub static LIST_AD_GROUPS_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "campaign_id" => id("a campaign").optional(),
        ..period(),
        "limit" => rows(),
    })
});

pub static GET_PERFORMANCE_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "level" => choice(GOOGLE_ADS_PERFORMANCE_LEVELS).optional(),
        "segment" => choice(GOOGLE_ADS_SEGMENTS).optional(),
        "campaign_id" => id("a campaign").optional(),
        ..period(),
        "limit" => rows(),
    })
});

pub static LIST_ASSETS_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "type" => choice(GOOGLE_ADS_ASSET_TYPES).optional(),
        "limit" => rows(),
    })
});

pub static LIST_CHANGES_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "days" => integer(1..=limits::CHANGE_DAYS).optional(),
        "limit" => rows(),
    })
});

pub static SEARCH_LOCATIONS_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "names" => texts("place names", TextBounds { min: 1, max: limits::LOCATION_NAMES, length: 80 }),
        "country_code" => pattern(r"^[A-Za-z]{2}$", "a two-letter country code such as FR").optional(),
        "locale" => pattern(r"^[A-Za-z]{2}$", "a two-letter language code such as fr").optional(),
    })
});

pub static KEYWORD_IDEAS_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "keywords" => texts(
            "seed keywords",
            TextBounds { min: 1, max: limits::SEED_KEYWORDS, length: limits::KEYWORD_LENGTH },
        )
        .optional(),
        "url" => web_address().optional().use_rule(required_unless_rule("keywords", "Set keywords, url, or both")),
        "location_ids" => ids("locations", limits::LOCATIONS).optional(),
        "language" => pattern(r"^[A-Za-z]{2}([_-][A-Za-z]{2})?$", "a language code such as en, fr, or pt_BR").optional(),
        "limit" => rows(),
    })
});

pub static RUN_QUERY_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "query" => text(limits::QUERY_LENGTH).use_rule(select_rule()),
        "limit" => rows(),
    })
});

/// The name of an image sent to an upload link.
pub fn image_file_name() -> VineArgument {
    uploaded_file_name(limits::FILENAME_LENGTH)
}

fn upload_id_rule() -> Rule {
    vine::rule(|value, field| {
        let written = value
            .as_str()
            .map(|text| vine::js::trim(text).to_lowercase())
            .unwrap_or_default();
        if !is_builtin_upload_id(&written) {
            field.report(
                "{{ field }} must be an upload ID, as returned by create_image_upload_link",
                "uploadId",
            );
            return;
        }
        field.mutate(written);
    })
    .json_schema(|schema| {
        schema.insert("type".to_owned(), json!("string"));
    })
}

/// What names an uploaded file: the upload ID create_image_upload_link returned.
pub fn upload_id() -> VineArgument {
    argument(upload_id_rule()).parse(blank_as_missing)
}

/// One page of a report. Each row holds one object for each resource the query
/// selects from, whose fields are the ones that were selected.
pub static GOOGLE_ADS_ROWS_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "results" => vine::array(vine::record(vine::any())).optional(),
        "nextPageToken" => vine::string().optional(),
    })
});

/// What a mutate answers: for each operation, one key naming what it changed.
pub static GOOGLE_ADS_MUTATION_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "mutateOperationResponses" => vine::array(vine::record(vine::object! {
            "resourceName" => vine::string().optional(),
        }))
        .optional(),
    })
});

/// The body of a response Google sent with an error status. Each detail may
/// carry the failure of the Google Ads API itself, with one entry per mistake
/// in the request.
pub static GOOGLE_ADS_FAILURE_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "error" => vine::object! {
            "code" => vine::number().optional(),
            "message" => vine::string().optional(),
            "status" => vine::string().optional(),
            "details" => vine::array(vine::object! {
                "errors" => vine::array(vine::object! {
                    // One key naming the kind of error, such as `{ "authorizationError": "USER_PERMISSION_DENIED" }`.
                    "errorCode" => vine::record(vine::string()).optional(),
                    "message" => vine::string().optional(),
                    "location" => vine::object! {
                        "fieldPathElements" => vine::array(vine::object! {
                            "fieldName" => vine::string().optional(),
                            "index" => vine::number().optional(),
                        })
                        .optional(),
                    }
                    .optional(),
                })
                .optional(),
                "requestId" => vine::string().optional(),
            })
            .optional(),
        },
    })
});

pub static CREATE_CAMPAIGN_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "name" => line(limits::NAME_LENGTH),
        "channel" => choice(GOOGLE_ADS_CHANNELS),
        "daily_budget" => amount(limits::DAILY_BUDGET),
        "bidding_strategy" => choice(GOOGLE_ADS_BIDDING_STRATEGIES),
        ..bidding(),
        "location_ids" => ids("locations", limits::LOCATIONS).optional(),
        "languages" => languages().optional(),
        "start_date" => calendar_date().optional(),
        "end_date" => calendar_date().optional(),
        "status" => choice(["PAUSED", "ENABLED"]).optional(),
        "search_partners" => boolean().optional(),
        "display_network" => boolean().optional(),
        "eu_political_ads" => boolean().optional(),
    })
});

pub static UPDATE_CAMPAIGN_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "campaign_id" => id("a campaign"),
        "name" => line(limits::NAME_LENGTH).optional(),
        "bidding_strategy" => choice(GOOGLE_ADS_BIDDING_STRATEGIES).optional(),
        ..bidding(),
        "start_date" => calendar_date().optional(),
        "end_date" => calendar_date().optional(),
        "search_partners" => boolean().optional(),
        "display_network" => boolean().optional(),
    })
});

pub static SET_CAMPAIGN_STATUS_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "campaign_id" => id("a campaign"),
        "status" => choice(GOOGLE_ADS_STATUSES),
    })
});

pub static UPDATE_CAMPAIGN_BUDGET_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "campaign_id" => id("a campaign"),
        "daily_budget" => amount(limits::DAILY_BUDGET),
    })
});

pub static UPDATE_CAMPAIGN_TARGETING_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "campaign_id" => id("a campaign"),
        "add_location_ids" => ids("locations", limits::LOCATIONS).optional(),
        "remove_location_ids" => ids("locations", limits::LOCATIONS).optional(),
        "add_languages" => languages().optional(),
        "remove_languages" => languages().optional(),
        "add_negative_keywords" => keywords().optional(),
        "remove_criterion_ids" => ids("campaign criteria", limits::CRITERIA).optional(),
    })
});

pub static CREATE_AD_GROUP_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "campaign_id" => id("a campaign"),
        "name" => line(limits::NAME_LENGTH),
        "max_cpc" => amount(limits::BID).optional(),
        "status" => choice(["ENABLED", "PAUSED"]).optional(),
    })
});

pub static UPDATE_AD_GROUP_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "ad_group_id" => id("an ad group"),
        "name" => line(limits::NAME_LENGTH).optional(),
        "max_cpc" => amount(limits::BID).optional(),
        "status" => choice(GOOGLE_ADS_STATUSES).optional(),
    })
});

pub static ADD_KEYWORDS_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "ad_group_id" => id("an ad group"),
        "keywords" => keywords(),
        "negative" => boolean().optional(),
    })
});

pub static UPDATE_KEYWORD_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "ad_group_id" => id("an ad group"),
        "criterion_id" => id("a keyword"),
        "max_cpc" => amount(limits::BID).optional(),
        "status" => choice(GOOGLE_ADS_STATUSES)
            .optional()
            .use_rule(required_unless_rule("max_cpc", "Set status, max_cpc, or both")),
    })
});

pub static CREATE_SEARCH_AD_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "ad_group_id" => id("an ad group"),
        "headlines" => texts(
            "headlines",
            TextBounds { min: 3, max: limits::HEADLINES, length: limits::HEADLINE_LENGTH },
        ),
        "descriptions" => texts(
            "descriptions",
            TextBounds { min: 2, max: limits::DESCRIPTIONS, length: limits::DESCRIPTION_LENGTH },
        ),
        "final_url" => web_address(),
        "path1" => display_path().optional(),
        "path2" => display_path().optional(),
        "status" => choice(["ENABLED", "PAUSED"]).optional(),
    })
});

pub static CREATE_DISPLAY_AD_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "ad_group_id" => id("an ad group"),
        "marketing_image_asset_ids" => ids("landscape image assets", limits::IMAGES),
        "square_marketing_image_asset_ids" => ids("square image assets", limits::IMAGES),
        "square_logo_asset_ids" => ids("square logo assets", limits::LOGOS).optional(),
        "wide_logo_asset_ids" => ids("wide logo assets", limits::LOGOS).optional(),
        "headlines" => texts(
            "headlines",
            TextBounds { min: 1, max: limits::DISPLAY_TEXTS, length: limits::HEADLINE_LENGTH },
        ),
        "long_headline" => line(limits::LONG_HEADLINE_LENGTH),
        "descriptions" => texts(
            "descriptions",
            TextBounds { min: 1, max: limits::DISPLAY_TEXTS, length: limits::DESCRIPTION_LENGTH },
        ),
        "business_name" => line(limits::BUSINESS_NAME_LENGTH),
        "final_url" => web_address(),
        "status" => choice(["ENABLED", "PAUSED"]).optional(),
    })
});

pub static SET_AD_STATUS_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "ad_group_id" => id("an ad group"),
        "ad_id" => id("an ad"),
        "status" => choice(GOOGLE_ADS_STATUSES),
    })
});

pub static CREATE_IMAGE_UPLOAD_LINK_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "filename" => image_file_name(),
        "content_type" => media_type().optional(),
        "expires_in_minutes" => integer(1..=limits::LINK_MINUTES).optional(),
    })
});

/// What create_image_upload_link puts in a link, and gets back when a file is sent to it.
pub static IMAGE_UPLOAD_REFERENCE_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "upload" => upload_id(),
        "filename" => image_file_name(),
        "content_type" => media_type().optional(),
    })
});

pub static CREATE_IMAGE_ASSET_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "upload_id" => upload_id(),
        "name" => line(limits::NAME_LENGTH),
    })
});

pub static ADD_CAMPAIGN_IMAGES_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "customer_id" => customer_id(),
        "campaign_id" => id("a campaign"),
        "asset_ids" => ids("image assets", limits::IMAGES),
    })
});
