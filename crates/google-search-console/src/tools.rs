//! The tools of the built-in Google Search Console MCP: five that read, and
//! two that submit or delete a sitemap once the admin allows write access.

use std::sync::Arc;

use chrono::{Days, Months, NaiveDate, Utc};
use http::Method;
use mymcps_builtin::arguments::{NO_ARGUMENTS_VALIDATOR, NoArguments};
use mymcps_builtin::{BuiltinError, BuiltinResult, BuiltinTool, BuiltinToolContext};
use mymcps_vine as vine;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::api::{
    SearchConsoleRequest, allowed_sites, path_segment, search_console_request, site_of, site_path,
};
use crate::validators::{
    AGGREGATION_TYPES, DATA_STATES, DIMENSIONS, FILTER_DIMENSIONS, FILTER_OPERATORS,
    INSPECT_URL_VALIDATOR, INSPECTION_VALIDATOR, LIST_SITEMAPS_VALIDATOR, PERIODS,
    QUERY_SEARCH_ANALYTICS_VALIDATOR, ROWS_VALIDATOR, SEARCH_TYPES, SITEMAP_ANSWER_VALIDATOR,
    SITEMAP_VALIDATOR, SITEMAPS_VALIDATOR, SITES_VALIDATOR, limits, normalize_site_url,
};
use crate::{READ_SCOPE, WRITE_SCOPE};

type Context = Arc<BuiltinToolContext>;
type Tool = BuiltinTool<BuiltinToolContext>;

const READ_SCOPES: [&str; 2] = [READ_SCOPE, WRITE_SCOPE];
const DEFAULT_PERIOD: &str = "LAST_28_DAYS";
const DEFAULT_ROWS: u64 = 100;

/// Every tool, the read ones first.
pub fn search_console_tools() -> Vec<Tool> {
    vec![
        list_sites_tool(),
        query_search_analytics_tool(),
        list_sitemaps_tool(),
        get_sitemap_tool(),
        inspect_url_tool(),
        submit_sitemap_tool(),
        delete_sitemap_tool(),
    ]
}

/// The arguments of a tool as the agent reads them.
fn input_schema(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required, "additionalProperties": false })
}

fn site_url_property() -> (String, Value) {
    (
        "site_url".to_owned(),
        json!({
            "type": "string",
            "description": "The Search Console property, such as sc-domain:example.com or https://www.example.com/, as list_sites returns it.",
        }),
    )
}

fn feedpath_property(what: &str) -> (String, Value) {
    (
        "feedpath".to_owned(),
        json!({
            "type": "string",
            "description": format!("The full address of the sitemap {what}, such as https://www.example.com/sitemap.xml."),
        }),
    )
}

fn properties(entries: impl IntoIterator<Item = (String, Value)>) -> Value {
    Value::Object(entries.into_iter().collect())
}

/// The first and the last day a named period covers. It ends yesterday in
/// UTC: Google may not have all of today, and with `data_state: final` the
/// last two or three days come back empty until it does.
pub fn period_dates(period: &str, today: NaiveDate) -> (NaiveDate, NaiveDate) {
    let end = today - Days::new(1);
    let start = match period {
        "LAST_7_DAYS" => end - Days::new(6),
        "LAST_3_MONTHS" => end - Months::new(3) + Days::new(1),
        "LAST_16_MONTHS" => end - Months::new(16) + Days::new(1),
        _ => end - Days::new(27),
    };
    (start, end)
}

fn round(value: f64, decimals: i32) -> Value {
    let scale = 10_f64.powi(decimals);
    vine::js::number((value * scale).round() / scale)
}

fn list_sites_tool() -> Tool {
    BuiltinTool::new(
        "list_sites",
        "List the Search Console properties this MCP can use, with the permission the connected Google account has on each: siteOwner, siteFullUser, siteRestrictedUser, or siteUnverifiedUser, which cannot read any data. A property is either a domain, such as sc-domain:example.com, or a URL prefix, such as https://www.example.com/.",
        json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        &NO_ARGUMENTS_VALIDATOR,
        |_: NoArguments, context: Context| async move { list_sites(&context).await },
    )
    .requires_any_scope(&READ_SCOPES)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SiteEntry {
    site_url: String,
    permission_level: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Sites {
    #[serde(default)]
    site_entry: Vec<SiteEntry>,
}

async fn list_sites(context: &BuiltinToolContext) -> BuiltinResult<Value> {
    let answer =
        search_console_request(context, "/webmasters/v3/sites", SearchConsoleRequest::get())
            .await?;
    let sites: Sites = SITES_VALIDATOR
        .validate_as(&answer)
        .map_err(|_| BuiltinError::tool("Google Search Console did not return its properties"))?;
    let allowed = allowed_sites(context);
    let sites: Vec<Value> = sites
        .site_entry
        .into_iter()
        .filter(|site| match &allowed {
            None => true,
            Some(allowed) => normalize_site_url(&site.site_url)
                .is_some_and(|site| allowed.contains(&site.as_str())),
        })
        .map(|site| json!({ "site_url": site.site_url, "permission_level": site.permission_level }))
        .collect();
    Ok(json!({ "sites": sites }))
}

fn query_search_analytics_tool() -> Tool {
    let filter = json!({
        "type": "object",
        "properties": {
            "dimension": { "type": "string", "enum": FILTER_DIMENSIONS },
            "operator": { "type": "string", "enum": FILTER_OPERATORS, "default": "equals" },
            "expression": {
                "type": "string",
                "description": "What to compare with. Countries are ISO 3166-1 alpha-3 codes such as fra, devices DESKTOP, MOBILE, or TABLET. Regular expressions use RE2 syntax.",
            },
        },
        "required": ["dimension", "expression"],
    });
    BuiltinTool::new(
        "query_search_analytics",
        "Report how a property does in Google Search over a period: clicks, impressions, click-through rate (ctr, in percent), and average position, grouped by query, page, country, device, search appearance, or date, and narrowed by filters. Rows come sorted by clicks. Without dimensions, one row holds the totals. Google keeps 16 months of data, and the last two or three days are only final later.",
        input_schema(
            properties([
                site_url_property(),
                (
                    "period".to_owned(),
                    json!({
                        "type": "string",
                        "enum": PERIODS,
                        "default": DEFAULT_PERIOD,
                        "description": "The days the report covers, ending yesterday (UTC). Ignored when start_date and end_date are set.",
                    }),
                ),
                (
                    "start_date".to_owned(),
                    json!({ "type": "string", "description": "First day of a custom period, such as 2026-01-01, in Pacific Time. Set it with end_date." }),
                ),
                (
                    "end_date".to_owned(),
                    json!({ "type": "string", "description": "Last day of a custom period, such as 2026-01-31. Set it with start_date." }),
                ),
                (
                    "dimensions".to_owned(),
                    json!({
                        "type": "array",
                        "items": { "type": "string", "enum": DIMENSIONS },
                        "maxItems": DIMENSIONS.len(),
                        "description": "What to group the figures by, in this order. Each row's dimension values come under these names.",
                    }),
                ),
                (
                    "search_type".to_owned(),
                    json!({
                        "type": "string",
                        "enum": SEARCH_TYPES,
                        "default": "web",
                        "description": "Which results to count: the web tab, image, video, or news searches, Discover, or Google News.",
                    }),
                ),
                (
                    "filters".to_owned(),
                    json!({
                        "type": "array",
                        "items": filter,
                        "minItems": 1,
                        "maxItems": limits::FILTERS,
                        "description": "Conditions every row must meet, such as {\"dimension\": \"page\", \"operator\": \"contains\", \"expression\": \"/blog/\"}.",
                    }),
                ),
                (
                    "aggregation_type".to_owned(),
                    json!({
                        "type": "string",
                        "enum": AGGREGATION_TYPES,
                        "default": "auto",
                        "description": "Whether figures are counted by property or by page. byPage counts each page that appeared separately.",
                    }),
                ),
                (
                    "data_state".to_owned(),
                    json!({
                        "type": "string",
                        "enum": DATA_STATES,
                        "default": "final",
                        "description": "final for settled figures only, all to include the latest days, which may still change.",
                    }),
                ),
                (
                    "row_limit".to_owned(),
                    json!({ "type": "integer", "minimum": 1, "maximum": limits::ROWS, "default": DEFAULT_ROWS, "description": "Most rows to return." }),
                ),
                (
                    "start_row".to_owned(),
                    json!({ "type": "integer", "minimum": 0, "default": 0, "description": "Rows to skip, to read the next page of a report." }),
                ),
            ]),
            &["site_url"],
        ),
        &QUERY_SEARCH_ANALYTICS_VALIDATOR,
        |input: QuerySearchAnalytics, context: Context| async move {
            query_search_analytics(&context, input).await
        },
    )
    .requires_any_scope(&READ_SCOPES)
}

#[derive(Debug, Deserialize, Serialize)]
struct Filter {
    dimension: String,
    operator: Option<String>,
    expression: String,
}

#[derive(Debug, Deserialize)]
struct QuerySearchAnalytics {
    site_url: String,
    period: Option<String>,
    start_date: Option<NaiveDate>,
    end_date: Option<NaiveDate>,
    dimensions: Option<Vec<String>>,
    search_type: Option<String>,
    filters: Option<Vec<Filter>>,
    aggregation_type: Option<String>,
    data_state: Option<String>,
    row_limit: Option<u64>,
    start_row: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct Row {
    #[serde(default)]
    keys: Vec<String>,
    clicks: Option<f64>,
    impressions: Option<f64>,
    ctr: Option<f64>,
    position: Option<f64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Rows {
    #[serde(default)]
    rows: Vec<Row>,
    response_aggregation_type: Option<String>,
}

fn dates_of(input: &QuerySearchAnalytics) -> BuiltinResult<(NaiveDate, NaiveDate)> {
    match (input.start_date, input.end_date) {
        (None, None) => Ok(period_dates(
            input.period.as_deref().unwrap_or(DEFAULT_PERIOD),
            Utc::now().date_naive(),
        )),
        (Some(start), Some(end)) if start > end => {
            Err(BuiltinError::tool("start_date must not be after end_date"))
        }
        (Some(start), Some(end)) => Ok((start, end)),
        _ => Err(BuiltinError::tool(
            "Set both start_date and end_date, or neither and use period",
        )),
    }
}

async fn query_search_analytics(
    context: &BuiltinToolContext,
    input: QuerySearchAnalytics,
) -> BuiltinResult<Value> {
    let site = site_of(context, &input.site_url)?;
    let (start, end) = dates_of(&input)?;
    let mut dimensions: Vec<String> = Vec::new();
    for dimension in input.dimensions.iter().flatten() {
        if !dimensions.contains(dimension) {
            dimensions.push(dimension.clone());
        }
    }
    let search_type = input.search_type.as_deref().unwrap_or("web");
    let row_limit = input.row_limit.unwrap_or(DEFAULT_ROWS);
    let start_row = input.start_row.unwrap_or(0);

    let mut body = Map::new();
    body.insert("startDate".to_owned(), json!(start.to_string()));
    body.insert("endDate".to_owned(), json!(end.to_string()));
    if !dimensions.is_empty() {
        body.insert("dimensions".to_owned(), json!(dimensions));
    }
    body.insert("type".to_owned(), json!(search_type));
    if let Some(filters) = &input.filters {
        let filters: Vec<Value> = filters
            .iter()
            .map(|filter| {
                json!({
                    "dimension": filter.dimension,
                    "operator": filter.operator.as_deref().unwrap_or("equals"),
                    "expression": filter.expression,
                })
            })
            .collect();
        body.insert(
            "dimensionFilterGroups".to_owned(),
            json!([{ "groupType": "and", "filters": filters }]),
        );
    }
    if let Some(aggregation) = &input.aggregation_type {
        body.insert("aggregationType".to_owned(), json!(aggregation));
    }
    if let Some(state) = &input.data_state {
        body.insert("dataState".to_owned(), json!(state));
    }
    body.insert("rowLimit".to_owned(), json!(row_limit));
    if start_row > 0 {
        body.insert("startRow".to_owned(), json!(start_row));
    }

    let answer = search_console_request(
        context,
        &site_path(site, "/searchAnalytics/query"),
        SearchConsoleRequest::post(Value::Object(body)),
    )
    .await?;
    let report: Rows = ROWS_VALIDATOR.validate_as(&answer).map_err(|_| {
        BuiltinError::tool("Google Search Console did not return the rows of a report")
    })?;

    let rows: Vec<Value> = report
        .rows
        .iter()
        .map(|row| {
            let mut formatted = Map::new();
            for (dimension, key) in dimensions.iter().zip(&row.keys) {
                formatted.insert(dimension.clone(), json!(key));
            }
            formatted.insert("clicks".to_owned(), round(row.clicks.unwrap_or(0.0), 0));
            formatted.insert(
                "impressions".to_owned(),
                round(row.impressions.unwrap_or(0.0), 0),
            );
            formatted.insert("ctr".to_owned(), round(row.ctr.unwrap_or(0.0) * 100.0, 2));
            formatted.insert("position".to_owned(), round(row.position.unwrap_or(0.0), 1));
            Value::Object(formatted)
        })
        .collect();

    let mut result = Map::new();
    result.insert("site_url".to_owned(), json!(site));
    result.insert("start_date".to_owned(), json!(start.to_string()));
    result.insert("end_date".to_owned(), json!(end.to_string()));
    result.insert("search_type".to_owned(), json!(search_type));
    if let Some(aggregation) = report.response_aggregation_type {
        result.insert("aggregation".to_owned(), json!(aggregation));
    }
    if u64::try_from(rows.len()).is_ok_and(|count| count >= row_limit) {
        result.insert("truncated".to_owned(), json!(true));
        result.insert("next_start_row".to_owned(), json!(start_row + row_limit));
    }
    result.insert("rows".to_owned(), Value::Array(rows));
    Ok(Value::Object(result))
}

/// A sitemap as the tools return it.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all(deserialize = "camelCase"))]
struct Sitemap {
    path: String,
    #[serde(rename(deserialize = "type"), skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_submitted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_downloaded: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_pending: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_sitemaps_index: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    errors: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    warnings: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    contents: Vec<SitemapContent>,
}

/// How many addresses of one kind, such as `web` or `image`, the sitemap lists.
#[derive(Debug, Deserialize, Serialize)]
struct SitemapContent {
    #[serde(rename(serialize = "kind"), skip_serializing_if = "Option::is_none")]
    r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    submitted: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct Sitemaps {
    #[serde(default)]
    sitemap: Vec<Sitemap>,
}

const SITEMAP_FIELDS: &str = "Each sitemap comes with its type, when it was last submitted and last downloaded by Google, whether it is still pending, its errors and warnings, and how many addresses of each kind it lists.";

fn list_sitemaps_tool() -> Tool {
    BuiltinTool::new(
        "list_sitemaps",
        format!("List the sitemaps submitted for a property, or the ones a sitemap index lists. {SITEMAP_FIELDS}"),
        input_schema(
            properties([
                site_url_property(),
                (
                    "sitemap_index".to_owned(),
                    json!({ "type": "string", "description": "Only the sitemaps of this sitemap index, by its full address." }),
                ),
            ]),
            &["site_url"],
        ),
        &LIST_SITEMAPS_VALIDATOR,
        |input: ListSitemaps, context: Context| async move { list_sitemaps(&context, input).await },
    )
    .requires_any_scope(&READ_SCOPES)
}

#[derive(Debug, Deserialize)]
struct ListSitemaps {
    site_url: String,
    sitemap_index: Option<String>,
}

async fn list_sitemaps(context: &BuiltinToolContext, input: ListSitemaps) -> BuiltinResult<Value> {
    let site = site_of(context, &input.site_url)?;
    let query = input
        .sitemap_index
        .map(|index| format!("?sitemapIndex={}", path_segment(&index)))
        .unwrap_or_default();
    let answer = search_console_request(
        context,
        &site_path(site, &format!("/sitemaps{query}")),
        SearchConsoleRequest::get(),
    )
    .await?;
    let listed: Sitemaps = SITEMAPS_VALIDATOR
        .validate_as(&answer)
        .map_err(|_| BuiltinError::tool("Google Search Console did not return the sitemaps"))?;
    Ok(json!({ "site_url": site, "sitemaps": listed.sitemap }))
}

#[derive(Debug, Deserialize)]
struct SitemapInput {
    site_url: String,
    feedpath: String,
}

fn sitemap_schema(what: &str) -> Value {
    input_schema(
        properties([site_url_property(), feedpath_property(what)]),
        &["site_url", "feedpath"],
    )
}

fn sitemap_path(site: &str, feedpath: &str) -> String {
    site_path(site, &format!("/sitemaps/{}", path_segment(feedpath)))
}

fn get_sitemap_tool() -> Tool {
    BuiltinTool::new(
        "get_sitemap",
        format!("Get one sitemap of a property. {SITEMAP_FIELDS}"),
        sitemap_schema("to read"),
        &SITEMAP_VALIDATOR,
        |input: SitemapInput, context: Context| async move {
            let site = site_of(&context, &input.site_url)?;
            let answer = search_console_request(
                &context,
                &sitemap_path(site, &input.feedpath),
                SearchConsoleRequest::get(),
            )
            .await?;
            let sitemap: Sitemap = SITEMAP_ANSWER_VALIDATOR.validate_as(&answer).map_err(|_| {
                BuiltinError::tool("Google Search Console did not return the sitemap")
            })?;
            Ok(json!(sitemap))
        },
    )
    .requires_any_scope(&READ_SCOPES)
}

fn inspect_url_tool() -> Tool {
    BuiltinTool::new(
        "inspect_url",
        "Inspect how Google indexed one page of a property, as the URL Inspection tool of Search Console does: the verdict, the coverage state (such as Submitted and indexed, or Crawled - currently not indexed), what robots.txt and the page allow, when and as which crawler Google last fetched it, the canonical Google chose against the one the page declares, the sitemaps and pages that lead to it, and the verdicts of the AMP and rich result checks. It reads Google's index, not the live page. Google allows 2,000 inspections a day for each property.",
        input_schema(
            properties([
                site_url_property(),
                (
                    "inspection_url".to_owned(),
                    json!({ "type": "string", "description": "The full address of the page, which must belong to the property." }),
                ),
                (
                    "language_code".to_owned(),
                    json!({ "type": "string", "default": "en-US", "description": "The language of the messages Google writes, such as en-US or fr." }),
                ),
            ]),
            &["site_url", "inspection_url"],
        ),
        &INSPECT_URL_VALIDATOR,
        |input: InspectUrl, context: Context| async move { inspect_url(&context, input).await },
    )
    .requires_any_scope(&READ_SCOPES)
}

#[derive(Debug, Deserialize)]
struct InspectUrl {
    site_url: String,
    inspection_url: String,
    language_code: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Inspection {
    inspection_result: InspectionResult,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InspectionResult {
    inspection_result_link: Option<String>,
    index_status_result: Option<IndexStatus>,
    mobile_usability_result: Option<Verdict>,
    amp_result: Option<Verdict>,
    rich_results_result: Option<RichResults>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all(deserialize = "camelCase"))]
struct IndexStatus {
    #[serde(skip_serializing_if = "Option::is_none")]
    verdict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    coverage_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    robots_txt_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    indexing_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page_fetch_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_crawl_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    crawled_as: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    google_canonical: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_canonical: Option<String>,
    #[serde(
        default,
        rename(serialize = "sitemaps"),
        skip_serializing_if = "Vec::is_empty"
    )]
    sitemap: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    referring_urls: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Verdict {
    verdict: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RichResults {
    verdict: Option<String>,
    #[serde(default)]
    detected_items: Vec<RichResultItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RichResultItem {
    rich_result_type: Option<String>,
}

async fn inspect_url(context: &BuiltinToolContext, input: InspectUrl) -> BuiltinResult<Value> {
    let site = site_of(context, &input.site_url)?;
    let mut body = Map::new();
    body.insert("inspectionUrl".to_owned(), json!(input.inspection_url));
    body.insert("siteUrl".to_owned(), json!(site));
    if let Some(language) = &input.language_code {
        body.insert("languageCode".to_owned(), json!(language));
    }
    let answer = search_console_request(
        context,
        "/v1/urlInspection/index:inspect",
        SearchConsoleRequest::post(Value::Object(body)),
    )
    .await?;
    let inspection: Inspection = INSPECTION_VALIDATOR
        .validate_as(&answer)
        .map_err(|_| BuiltinError::tool("Google Search Console did not return the inspection"))?;
    let found = inspection.inspection_result;

    let mut result = Map::new();
    result.insert("inspection_url".to_owned(), json!(input.inspection_url));
    result.insert("site_url".to_owned(), json!(site));
    if let Some(index_status) = found.index_status_result {
        result.insert("index_status".to_owned(), json!(index_status));
    }
    if let Some(verdict) = found
        .mobile_usability_result
        .and_then(|result| result.verdict)
    {
        result.insert("mobile_usability".to_owned(), json!(verdict));
    }
    if let Some(verdict) = found.amp_result.and_then(|result| result.verdict) {
        result.insert("amp".to_owned(), json!(verdict));
    }
    if let Some(rich) = found.rich_results_result {
        let mut types: Vec<String> = Vec::new();
        for kind in rich
            .detected_items
            .into_iter()
            .filter_map(|item| item.rich_result_type)
        {
            if !types.contains(&kind) {
                types.push(kind);
            }
        }
        result.insert(
            "rich_results".to_owned(),
            json!({ "verdict": rich.verdict, "types": types }),
        );
    }
    if let Some(link) = found.inspection_result_link {
        result.insert("link".to_owned(), json!(link));
    }
    Ok(Value::Object(result))
}

fn submit_sitemap_tool() -> Tool {
    BuiltinTool::new(
        "submit_sitemap",
        "Submit a sitemap for a property, or submit it again so that Google reads it anew. Google fetches it later, within hours or days: get_sitemap shows when it was last downloaded and what it found.",
        sitemap_schema("to submit"),
        &SITEMAP_VALIDATOR,
        |input: SitemapInput, context: Context| async move {
            let site = site_of(&context, &input.site_url)?;
            search_console_request(
                &context,
                &sitemap_path(site, &input.feedpath),
                SearchConsoleRequest::change(Method::PUT),
            )
            .await?;
            Ok(json!({ "site_url": site, "submitted": input.feedpath }))
        },
    )
    .write()
    .requires_any_scope(&[WRITE_SCOPE])
}

fn delete_sitemap_tool() -> Tool {
    BuiltinTool::new(
        "delete_sitemap",
        "Remove a sitemap from the ones submitted for a property. Google stops reporting on it, but pages it already found stay in its index.",
        sitemap_schema("to remove"),
        &SITEMAP_VALIDATOR,
        |input: SitemapInput, context: Context| async move {
            let site = site_of(&context, &input.site_url)?;
            search_console_request(
                &context,
                &sitemap_path(site, &input.feedpath),
                SearchConsoleRequest::change(Method::DELETE),
            )
            .await?;
            Ok(json!({ "site_url": site, "deleted": input.feedpath }))
        },
    )
    .write()
    .requires_any_scope(&[WRITE_SCOPE])
}
