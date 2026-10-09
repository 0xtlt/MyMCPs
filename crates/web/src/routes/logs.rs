//! The log of the tool calls made through the gateway: one call a row, with
//! its failure, its arguments and its timing. (`logs_controller.ts`)

use axum::Router;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use chrono::{DateTime, Duration, Utc};
use http::header::CACHE_CONTROL;
use http::{HeaderMap, HeaderValue, StatusCode};
use mymcps_core::models::{McpCallLog, McpLogLevel};
use mymcps_gateway::validators::mcp_call_log::LOGS_QUERY;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use serde::Deserialize;
use serde_json::Value;
use sqlx::sqlite::SqliteExecutor;

use crate::error::AppError;
use crate::forms::{FormState, refusal};
use crate::input::Input;
use crate::redirect::redirect_back;
use crate::respond::{fragment, page};
use crate::routes::FeatureRoutes;
use crate::routes::analytics::{Zone, resolve_time_zone, time_zone_label};
use crate::session::Session;
use crate::state::AppState;
use crate::views::logs::{call_details, logs_content, logs_page};
use crate::views::shell::PageContext;

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        admin: Router::new().route("/logs", get(index)),
        ..Default::default()
    }
}

pub const DEFAULT_PAGE_SIZE: i64 = 25;
pub const PAGE_SIZES: [i64; 4] = [10, 25, 50, 100];

/// What `encodeURIComponent` leaves as it is.
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// A path of this site with a query string. Parameters without a value are
/// left out.
pub fn link(path: &str, parameters: &[(&str, &str)]) -> String {
    let mut address = path.to_string();
    for (name, value) in parameters.iter().filter(|(_, value)| !value.is_empty()) {
        address.push(if address.contains('?') { '&' } else { '?' });
        address.push_str(name);
        address.push('=');
        address.extend(utf8_percent_encode(value, COMPONENT));
    }
    address
}

/// The element the page script will put the answer in, when it named one.
pub fn fragment_target(headers: &HeaderMap) -> Option<String> {
    let target = headers.get("x-fragment")?.to_str().ok()?;
    Some(percent_decode_str(target).decode_utf8_lossy().into_owned())
}

/// A fragment shares its address with the page it belongs to: a browser
/// that kept it would show it in place of the page.
pub fn uncached(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// The period of the calls listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogRange {
    Hours24,
    Days7,
    Days30,
    All,
}

impl LogRange {
    pub const ALL: [LogRange; 4] = [Self::Hours24, Self::Days7, Self::Days30, Self::All];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hours24 => "24h",
            Self::Days7 => "7d",
            Self::Days30 => "30d",
            Self::All => "all",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Hours24 => "Last 24 hours",
            Self::Days7 => "Last 7 days",
            Self::Days30 => "Last 30 days",
            Self::All => "All retained",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|range| range.as_str() == value)
    }

    fn duration(&self) -> Option<Duration> {
        match self {
            Self::Hours24 => Some(Duration::hours(24)),
            Self::Days7 => Some(Duration::days(7)),
            Self::Days30 => Some(Duration::days(30)),
            Self::All => None,
        }
    }

    /// What `created_at` is compared with to keep the calls of the period.
    /// The text Luxon's `toSQL()` gave the Node app: a call made in the very
    /// second of the cutoff is left out, as it was there.
    pub fn cutoff(&self, now: DateTime<Utc>) -> Option<String> {
        let cutoff = now - self.duration()?;
        Some(cutoff.format("%Y-%m-%d %H:%M:%S%.3f Z").to_string())
    }
}

/// The filters of the list, as they were asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogFilters {
    pub range: LogRange,
    /// `success`, `error`, or empty for both.
    pub outcome: String,
    /// The slug of an MCP, or empty for all.
    pub mcp: String,
    /// The identifier of an access token, or empty for all.
    pub token: String,
    pub page_size: i64,
    /// The name of the zone the times are shown in.
    pub time_zone: String,
}

/// What the Logs page shows.
#[derive(Debug, Clone)]
pub struct Logs {
    pub filters: LogFilters,
    pub zone: Zone,
    pub time_zone_label: String,
    pub logs: Vec<McpCallLog>,
    /// The call whose details are open.
    pub selected: Option<McpCallLog>,
    pub page: i64,
    pub total: i64,
    pub total_pages: i64,
    /// The MCPs that exist, as slug and name.
    pub mcp_options: Vec<(String, String)>,
    /// The tokens that made a call, as identifier and name.
    pub token_options: Vec<(String, String)>,
    pub logging_off: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LogsQuery {
    range: Option<String>,
    outcome: Option<String>,
    mcp: Option<String>,
    token: Option<String>,
    // Whole numbers of any size: a page past the last one is an empty page.
    page: Option<f64>,
    page_size: Option<i64>,
    log_id: Option<f64>,
    time_zone: Option<String>,
}

fn push_filters(
    query: &mut sqlx::QueryBuilder<sqlx::Sqlite>,
    filters: &LogFilters,
    cutoff: Option<&str>,
) {
    query.push(" where 1 = 1");
    if let Some(cutoff) = cutoff {
        query.push(" and `created_at` >= ");
        query.push_bind(cutoff.to_string());
    }
    for (column, value) in [
        ("`outcome`", &filters.outcome),
        ("`mcp_slug`", &filters.mcp),
        ("`access_token_prefix`", &filters.token),
    ] {
        if !value.is_empty() {
            query.push(" and ");
            query.push(column);
            query.push(" = ");
            query.push_bind(value.clone());
        }
    }
}

/// One page of the calls that pass the filters, the most recent first, and
/// how many pass them in all.
pub async fn find_logs<'e, E>(
    db: E,
    filters: &LogFilters,
    page: i64,
    now: DateTime<Utc>,
) -> Result<(Vec<McpCallLog>, i64), sqlx::Error>
where
    E: SqliteExecutor<'e> + Copy,
{
    let cutoff = filters.range.cutoff(now);
    let mut count = sqlx::QueryBuilder::<sqlx::Sqlite>::new("select count(*) from `mcp_call_logs`");
    push_filters(&mut count, filters, cutoff.as_deref());
    let total: i64 = count.build_query_scalar().fetch_one(db).await?;

    let mut rows = sqlx::QueryBuilder::<sqlx::Sqlite>::new("select * from `mcp_call_logs`");
    push_filters(&mut rows, filters, cutoff.as_deref());
    // Calls of the same second keep one order from page to page.
    rows.push(" order by `created_at` desc, `id` desc limit ");
    rows.push_bind(filters.page_size);
    rows.push(" offset ");
    rows.push_bind((page - 1).saturating_mul(filters.page_size));
    let logs = rows.build_query_as().fetch_all(db).await?;
    Ok((logs, total))
}

fn whole(number: f64) -> i64 {
    // Saturates: a number too large to be an id or a page is the largest one.
    number as i64
}

/// `GET /logs`
pub async fn index(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    state.gateway.call_log.prune_expired(false).await;

    let query: LogsQuery = match LOGS_QUERY.validate_as(&Value::Object(input.clone())) {
        Ok(query) => query,
        Err(error) => {
            let error = refusal(error)?;
            FormState::new(&error, &input).flash(&session);
            return Ok(redirect_back(&headers, "/"));
        }
    };
    let (time_zone, zone) = resolve_time_zone(query.time_zone.as_deref());
    let filters = LogFilters {
        range: query
            .range
            .as_deref()
            .and_then(LogRange::parse)
            .unwrap_or(LogRange::Hours24),
        outcome: query.outcome.unwrap_or_default(),
        mcp: query.mcp.unwrap_or_default(),
        token: query.token.unwrap_or_default(),
        page_size: query.page_size.unwrap_or(DEFAULT_PAGE_SIZE),
        time_zone,
    };
    let page_number = query.page.map_or(1, whole);
    let db = &*state.core.db;
    let now = Utc::now();

    let selected = match query.log_id {
        Some(id) => McpCallLog::find(db, whole(id)).await?,
        None => None,
    };
    let target = fragment_target(&headers).filter(|_| context.is_fetch);
    // The page script opens the details of a call in the panel of the page it is on.
    if target.as_deref() == Some("call-details") {
        return Ok(uncached(fragment(
            StatusCode::OK,
            call_details(selected.as_ref(), &zone),
        )));
    }

    let (logs, total) = find_logs(db, &filters, page_number, now).await?;
    // Slugs in the log are what callers asked for, including MCPs that never
    // existed. The filter lists the MCPs that do.
    let mcp_options: Vec<(String, String)> =
        sqlx::query_as("select `slug`, `name` from `mcps` order by `name` asc")
            .fetch_all(db)
            .await?;
    let token_options: Vec<(String, String)> = sqlx::query_as(
        "select `access_token_prefix`, `access_token_name` from `mcp_call_logs` \
         group by `access_token_prefix`, `access_token_name` order by `access_token_name` asc",
    )
    .fetch_all(db)
    .await?;
    let settings = state.gateway.call_log.settings().await?;

    let logs = Logs {
        time_zone_label: time_zone_label(&filters.time_zone, zone, now),
        zone,
        logs,
        selected,
        page: page_number,
        total,
        total_pages: ((total + filters.page_size - 1) / filters.page_size).max(1),
        mcp_options,
        token_options,
        logging_off: settings.mcp_log_level == McpLogLevel::Off,
        filters,
    };

    // A change of filter is answered with the filters and the list only.
    if target.as_deref() == Some("logs") {
        return Ok(uncached(fragment(StatusCode::OK, logs_content(&logs))));
    }
    Ok(page(logs_page(&context, &logs)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_addresses_with_their_parameters_encoded() {
        assert_eq!(link("/logs", &[]), "/logs");
        assert_eq!(
            link(
                "/logs",
                &[("range", "7d"), ("outcome", ""), ("mcp", "search")]
            ),
            "/logs?range=7d&mcp=search"
        );
        assert_eq!(
            link(
                "/analytics",
                &[("timeZone", "Europe/Paris"), ("start", "2026-10-25T00:30Z")]
            ),
            "/analytics?timeZone=Europe%2FParis&start=2026-10-25T00%3A30Z"
        );
        assert_eq!(
            link("/logs", &[("token", "a b&c=d\"<é>")]),
            "/logs?token=a%20b%26c%3Dd%22%3C%C3%A9%3E"
        );
    }

    #[test]
    fn cuts_a_period_where_the_node_app_did() {
        let now = DateTime::parse_from_rfc3339("2026-10-07T12:19:57.123Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(
            LogRange::Hours24.cutoff(now).as_deref(),
            Some("2026-10-06 12:19:57.123 Z")
        );
        assert_eq!(
            LogRange::Days7.cutoff(now).as_deref(),
            Some("2026-09-30 12:19:57.123 Z")
        );
        assert_eq!(
            LogRange::Days30.cutoff(now).as_deref(),
            Some("2026-09-07 12:19:57.123 Z")
        );
        assert_eq!(LogRange::All.cutoff(now), None);
    }

    #[test]
    fn reads_the_target_of_a_fragment_request() {
        let mut headers = HeaderMap::new();
        assert_eq!(fragment_target(&headers), None);
        headers.insert("x-fragment", HeaderValue::from_static("call-details"));
        assert_eq!(fragment_target(&headers).as_deref(), Some("call-details"));
        headers.insert("x-fragment", HeaderValue::from_static("logs%20results"));
        assert_eq!(fragment_target(&headers).as_deref(), Some("logs results"));
    }
}
