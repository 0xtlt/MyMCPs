//! The home page of a signed-in person: the gateway endpoint, what needs
//! attention, and for an administrator the activity of the gateway.
//!
//! The Node app's home was three links. This one is the "01 Home" screen of
//! the redesign, built from what the app knows. Everything that comes from
//! the call log is for administrators only, as the Logs page is.

use std::collections::HashMap;
use std::sync::LazyLock;

use axum::Router;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use chrono::{DateTime, Duration, Utc};
use http::{HeaderMap, StatusCode};
use mymcps_core::models::{
    AccessToken, GatewayToolMode, Mcp, McpCallLog, McpLogLevel, McpStatus, TokenSource, User,
};
use mymcps_core::{Db, Timestamp};
use mymcps_vine as vine;
use serde_json::{Map, Value};

use crate::error::AppError;
use crate::input::Input;
use crate::respond::{fragment, page};
use crate::routes::FeatureRoutes;
use crate::routes::analytics::{
    Metrics, Range, Zone, buckets, days_ending_today, metrics, resolve_time_zone, timeline,
};
use crate::routes::logs::{LogRange, fragment_target, uncached};
use crate::session::Session;
use crate::state::AppState;
use crate::views::charts::Bar;
use crate::views::home::{activity_content, home_page};
use crate::views::shell::PageContext;

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        signed_in: Router::new().route("/", get(show)),
        ..Default::default()
    }
}

/// How many days the activity chart covers, and the period before it that
/// it is compared with.
pub const ACTIVITY_DAYS: usize = 14;

/// A token or an invite that ends within this many days is worth a word.
const SOON_DAYS: i64 = 7;

/// Where the viewer's time zone is kept between two visits, so that the
/// activity chart is cut in their days from the first byte.
const TIME_ZONE_KEY: &str = "timeZone";

/// The client the "Install in a client" link opens the install dialog on:
/// the ids of `mcp_install_config.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Client {
    Codex,
    Claude,
    Cursor,
}

impl Client {
    pub const ALL: [Client; 3] = [Self::Codex, Self::Claude, Self::Cursor];

    pub fn id(&self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Cursor => "cursor",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Cursor => "Cursor",
        }
    }
}

static RANGE: LazyLock<vine::Validator> =
    LazyLock::new(|| vine::global().create(vine::enum_(["24h", "7d", "30d"])));
static CLIENT: LazyLock<vine::Validator> =
    LazyLock::new(|| vine::global().create(vine::enum_(["codex", "claude", "cursor"])));
static TIME_ZONE: LazyLock<vine::Validator> =
    LazyLock::new(|| vine::global().create(vine::string().trim().max_length(100)));

/// What the address of the page chooses. A parameter the page cannot read
/// is one left out: people land here from other pages with their query
/// string, when a page is not for them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeQuery {
    pub range: LogRange,
    pub client: Client,
    pub time_zone: Option<String>,
}

impl HomeQuery {
    pub fn read(input: &Map<String, Value>) -> Self {
        let text =
            |validator: &vine::Validator, name: &str| match validator.validate(input.get(name)) {
                Ok(Value::String(text)) => Some(text),
                _ => None,
            };
        Self {
            range: text(&RANGE, "range")
                .and_then(|range| {
                    LogRange::ALL
                        .into_iter()
                        .find(|known| known.as_str() == range)
                })
                .unwrap_or(LogRange::Days7),
            client: text(&CLIENT, "client")
                .and_then(|client| Client::ALL.into_iter().find(|known| known.id() == client))
                .unwrap_or(Client::Claude),
            time_zone: text(&TIME_ZONE, "timeZone"),
        }
    }
}

/// Calls of the period chosen in the context bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub metrics: Metrics,
    /// Nothing new is recorded: the rates would describe old calls only.
    pub logging_off: bool,
}

impl Stats {
    /// Whether there is something to compute a rate from.
    pub fn has_rates(&self) -> bool {
        !self.logging_off && self.metrics.total > 0
    }
}

/// The "Gateway activity" card.
#[derive(Debug, Clone)]
pub struct Activity {
    /// One bar a day, the oldest first.
    pub days: Vec<Bar>,
    pub total: i64,
    pub errors: i64,
    /// Calls of the same number of days before the chart.
    pub previous_total: i64,
    pub recent: Vec<McpCallLog>,
    pub logging_off: bool,
    /// The name of the zone the days are cut in.
    pub time_zone: String,
}

impl Activity {
    /// The change against the period before, in percent. `None` when there
    /// is nothing to compare with.
    pub fn change(&self) -> Option<i64> {
        if self.previous_total <= 0 {
            return None;
        }
        let change = (self.total - self.previous_total) as f64 / self.previous_total as f64;
        Some((change * 100.0).round() as i64)
    }
}

/// How many tools the enabled MCPs expose, as far as it is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolsExposed {
    pub tools: usize,
    /// The enabled MCPs whose tools were counted.
    pub mcps: usize,
}

/// Tool lists are not stored: the count is known for the MCPs that listed
/// their tools since the server started. `None` when none of the enabled
/// ones did.
pub fn tools_exposed(mcps: &[Mcp], counts: &HashMap<i64, usize>) -> Option<ToolsExposed> {
    let known: Vec<usize> = mcps
        .iter()
        .filter(|mcp| mcp.enabled)
        .filter_map(|mcp| counts.get(&mcp.id).copied())
        .collect();
    (!known.is_empty()).then(|| ToolsExposed {
        tools: known.iter().sum(),
        mcps: known.len(),
    })
}

/// When the connection of a token ends: an OAuth connection lives as long
/// as it can be refreshed.
pub fn connection_expires_at(token: &AccessToken) -> Option<Timestamp> {
    if token.source == TokenSource::Oauth {
        token.oauth_refresh_expires_at.or(token.expires_at)
    } else {
        token.expires_at
    }
}

/// Where a token stands, as its badge says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenState {
    Active,
    /// Active, and ends within a week.
    Expiring,
    Expired,
    Revoked,
}

pub fn token_state(token: &AccessToken, now: Timestamp) -> TokenState {
    if token.is_revoked() {
        TokenState::Revoked
    } else if !token.is_active() {
        TokenState::Expired
    } else if connection_expires_at(token)
        .is_some_and(|expires_at| expires_at <= now + Duration::days(SOON_DAYS))
    {
        TokenState::Expiring
    } else {
        TokenState::Active
    }
}

/// What the home page shows.
#[derive(Debug, Clone)]
pub struct Home {
    pub admin: bool,
    /// The name the person is greeted by, when one is known.
    pub first_name: Option<String>,
    /// The person's name, or their email.
    pub display_name: String,
    pub query: HomeQuery,
    pub tool_mode: GatewayToolMode,
    /// Every MCP, by name.
    pub mcps: Vec<Mcp>,
    pub tools: Option<ToolsExposed>,
    /// Every access token, the most recently used first.
    pub tokens: Vec<AccessToken>,
    pub now: Timestamp,
    /// Members and pending invites. Administrators only.
    pub team: Option<(i64, i64)>,
    /// Administrators only.
    pub stats: Option<Stats>,
    /// Administrators only.
    pub activity: Option<Activity>,
    pub pending_approvals: i64,
}

impl Home {
    pub fn enabled_mcps(&self) -> usize {
        self.mcps.iter().filter(|mcp| mcp.enabled).count()
    }

    /// MCPs added in the last 30 days.
    pub fn recent_mcps(&self) -> usize {
        let since = self.now - Duration::days(30);
        self.mcps
            .iter()
            .filter(|mcp| mcp.created_at >= since)
            .count()
    }

    /// Enabled MCPs that wait for someone to authorize them.
    pub fn mcps_awaiting_authorization(&self) -> Vec<&Mcp> {
        self.mcps
            .iter()
            .filter(|mcp| mcp.enabled && mcp.oauth_required)
            .collect()
    }

    /// Enabled MCPs whose last connection failed for another reason.
    pub fn mcps_in_error(&self) -> Vec<&Mcp> {
        self.mcps
            .iter()
            .filter(|mcp| mcp.enabled && !mcp.oauth_required && mcp.status == McpStatus::Error)
            .collect()
    }

    pub fn token_count(&self, state: TokenState) -> usize {
        self.tokens
            .iter()
            .filter(|token| token_state(token, self.now) == state)
            .count()
    }

    pub fn active_tokens(&self) -> usize {
        self.token_count(TokenState::Active) + self.token_count(TokenState::Expiring)
    }

    /// Active tokens that made a request in the last 7 days.
    pub fn tokens_used_recently(&self) -> usize {
        let since = self.now - Duration::days(SOON_DAYS);
        self.tokens
            .iter()
            .filter(|token| token.is_active())
            .filter(|token| token.last_used_at.is_some_and(|used_at| used_at >= since))
            .count()
    }
}

fn first_name(user: &User) -> Option<String> {
    user.full_name
        .as_deref()
        .and_then(|name| name.split_whitespace().next())
        .map(str::to_string)
}

/// Calls of the last hours or days, counted from now as the Logs page
/// counts them: its error filter lists what the errors figure counts.
async fn stats(db: &Db, range: LogRange, now: DateTime<Utc>) -> Result<Metrics, sqlx::Error> {
    let (total, errors, average): (i64, Option<i64>, Option<f64>) = sqlx::query_as(
        "select count(*), sum(case when `outcome` = 'error' then 1 else 0 end), avg(`duration_ms`) \
         from `mcp_call_logs` where `created_at` >= ?",
    )
    .bind(range.cutoff(now).unwrap_or_default())
    .fetch_one(&**db)
    .await?;
    Ok(Metrics {
        total,
        errors: errors.unwrap_or(0),
        average_duration_ms: (average.unwrap_or(0.0) + 0.5).floor() as i64,
    })
}

/// The last 14 days of calls by day of the viewer's zone, the 14 days
/// before for comparison, and the latest calls.
pub async fn activity(
    db: &Db,
    time_zone: &str,
    zone: Zone,
    logging_off: bool,
    now: DateTime<Utc>,
) -> Result<Activity, sqlx::Error> {
    let config = days_ending_today(Range::Custom, ACTIVITY_DAYS, zone, now);
    let buckets = buckets(&config);
    let days = timeline(&**db, &config, &buckets).await?;
    let before = config.start.plus_days(-(ACTIVITY_DAYS as i64));
    let previous = metrics(&**db, &before, &config.start).await?;
    let recent = sqlx::query_as(
        "select * from `mcp_call_logs` order by `created_at` desc, `id` desc limit 5",
    )
    .fetch_all(&**db)
    .await?;
    Ok(Activity {
        total: days.iter().map(|day| day.calls).sum(),
        errors: days.iter().map(|day| day.errors).sum(),
        days: buckets
            .iter()
            .zip(&days)
            .map(|(bucket, day)| Bar {
                label: bucket.start.format("%-d %b"),
                value: day.calls,
            })
            .collect(),
        previous_total: previous.total,
        recent,
        logging_off,
        time_zone: time_zone.to_string(),
    })
}

/// Everything the page shows to this person. `tool_counts` is what
/// [`AppState::known_tool_counts`] answers.
pub async fn load(
    state: &AppState,
    user: &User,
    query: HomeQuery,
    zone: Zone,
    pending_approvals: i64,
    tool_counts: &HashMap<i64, usize>,
) -> Result<Home, AppError> {
    let db = &state.core.db;
    let now = Utc::now();
    let admin = user.is_admin();
    // What a member is shown on the MCPs and Access tokens pages: all of them.
    let mcps: Vec<Mcp> = sqlx::query_as("select * from `mcps` order by `name` asc")
        .fetch_all(&**db)
        .await?;
    let tokens: Vec<AccessToken> = sqlx::query_as(
        "select * from `access_tokens` order by `last_used_at` is null, `last_used_at` desc, `created_at` desc, `id` desc",
    )
    .fetch_all(&**db)
    .await?;
    let settings = state.gateway.call_log.settings().await?;
    let logging_off = settings.mcp_log_level == McpLogLevel::Off;

    let (team, stats_of_range, activity_of_days) = if admin {
        let members: i64 = sqlx::query_scalar("select count(*) from `users`")
            .fetch_one(&**db)
            .await?;
        let invites: i64 = sqlx::query_scalar(
            "select count(*) from `invites` where `accepted_at` is null and `expires_at` > ?",
        )
        .bind(Timestamp::from(now))
        .fetch_one(&**db)
        .await?;
        let time_zone = query.time_zone.clone().unwrap_or_else(|| "UTC".into());
        (
            Some((members, invites)),
            Some(Stats {
                metrics: stats(db, query.range, now).await?,
                logging_off,
            }),
            Some(activity(db, &time_zone, zone, logging_off, now).await?),
        )
    } else {
        (None, None, None)
    };

    Ok(Home {
        admin,
        first_name: first_name(user),
        display_name: user
            .full_name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| user.email.clone()),
        query,
        tool_mode: settings.gateway_tool_mode,
        tools: tools_exposed(&mcps, tool_counts),
        mcps,
        tokens,
        now: Timestamp::from(now),
        team,
        stats: stats_of_range,
        activity: activity_of_days,
        pending_approvals,
    })
}

/// `GET /`
pub async fn show(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    let Some(user) = context.user.clone() else {
        return Err(AppError::Unauthorized);
    };
    let mut query = HomeQuery::read(&input);
    // The figures of an administrator come from the call log: it is read
    // within its retention, as the Logs and Analytics pages read it.
    if user.is_admin() {
        state.gateway.call_log.prune_expired(false).await;
    }

    // The page script names the viewer's time zone once, and the session
    // remembers it: the next visit draws the activity in their days at once.
    let remembered = session.get_as::<String>(TIME_ZONE_KEY);
    let named = query
        .time_zone
        .take()
        .filter(|name| Zone::parse(name).is_some());
    if let Some(name) = &named
        && remembered.as_ref() != Some(name)
    {
        session.put(TIME_ZONE_KEY, name);
    }
    let (time_zone, zone) = resolve_time_zone(named.or(remembered).as_deref());
    query.time_zone = Some(time_zone);

    let home = load(
        &state,
        &user,
        query,
        zone,
        context.pending_approvals,
        &state.known_tool_counts(),
    )
    .await?;

    // The answer to the time zone the page script sent: the activity card only.
    if context.is_fetch
        && fragment_target(&headers).as_deref() == Some("home-activity")
        && let Some(activity) = &home.activity
    {
        return Ok(uncached(fragment(
            StatusCode::OK,
            activity_content(activity, &home.mcps),
        )));
    }
    Ok(page(home_page(&context, &home)))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn query(input: Value) -> HomeQuery {
        HomeQuery::read(input.as_object().unwrap())
    }

    #[test]
    fn reads_what_it_can_of_the_address() {
        let defaults = HomeQuery {
            range: LogRange::Days7,
            client: Client::Claude,
            time_zone: None,
        };
        assert_eq!(query(json!({})), defaults);
        assert_eq!(
            query(json!({ "range": "24h", "client": "cursor", "timeZone": " Europe/Paris " })),
            HomeQuery {
                range: LogRange::Hours24,
                client: Client::Cursor,
                time_zone: Some("Europe/Paris".into()),
            }
        );
        // The query string of another page, which sent the person here.
        assert_eq!(
            query(
                json!({ "range": "all", "outcome": "error", "client": ["codex"], "timeZone": "" })
            ),
            defaults
        );
        assert_eq!(
            query(json!({ "range": "custom", "client": "vim" })),
            defaults
        );
    }

    #[test]
    fn counts_the_tools_of_the_enabled_mcps_it_knows() {
        let mcp = |id: i64, enabled: bool| Mcp {
            id,
            enabled,
            ..Default::default()
        };
        let mcps = [mcp(1, true), mcp(2, true), mcp(3, false), mcp(4, true)];
        let counts = HashMap::from([(1, 12), (3, 40), (4, 0), (9, 5)]);
        assert_eq!(
            tools_exposed(&mcps, &counts),
            Some(ToolsExposed { tools: 12, mcps: 2 })
        );
        assert_eq!(tools_exposed(&mcps, &HashMap::new()), None);
        assert_eq!(tools_exposed(&mcps, &HashMap::from([(3, 40)])), None);
    }

    #[test]
    fn tells_where_a_token_stands() {
        let now = Timestamp::now();
        let days = |count: i64| Some(now + Duration::days(count));
        let manual = |expires_at, revoked_at| AccessToken {
            expires_at,
            revoked_at,
            ..Default::default()
        };
        assert_eq!(token_state(&manual(None, None), now), TokenState::Active);
        assert_eq!(
            token_state(&manual(days(30), None), now),
            TokenState::Active
        );
        assert_eq!(
            token_state(&manual(days(3), None), now),
            TokenState::Expiring
        );
        assert_eq!(
            token_state(&manual(days(-1), None), now),
            TokenState::Expired
        );
        assert_eq!(
            token_state(&manual(days(3), days(-1)), now),
            TokenState::Revoked
        );

        // An OAuth connection outlives its access token for as long as it can be refreshed.
        let oauth = |refresh| AccessToken {
            source: TokenSource::Oauth,
            expires_at: Some(now + Duration::minutes(30)),
            oauth_refresh_expires_at: refresh,
            ..Default::default()
        };
        assert_eq!(token_state(&oauth(days(30)), now), TokenState::Active);
        assert_eq!(token_state(&oauth(days(2)), now), TokenState::Expiring);
        assert_eq!(token_state(&oauth(days(-2)), now), TokenState::Expired);
        assert_eq!(token_state(&oauth(None), now), TokenState::Expiring);
    }

    #[test]
    fn compares_the_activity_with_the_days_before() {
        let activity = |total, previous_total| Activity {
            days: Vec::new(),
            total,
            errors: 0,
            previous_total,
            recent: Vec::new(),
            logging_off: false,
            time_zone: "UTC".into(),
        };
        assert_eq!(activity(112, 100).change(), Some(12));
        assert_eq!(activity(50, 100).change(), Some(-50));
        assert_eq!(activity(100, 100).change(), Some(0));
        assert_eq!(activity(0, 3).change(), Some(-100));
        assert_eq!(activity(7, 3).change(), Some(133));
        assert_eq!(activity(5, 0).change(), None);
    }
}
