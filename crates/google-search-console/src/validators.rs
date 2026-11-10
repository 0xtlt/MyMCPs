//! Vine schemas for the built-in Google Search Console MCP: the arguments of
//! its tools, then the JSON Google answers with. A schema lists the arguments
//! in the order they are checked: of several wrong ones, the agent is told
//! about the first.

use std::sync::LazyLock;

use chrono::NaiveDate;
use mymcps_builtin::arguments::{
    TOOL_VINE, VineArgument, argument, blank_as_missing, choice, integer, line, list_length,
    pattern,
};
use mymcps_vine as vine;
use serde_json::{Value, json};
use url::Url;
use vine::{JsRegex, Rule, Validator, VineArray};

/// The bounds the tools advertise, and the schemas below enforce.
pub mod limits {
    /// Google returns at most this many rows for one query.
    pub const ROWS: i64 = 25_000;
    pub const FILTERS: usize = 10;
    pub const EXPRESSION_LENGTH: usize = 4096;
    pub const URL_LENGTH: usize = 2048;
}

/// What a report can group its figures by.
pub const DIMENSIONS: [&str; 6] = [
    "query",
    "page",
    "country",
    "device",
    "searchAppearance",
    "date",
];
/// What a report can be narrowed by: every dimension but the date, which the period sets.
pub const FILTER_DIMENSIONS: [&str; 5] = ["query", "page", "country", "device", "searchAppearance"];
pub const FILTER_OPERATORS: [&str; 6] = [
    "equals",
    "notEquals",
    "contains",
    "notContains",
    "includingRegex",
    "excludingRegex",
];
/// Where in Google the results were shown.
pub const SEARCH_TYPES: [&str; 6] = ["web", "image", "video", "news", "discover", "googleNews"];
pub const AGGREGATION_TYPES: [&str; 3] = ["auto", "byPage", "byProperty"];
pub const DATA_STATES: [&str; 2] = ["final", "all"];
pub const PERIODS: [&str; 4] = [
    "LAST_7_DAYS",
    "LAST_28_DAYS",
    "LAST_3_MONTHS",
    "LAST_16_MONTHS",
];

fn js_regex(expression: &str) -> JsRegex {
    vine::js::regex(expression, "").expect("static regex")
}

/// A Search Console property as Google writes it, or `None` when this is not one:
/// `sc-domain:example.com` for a domain property, and for a URL-prefix
/// property its address, which always ends with a slash. The scheme and the
/// host are lowercased, as Google does, and `https://example.com` is taken
/// for `https://example.com/`.
pub fn normalize_site_url(written: &str) -> Option<String> {
    static DOMAIN: LazyLock<JsRegex> = LazyLock::new(|| {
        js_regex(r"^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$")
    });
    let written = vine::js::trim(written);
    let prefix = "sc-domain:";
    if written
        .get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
    {
        let domain = written[prefix.len()..].to_ascii_lowercase();
        return DOMAIN.test(&domain).then(|| format!("{prefix}{domain}"));
    }
    let url = Url::parse(written).ok()?;
    let is_property = matches!(url.scheme(), "https" | "http")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.path().ends_with('/');
    is_property.then(|| url.to_string())
}

fn site_url_rule() -> Rule {
    vine::rule(|value, field| match value.as_str().and_then(normalize_site_url) {
        Some(site) => field.mutate(site),
        None => field.report(
            "{{ field }} must be a Search Console property such as sc-domain:example.com or https://www.example.com/, as list_sites returns them",
            "siteUrl",
        ),
    })
    .json_schema(|schema| {
        schema.insert("type".to_owned(), json!("string"));
    })
}

/// The property a tool acts on.
pub fn site_url() -> VineArgument {
    argument(site_url_rule()).parse(blank_as_missing)
}

fn web_address_rule() -> Rule {
    vine::rule(|value, field| {
        let url = value.as_str().and_then(|written| Url::parse(written).ok());
        if !url.is_some_and(|url| matches!(url.scheme(), "https" | "http") && url.has_host()) {
            field.report(
                "{{ field }} must be a web address such as https://www.example.com/page",
                "webAddress",
            );
        }
    })
}

/// The full address of a page or of a sitemap.
pub fn web_address() -> VineArgument {
    line(limits::URL_LENGTH).use_rule(web_address_rule())
}

fn calendar_date_rule() -> Rule {
    static WRITTEN: LazyLock<JsRegex> = LazyLock::new(|| js_regex(r"^\d{4}-\d{2}-\d{2}$"));
    vine::rule(|value, field| {
        let written = value.as_str().map(vine::js::trim).unwrap_or_default();
        let exists =
            WRITTEN.test(written) && NaiveDate::parse_from_str(written, "%Y-%m-%d").is_ok();
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

/// A day, as `YYYY-MM-DD`. Search Console counts days in Pacific Time.
pub fn calendar_date() -> VineArgument {
    argument(calendar_date_rule()).parse(blank_as_missing)
}

/// A single value is a list of one.
fn as_list(value: Option<Value>, _: &vine::ParseContext<'_>) -> Option<Value> {
    match value? {
        Value::Null => None,
        Value::String(text) if text.is_empty() => None,
        Value::Array(items) => Some(Value::Array(items)),
        single => Some(Value::Array(vec![single])),
    }
}

fn dimensions() -> VineArray {
    vine::array(choice(DIMENSIONS))
        .parse(as_list)
        .use_rule(list_length(
            ..=DIMENSIONS.len(),
            &format!(
                "{{{{ field }}}} must be a list of dimensions among {}",
                DIMENSIONS.join(", ")
            ),
        ))
}

fn filters() -> VineArray {
    let max = limits::FILTERS;
    vine::array(vine::object! {
        "dimension" => choice(FILTER_DIMENSIONS),
        "operator" => choice(FILTER_OPERATORS).optional(),
        "expression" => line(limits::EXPRESSION_LENGTH),
    })
    .parse(as_list)
    .use_rule(list_length(
        1..=max,
        &format!(
            "{{{{ field }}}} must be a list of 1 to {max} filters such as {{\"dimension\": \"page\", \"operator\": \"contains\", \"expression\": \"/blog/\"}}"
        ),
    ))
}

pub static QUERY_SEARCH_ANALYTICS_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "site_url" => site_url(),
        "period" => choice(PERIODS).optional(),
        "start_date" => calendar_date().optional(),
        "end_date" => calendar_date().optional(),
        "dimensions" => dimensions().optional(),
        "search_type" => choice(SEARCH_TYPES).optional(),
        "filters" => filters().optional(),
        "aggregation_type" => choice(AGGREGATION_TYPES).optional(),
        "data_state" => choice(DATA_STATES).optional(),
        "row_limit" => integer(1..=limits::ROWS).optional(),
        "start_row" => integer(0..).optional(),
    })
});

pub static LIST_SITEMAPS_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "site_url" => site_url(),
        "sitemap_index" => web_address().optional(),
    })
});

/// One sitemap of a property, to read, submit, or delete.
pub static SITEMAP_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "site_url" => site_url(),
        "feedpath" => web_address(),
    })
});

pub static INSPECT_URL_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    TOOL_VINE.create(vine::object! {
        "site_url" => site_url(),
        "inspection_url" => web_address(),
        "language_code" => pattern(
            r"^[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8}){0,2}$",
            "a language code such as en-US or fr",
        )
        .optional(),
    })
});

/// The body of a response Google sent with an error status. `errors` carries
/// the old reasons, such as `accessNotConfigured`, and `details` the new ones,
/// such as `SERVICE_DISABLED`.
pub static FAILURE_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "error" => vine::object! {
            "code" => vine::number().optional(),
            "message" => vine::string().optional(),
            "status" => vine::string().optional(),
            "errors" => vine::array(vine::object! {
                "reason" => vine::string().optional(),
                "message" => vine::string().optional(),
            })
            .optional(),
            "details" => vine::array(vine::object! {
                "reason" => vine::string().optional(),
            })
            .optional(),
        },
    })
});

pub static SITES_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "siteEntry" => vine::array(vine::object! {
            "siteUrl" => vine::string(),
            "permissionLevel" => vine::string().optional(),
        })
        .optional(),
    })
});

/// A report: one row for each combination of the dimensions asked for, or a
/// single row of totals when none was.
pub static ROWS_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "rows" => vine::array(vine::object! {
            "keys" => vine::array(vine::string()).optional(),
            "clicks" => vine::number().optional(),
            "impressions" => vine::number().optional(),
            "ctr" => vine::number().optional(),
            "position" => vine::number().optional(),
        })
        .optional(),
        "responseAggregationType" => vine::string().optional(),
    })
});

fn sitemap() -> vine::VineObject {
    // Google writes its 64-bit counts as text, which Vine's numbers read.
    vine::object! {
        "path" => vine::string(),
        "type" => vine::string().optional(),
        "lastSubmitted" => vine::string().optional(),
        "lastDownloaded" => vine::string().optional(),
        "isPending" => vine::boolean().optional(),
        "isSitemapsIndex" => vine::boolean().optional(),
        "warnings" => vine::number().optional(),
        "errors" => vine::number().optional(),
        "contents" => vine::array(vine::object! {
            "type" => vine::string().optional(),
            "submitted" => vine::number().optional(),
        })
        .optional(),
    }
}

pub static SITEMAPS_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "sitemap" => vine::array(sitemap()).optional(),
    })
});

pub static SITEMAP_ANSWER_VALIDATOR: LazyLock<Validator> =
    LazyLock::new(|| vine::global().create(sitemap()));

fn verdict() -> vine::VineObject {
    vine::object! { "verdict" => vine::string().optional() }
}

/// What the URL Inspection API says of a page: how Google indexed it, and
/// the verdicts of its other checks.
pub static INSPECTION_VALIDATOR: LazyLock<Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "inspectionResult" => vine::object! {
            "inspectionResultLink" => vine::string().optional(),
            "indexStatusResult" => vine::object! {
                "verdict" => vine::string().optional(),
                "coverageState" => vine::string().optional(),
                "robotsTxtState" => vine::string().optional(),
                "indexingState" => vine::string().optional(),
                "lastCrawlTime" => vine::string().optional(),
                "pageFetchState" => vine::string().optional(),
                "googleCanonical" => vine::string().optional(),
                "userCanonical" => vine::string().optional(),
                "crawledAs" => vine::string().optional(),
                "sitemap" => vine::array(vine::string()).optional(),
                "referringUrls" => vine::array(vine::string()).optional(),
            }
            .optional(),
            "mobileUsabilityResult" => verdict().optional(),
            "ampResult" => verdict().optional(),
            "richResultsResult" => vine::object! {
                "verdict" => vine::string().optional(),
                "detectedItems" => vine::array(vine::object! {
                    "richResultType" => vine::string().optional(),
                })
                .optional(),
            }
            .optional(),
        },
    })
});
