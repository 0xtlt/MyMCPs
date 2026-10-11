//! MCP analytics: how much the gateway is used, how reliably and how fast,
//! over a period cut in the viewer's time zone. (`analytics_controller.ts`)
//!
//! The Node app computed its periods with Luxon. [`ZonedTime`] does what
//! Luxon's `DateTime` did for it, to the second: days are added on the
//! calendar of the zone, hours on the clock, and a local time that a clock
//! change skips or repeats is resolved the way Luxon resolves it.

use std::sync::LazyLock;

use axum::Router;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use chrono::{
    DateTime, Duration, NaiveDate, NaiveDateTime, Offset, TimeZone, Timelike, Utc, Weekday,
};
use chrono_tz::Tz;
use http::{HeaderMap, StatusCode};
use mymcps_core::Timestamp;
use mymcps_core::models::McpLogLevel;
use mymcps_gateway::validators::mcp_call_log::ANALYTICS_QUERY;
use mymcps_vine as vine;
use serde::Deserialize;
use serde_json::Value;
use sqlx::sqlite::SqliteExecutor;

use crate::error::AppError;
use crate::forms::{FormState, refusal};
use crate::input::Input;
use crate::redirect::redirect_back;
use crate::respond::{fragment, navigate, page, page_with_status};
use crate::routes::FeatureRoutes;
use crate::routes::logs::link;
use crate::session::Session;
use crate::state::AppState;
use crate::views::analytics::{analytics_page, custom_range_form};
use crate::views::charts::LinePoint;
use crate::views::shell::PageContext;

pub fn routes() -> FeatureRoutes {
    FeatureRoutes {
        admin: Router::new().route("/analytics", get(index)),
        ..Default::default()
    }
}

// ------------------------------------------------------------- time zones

/// A time zone a viewer can be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    /// `UTC` and `GMT`.
    Utc,
    /// An offset written as a zone, such as `+02:00`: seconds east of UTC.
    Fixed(i32),
    /// A zone of the IANA database.
    Named(Tz),
}

impl Zone {
    /// The zone of this name, as the Node app's `Intl` knew it: a name of
    /// the IANA database in any case, or an offset (`+02`, `+0200`, `+02:00`).
    pub fn parse(name: &str) -> Option<Self> {
        if name.eq_ignore_ascii_case("utc") || name.eq_ignore_ascii_case("gmt") {
            return Some(Self::Utc);
        }
        if let Some(seconds) = offset_zone(name) {
            return Some(Self::Fixed(seconds));
        }
        name.parse::<Tz>().ok().map(Self::Named).or_else(|| {
            chrono_tz::TZ_VARIANTS
                .iter()
                .find(|zone| zone.name().eq_ignore_ascii_case(name))
                .copied()
                .map(Self::Named)
        })
    }

    /// Seconds east of UTC at this instant.
    pub fn offset_at(&self, seconds: i64) -> i32 {
        match self {
            Self::Utc => 0,
            Self::Fixed(offset) => *offset,
            Self::Named(zone) => DateTime::from_timestamp(seconds, 0)
                .map(|instant| {
                    zone.offset_from_utc_datetime(&instant.naive_utc())
                        .fix()
                        .local_minus_utc()
                })
                .unwrap_or(0),
        }
    }
}

/// `+02`, `+0200` or `+02:00` as seconds east of UTC.
fn offset_zone(name: &str) -> Option<i32> {
    if !name.is_ascii() {
        return None;
    }
    let (sign, digits) = match name.as_bytes().first()? {
        b'+' => (1, &name[1..]),
        b'-' => (-1, &name[1..]),
        _ => return None,
    };
    let (hours, minutes) = match digits.len() {
        2 => (digits, "00"),
        4 => digits.split_at(2),
        5 if digits.as_bytes()[2] == b':' => (&digits[..2], &digits[3..]),
        _ => return None,
    };
    if !hours
        .bytes()
        .chain(minutes.bytes())
        .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let hours: i32 = hours.parse().ok().filter(|hours| *hours <= 23)?;
    let minutes: i32 = minutes.parse().ok().filter(|minutes| *minutes <= 59)?;
    Some(sign * (hours * 3600 + minutes * 60))
}

/// The zone the page is drawn in: the one the browser named when it is one,
/// UTC otherwise. Its name is kept as it was sent.
pub fn resolve_time_zone(time_zone: Option<&str>) -> (String, Zone) {
    time_zone
        .filter(|name| !name.is_empty())
        .and_then(|name| Zone::parse(name).map(|zone| (name.to_string(), zone)))
        .unwrap_or_else(|| ("UTC".to_string(), Zone::Utc))
}

/// `+02:00`, `-03:30`.
fn offset_text(offset: i32) -> String {
    let minutes = offset.abs() / 60;
    format!(
        "{}{:02}:{:02}",
        if offset < 0 { '-' } else { '+' },
        minutes / 60,
        minutes % 60
    )
}

/// The zone as the pages name it: `Europe/Paris (UTC+02:00)`.
pub fn time_zone_label(name: &str, zone: Zone, now: DateTime<Utc>) -> String {
    format!(
        "{name} (UTC{})",
        offset_text(zone.offset_at(now.timestamp()))
    )
}

/// An instant, with the offset its zone has at that instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZonedTime {
    /// Seconds since the Unix epoch.
    seconds: i64,
    /// Seconds east of UTC.
    offset: i32,
    zone: Zone,
}

impl ZonedTime {
    pub fn at(seconds: i64, zone: Zone) -> Self {
        Self {
            seconds,
            offset: zone.offset_at(seconds),
            zone,
        }
    }

    /// The instant a local time stands for. `offset` is a first guess, and
    /// settles which instant a local time that happens twice stands for; a
    /// local time a clock change skips moves by the size of the change.
    /// (Luxon's `fixOffset`.)
    fn from_local(local: NaiveDateTime, offset: i32, zone: Zone) -> Self {
        let local = local.and_utc().timestamp();
        let guess = local - i64::from(offset);
        let second = zone.offset_at(guess);
        if second == offset {
            return Self {
                seconds: guess,
                offset,
                zone,
            };
        }
        let guess = guess - i64::from(second - offset);
        let third = zone.offset_at(guess);
        if second == third {
            return Self {
                seconds: guess,
                offset: second,
                zone,
            };
        }
        Self {
            seconds: local - i64::from(second.min(third)),
            offset: second.max(third),
            zone,
        }
    }

    pub fn seconds(&self) -> i64 {
        self.seconds
    }

    pub fn utc(&self) -> DateTime<Utc> {
        DateTime::from_timestamp(self.seconds, 0).unwrap_or_default()
    }

    /// The date and time a clock of the zone shows.
    pub fn local(&self) -> NaiveDateTime {
        DateTime::from_timestamp(self.seconds + i64::from(self.offset), 0)
            .unwrap_or_default()
            .naive_utc()
    }

    fn start_of_day(&self) -> Self {
        let midnight = self.local().date().and_hms_opt(0, 0, 0).unwrap_or_default();
        Self::from_local(midnight, self.offset, self.zone)
    }

    fn start_of_hour(&self) -> Self {
        let local = self.local();
        let seconds = i64::from(local.minute()) * 60 + i64::from(local.second());
        Self::from_local(local - Duration::seconds(seconds), self.offset, self.zone)
    }

    /// The same time of day, this many days later on the calendar of the zone.
    pub fn plus_days(&self, days: i64) -> Self {
        match self.local().checked_add_signed(Duration::days(days)) {
            Some(local) => Self::from_local(local, self.offset, self.zone),
            None => *self,
        }
    }

    pub fn plus_hours(&self, hours: i64) -> Self {
        Self::at(self.seconds + hours * 3600, self.zone)
    }

    /// The time as text, with a `chrono` format.
    pub fn format(&self, format: &str) -> String {
        self.local().format(format).to_string()
    }

    /// ISO 8601 to the minute when the seconds are zero, with the offset of
    /// the zone: `2026-03-29T01:30+01:00`.
    pub fn to_iso(&self) -> String {
        let local = self.local();
        let time = if local.second() == 0 {
            local.format("%Y-%m-%dT%H:%M")
        } else {
            local.format("%Y-%m-%dT%H:%M:%S")
        };
        match self.zone {
            Zone::Utc => format!("{time}Z"),
            _ => format!("{time}{}", offset_text(self.offset)),
        }
    }

    /// The instant in UTC, as the custom range travels in the address.
    pub fn to_utc_iso(&self) -> String {
        Self::at(self.seconds, Zone::Utc).to_iso()
    }
}

// -------------------------------------------------- ISO 8601, as Luxon reads it

fn static_regex(source: &str, flags: &str) -> vine::JsRegex {
    vine::js::regex(source, flags).expect("a valid static pattern")
}

const ISO_OFFSET: &str = r"(?:([Zz])|([+-]\d\d)(?::?(\d\d))?)";
const ISO_TIME: &str = r"(\d\d)(?::?(\d\d)(?::?(\d\d)(?:[.,](\d{1,30}))?)?)?";

/// A date in one of the three ISO forms, then its time and offset.
fn iso_date_pattern(date: &str) -> vine::JsRegex {
    static_regex(&format!("^{date}(?:[Tt]{ISO_TIME}{ISO_OFFSET}?)?$"), "")
}

static ISO_YMD: LazyLock<vine::JsRegex> =
    LazyLock::new(|| iso_date_pattern(r"([+-]\d{6}|\d{4})(?:-?(\d\d)(?:-?(\d\d))?)?"));
static ISO_WEEK: LazyLock<vine::JsRegex> =
    LazyLock::new(|| iso_date_pattern(r"(\d{4})-?W(\d\d)(?:-?(\d))?"));
static ISO_ORDINAL: LazyLock<vine::JsRegex> =
    LazyLock::new(|| iso_date_pattern(r"(\d{4})-?(\d{3})"));
static ISO_TIME_ONLY: LazyLock<vine::JsRegex> =
    LazyLock::new(|| static_regex(&format!("^{ISO_TIME}{ISO_OFFSET}?$"), ""));
static EXPLICIT_OFFSET: LazyLock<vine::JsRegex> =
    LazyLock::new(|| static_regex(r"(?:Z|[+-]\d{2}:\d{2})$", "i"));
static SECOND_FRACTION: LazyLock<vine::JsRegex> =
    LazyLock::new(|| static_regex(r":\d{2}\.([0-9]+)(?:Z|[+-]\d{2}:\d{2})$", "i"));

/// A fraction of a second the page cannot keep: the log is to the second.
fn has_unsupported_fraction(value: &str) -> bool {
    SECOND_FRACTION
        .as_regex()
        .captures(value)
        .and_then(|captures| captures.get(1))
        .is_some_and(|fraction| fraction.as_str().bytes().any(|digit| digit != b'0'))
}

/// The groups of a pattern in a value, when it matches: `None` for a group
/// that took no part in the match.
fn groups<'v>(pattern: &vine::JsRegex, value: &'v str) -> Option<Vec<Option<&'v str>>> {
    let captures = pattern.as_regex().captures(value)?;
    Some(
        captures
            .iter()
            .map(|group| group.map(|group| group.as_str()))
            .collect(),
    )
}

/// `DateTime.fromISO(value, { setZone: true })` for a value that names its
/// offset: the instant in seconds, and what is left of it in milliseconds.
/// A time without a date is today's, on the clock of its own offset.
fn parse_iso_instant(value: &str, now: DateTime<Utc>) -> Option<(i64, u32)> {
    fn text<'v>(groups: &[Option<&'v str>], index: usize) -> Option<&'v str> {
        groups.get(index).copied().flatten()
    }
    fn number(groups: &[Option<&str>], index: usize, fallback: u32) -> Option<u32> {
        match text(groups, index) {
            Some(digits) => digits.parse().ok(),
            None => Some(fallback),
        }
    }
    let year = |groups: &[Option<&str>]| text(groups, 1)?.parse::<i32>().ok();

    // The date, then the index of the first group of the time.
    let (groups, date, time) = if let Some(groups) = groups(&ISO_YMD, value) {
        let date = NaiveDate::from_ymd_opt(
            year(&groups)?,
            number(&groups, 2, 1)?,
            number(&groups, 3, 1)?,
        )?;
        (groups, Some(date), 4)
    } else if let Some(groups) = groups(&ISO_WEEK, value) {
        let weekday = match number(&groups, 3, 1)? {
            1 => Weekday::Mon,
            2 => Weekday::Tue,
            3 => Weekday::Wed,
            4 => Weekday::Thu,
            5 => Weekday::Fri,
            6 => Weekday::Sat,
            7 => Weekday::Sun,
            _ => return None,
        };
        let date = NaiveDate::from_isoywd_opt(year(&groups)?, number(&groups, 2, 1)?, weekday)?;
        (groups, Some(date), 4)
    } else if let Some(groups) = groups(&ISO_ORDINAL, value) {
        let date = NaiveDate::from_yo_opt(year(&groups)?, number(&groups, 2, 1)?)?;
        (groups, Some(date), 3)
    } else {
        (groups(&ISO_TIME_ONLY, value)?, None, 1)
    };

    let offset = if text(&groups, time + 4).is_some() {
        0
    } else {
        // The sign of the hours is also the sign of the minutes, for `-00:30` too.
        let hours = text(&groups, time + 5)?;
        let minutes = i64::from(number(&groups, time + 6, 0)?);
        let sign = if hours.starts_with('-') { -1 } else { 1 };
        sign * (hours[1..].parse::<i64>().ok()? * 3600 + minutes * 60)
    };

    let hour = number(&groups, time, 0)?;
    let minute = number(&groups, time + 1, 0)?;
    let second = number(&groups, time + 2, 0)?;
    let milliseconds = match text(&groups, time + 3) {
        Some(fraction) => (format!("0.{fraction}").parse::<f64>().ok()? * 1000.0).floor() as u32,
        None => 0,
    };
    let midnight_after = hour == 24 && minute == 0 && second == 0 && milliseconds == 0;
    if (hour > 23 && !midnight_after) || minute > 59 || second > 59 || milliseconds > 999 {
        return None;
    }

    let date = match date {
        Some(date) => date,
        None => DateTime::from_timestamp(now.timestamp() + offset, 0)?.date_naive(),
    };
    let local = date.and_hms_opt(0, 0, 0)?.and_utc().timestamp()
        + i64::from(hour) * 3600
        + i64::from(minute) * 60
        + i64::from(second);
    // Out of what a date can hold: not an instant.
    DateTime::from_timestamp(local - offset, 0)?;
    Some((local - offset, milliseconds))
}

// ------------------------------------------------------------- the period

/// The period the page covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    Hours24,
    Days7,
    Days30,
    Custom,
}

impl Range {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hours24 => "24h",
            Self::Days7 => "7d",
            Self::Days30 => "30d",
            Self::Custom => "custom",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Self::Hours24, Self::Days7, Self::Days30, Self::Custom]
            .into_iter()
            .find(|range| range.as_str() == value)
    }
}

/// What one point of the timeline spans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Hour,
    Day,
}

impl Unit {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hour => "hour",
            Self::Day => "day",
        }
    }
}

/// A period, and how its timeline is cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RangeConfig {
    pub range: Range,
    pub start: ZonedTime,
    /// Excluded.
    pub end: ZonedTime,
    pub unit: Unit,
    pub count: usize,
}

/// This many days of the zone's calendar, today being the last one.
pub fn days_ending_today(
    range: Range,
    count: usize,
    zone: Zone,
    now: DateTime<Utc>,
) -> RangeConfig {
    let today = ZonedTime::at(now.timestamp(), zone).start_of_day();
    let start = today.plus_days(1 - count as i64);
    RangeConfig {
        range,
        start,
        end: start.plus_days(count as i64),
        unit: Unit::Day,
        count,
    }
}

/// The last 24 hours by the hour, or the last 7 or 30 days by the day, the
/// current hour or day included.
pub fn preset_range_config(range: Range, zone: Zone, now: DateTime<Utc>) -> RangeConfig {
    match range {
        Range::Hours24 => {
            let start = ZonedTime::at(now.timestamp(), zone)
                .start_of_hour()
                .plus_hours(-23);
            RangeConfig {
                range,
                start,
                end: start.plus_hours(24),
                unit: Unit::Hour,
                count: 24,
            }
        }
        Range::Days30 => days_ending_today(range, 30, zone, now),
        Range::Days7 | Range::Custom => days_ending_today(Range::Days7, 7, zone, now),
    }
}

/// The two instants of a custom range, when the page can use them: each
/// names its offset and falls on a whole second, the end is after the start
/// and at most 365 days after it.
fn custom_bounds(
    zone: Zone,
    start_input: &str,
    end_input: &str,
    now: DateTime<Utc>,
) -> Option<(ZonedTime, ZonedTime)> {
    for input in [start_input, end_input] {
        if !EXPLICIT_OFFSET.test(input) || has_unsupported_fraction(input) {
            return None;
        }
    }
    let (start, start_milliseconds) = parse_iso_instant(start_input, now)?;
    let (end, end_milliseconds) = parse_iso_instant(end_input, now)?;
    let start = ZonedTime::at(start, zone);
    let end = ZonedTime::at(end, zone);
    let usable = start_milliseconds == 0
        && end_milliseconds == 0
        && end.seconds > start.seconds
        && end.seconds <= start.plus_days(365).seconds;
    usable.then_some((start, end))
}

/// The period that was asked for. A custom range the page cannot use is
/// answered with the last 7 days.
pub fn range_config(
    range: Range,
    zone: Zone,
    start_input: Option<&str>,
    end_input: Option<&str>,
    now: DateTime<Utc>,
) -> RangeConfig {
    let bounds = match (range, start_input, end_input) {
        (Range::Custom, Some(start), Some(end)) if !start.is_empty() && !end.is_empty() => {
            custom_bounds(zone, start, end, now)
        }
        (Range::Custom, ..) => None,
        _ => return preset_range_config(range, zone, now),
    };
    let Some((start, end)) = bounds else {
        return preset_range_config(Range::Days7, zone, now);
    };

    let duration = end.seconds - start.seconds;
    // Up to two days are shown by the hour.
    if duration <= 48 * 3600 {
        return RangeConfig {
            range,
            start,
            end,
            unit: Unit::Hour,
            count: usize::try_from((duration + 3599) / 3600)
                .unwrap_or(1)
                .max(1),
        };
    }
    // As many days of the zone's calendar as it takes to reach the end.
    let mut count = 1;
    while start.plus_days(count).seconds < end.seconds && count < 367 {
        count += 1;
    }
    RangeConfig {
        range,
        start,
        end,
        unit: Unit::Day,
        count: count as usize,
    }
}

/// One point of the timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bucket {
    pub start: ZonedTime,
    /// Excluded.
    pub end: ZonedTime,
    pub label: String,
}

pub fn buckets(config: &RangeConfig) -> Vec<Bucket> {
    (0..config.count as i64)
        .map(|index| {
            let (start, candidate_end, label) = match config.unit {
                Unit::Hour => {
                    let start = config.start.plus_hours(index);
                    (start, start.plus_hours(1), start.format("%H:%M"))
                }
                Unit::Day => {
                    let start = config.start.plus_days(index);
                    (start, start.plus_days(1), start.format("%b %-d"))
                }
            };
            Bucket {
                start,
                end: if candidate_end.seconds < config.end.seconds {
                    candidate_end
                } else {
                    config.end
                },
                label,
            }
        })
        .collect()
}

// ------------------------------------------------------------ the figures

/// Calls, errors and latency of a period.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Metrics {
    pub total: i64,
    pub errors: i64,
    pub average_duration_ms: i64,
}

impl Metrics {
    pub fn successes(&self) -> i64 {
        self.total - self.errors
    }

    pub fn success_rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.successes() as f64 / self.total as f64) * 100.0
        }
    }

    pub fn error_rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            (self.errors as f64 / self.total as f64) * 100.0
        }
    }
}

/// One line of a ranking: an MCP, a tool or an access token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breakdown {
    pub label: String,
    pub total: i64,
    pub errors: i64,
    pub average_duration_ms: i64,
}

/// `Math.round` of an average the database computed.
fn rounded(average: Option<f64>) -> i64 {
    (average.unwrap_or(0.0) + 0.5).floor() as i64
}

fn sql_time(time: &ZonedTime) -> Timestamp {
    Timestamp::from(time.utc())
}

const ERROR_COUNT_SQL: &str = "sum(case when `outcome` = 'error' then 1 else 0 end)";

/// Calls, errors and average duration from `start` to `end`, excluded.
pub async fn metrics<'e, E>(
    db: E,
    start: &ZonedTime,
    end: &ZonedTime,
) -> Result<Metrics, sqlx::Error>
where
    E: SqliteExecutor<'e>,
{
    let (total, errors, average): (i64, Option<i64>, Option<f64>) = sqlx::query_as(
        "select count(*), sum(case when `outcome` = 'error' then 1 else 0 end), avg(`duration_ms`) \
         from `mcp_call_logs` where `created_at` >= ? and `created_at` < ?",
    )
    .bind(sql_time(start))
    .bind(sql_time(end))
    .fetch_one(db)
    .await?;
    Ok(Metrics {
        total,
        errors: errors.unwrap_or(0),
        average_duration_ms: rounded(average),
    })
}

/// Calls and errors of each bucket, in the order of the buckets. One
/// statement sorts the calls of the whole period into them.
pub async fn timeline<'e, E>(
    db: E,
    config: &RangeConfig,
    buckets: &[Bucket],
) -> Result<Vec<LinePoint>, sqlx::Error>
where
    E: SqliteExecutor<'e>,
{
    let mut counts = vec![(0, 0); buckets.len()];
    if !buckets.is_empty() {
        let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new("select case");
        for (index, bucket) in buckets.iter().enumerate() {
            query.push(" when `created_at` >= ");
            query.push_bind(sql_time(&bucket.start));
            query.push(" and `created_at` < ");
            query.push_bind(sql_time(&bucket.end));
            query.push(" then ");
            query.push_bind(index as i64);
        }
        query.push(" end as `bucket`, count(*), ");
        query.push(ERROR_COUNT_SQL);
        query.push(" from `mcp_call_logs` where `created_at` >= ");
        query.push_bind(sql_time(&config.start));
        query.push(" and `created_at` < ");
        query.push_bind(sql_time(&config.end));
        query.push(" group by `bucket`");
        let rows: Vec<(Option<i64>, i64, Option<i64>)> =
            query.build_query_as().fetch_all(db).await?;
        for (bucket, total, errors) in rows {
            let slot = bucket
                .and_then(|index| usize::try_from(index).ok())
                .and_then(|index| counts.get_mut(index));
            if let Some(slot) = slot {
                *slot = (total, errors.unwrap_or(0));
            }
        }
    }
    Ok(buckets
        .iter()
        .zip(counts)
        .map(|(bucket, (calls, errors))| LinePoint {
            label: bucket.label.clone(),
            calls,
            errors,
        })
        .collect())
}

/// The five busiest values of a label over the period.
async fn breakdown<'e, E>(
    db: E,
    label_sql: &'static str,
    config: &RangeConfig,
) -> Result<Vec<Breakdown>, sqlx::Error>
where
    E: SqliteExecutor<'e>,
{
    // `label_sql` is one of the three expressions below, never input.
    let sql = format!(
        "select {label_sql} as `label`, count(*) as `total`, {ERROR_COUNT_SQL} as `errors`, avg(`duration_ms`) \
         from `mcp_call_logs` where `created_at` >= ? and `created_at` < ? \
         group by {label_sql} order by `total` desc, `label` asc limit 5"
    );
    let rows: Vec<(String, i64, Option<i64>, Option<f64>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(sql_time(&config.start))
            .bind(sql_time(&config.end))
            .fetch_all(db)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(label, total, errors, average)| Breakdown {
            label,
            total,
            errors: errors.unwrap_or(0),
            average_duration_ms: rounded(average),
        })
        .collect())
}

// ------------------------------------------------------- the custom range

/// The fields of the custom range dialog: dates and times on the clock of
/// the zone the page is drawn in.
static CUSTOM_RANGE_FORM: LazyLock<vine::Validator> = LazyLock::new(|| {
    vine::global().create(vine::object! {
        "startDate" => vine::string().trim().max_length(40).optional(),
        "startTime" => vine::string().trim().max_length(40).optional(),
        "endDate" => vine::string().trim().max_length(40).optional(),
        "endTime" => vine::string().trim().max_length(40).optional(),
    })
});

const CUSTOM_RANGE_FIELDS: [&str; 4] = ["startDate", "startTime", "endDate", "endTime"];

/// The dates and times in the fields of the custom range dialog.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct CustomRangeFields {
    pub start_date: Option<String>,
    pub start_time: Option<String>,
    pub end_date: Option<String>,
    pub end_time: Option<String>,
}

impl CustomRangeFields {
    /// The fields showing a period.
    fn of(start: &ZonedTime, end: &ZonedTime) -> Self {
        Self {
            start_date: Some(start.format("%Y-%m-%d")),
            start_time: Some(start.format("%H:%M")),
            end_date: Some(end.format("%Y-%m-%d")),
            end_time: Some(end.format("%H:%M")),
        }
    }
}

/// Which of the two instants a local time that happens twice stands for.
#[derive(Clone, Copy, PartialEq)]
enum Boundary {
    Start,
    End,
}

const LOCAL_MINUTE: &str = "%Y-%m-%dT%H:%M";

/// The instant a date and a time of the dialog stand for. `source` is the
/// instant the field showed: left as it was, it is kept, which matters for
/// a time that happens twice. A time that is typed again and happens twice
/// is the earlier one for a start and the later one for an end.
fn selected_instant(
    date: &str,
    time: &str,
    zone: Zone,
    source: Option<ZonedTime>,
    boundary: Boundary,
) -> Option<ZonedTime> {
    let value = format!("{date}T{time}");
    if let Some(source) = source.filter(|source| source.format(LOCAL_MINUTE) == value) {
        return Some(source);
    }
    let local = NaiveDateTime::parse_from_str(&value, LOCAL_MINUTE).ok()?;
    let instant = match zone {
        Zone::Utc => ZonedTime::at(local.and_utc().timestamp(), zone),
        Zone::Fixed(offset) => ZonedTime::at(local.and_utc().timestamp() - i64::from(offset), zone),
        Zone::Named(named) => {
            let candidates = named.from_local_datetime(&local);
            let chosen = match boundary {
                Boundary::Start => candidates.earliest(),
                Boundary::End => candidates.latest(),
            }?;
            ZonedTime::at(chosen.timestamp(), zone)
        }
    };
    // A time the clock skips, or a date that is not one, does not read back as it was typed.
    (instant.format(LOCAL_MINUTE) == value).then_some(instant)
}

/// The range the dialog asks for, or what to tell the person.
fn custom_range(
    fields: &CustomRangeFields,
    zone: Zone,
    source: Option<(ZonedTime, ZonedTime)>,
) -> Result<(ZonedTime, ZonedTime), &'static str> {
    let (Some(start_date), Some(start_time), Some(end_date), Some(end_time)) = (
        &fields.start_date,
        &fields.start_time,
        &fields.end_date,
        &fields.end_time,
    ) else {
        return Err("Choose both a start and an end time.");
    };
    let start = selected_instant(
        start_date,
        start_time,
        zone,
        source.map(|(start, _)| start),
        Boundary::Start,
    );
    let end = selected_instant(
        end_date,
        end_time,
        zone,
        source.map(|(_, end)| end),
        Boundary::End,
    );
    let (Some(start), Some(end)) = (start, end) else {
        return Err("Enter valid dates and times.");
    };
    if end.seconds <= start.seconds {
        return Err("End time must be after start time.");
    }
    if end.seconds > start.plus_days(365).seconds {
        return Err("Custom ranges can span up to 365 days.");
    }
    Ok((start, end))
}

// --------------------------------------------------------------- the page

/// What the Analytics page shows.
#[derive(Debug, Clone)]
pub struct Analytics {
    pub config: RangeConfig,
    /// The address of the period shown.
    pub address: String,
    /// The name of the zone, as the browser sent it.
    pub time_zone: String,
    pub time_zone_label: String,
    pub logging_off: bool,
    pub metrics: Metrics,
    pub timeline: Vec<LinePoint>,
    pub top_mcps: Vec<Breakdown>,
    pub top_tools: Vec<Breakdown>,
    pub top_tokens: Vec<Breakdown>,
    /// The custom range dialog: its fields, and why they were refused.
    pub custom: CustomRangeFields,
    pub custom_error: Option<&'static str>,
    /// Whether the page opens on the dialog: a custom range was asked for
    /// and there is none yet, or the one that was typed is refused.
    pub custom_open: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AnalyticsQuery {
    range: Option<String>,
    start: Option<String>,
    end: Option<String>,
    time_zone: Option<String>,
}

/// The address of a custom range: its two instants, in UTC.
fn custom_range_address(time_zone: &str, start: &ZonedTime, end: &ZonedTime) -> String {
    link(
        "/analytics",
        &[
            ("range", "custom"),
            ("start", &start.to_utc_iso()),
            ("end", &end.to_utc_iso()),
            ("timeZone", time_zone),
        ],
    )
}

/// `GET /analytics`
pub async fn index(
    State(state): State<AppState>,
    context: PageContext,
    session: Session,
    headers: HeaderMap,
    Input(input): Input,
) -> Result<Response, AppError> {
    state.gateway.call_log.prune_expired(false).await;

    let filters: AnalyticsQuery = match ANALYTICS_QUERY.validate_as(&Value::Object(input.clone())) {
        Ok(filters) => filters,
        Err(error) => {
            let error = refusal(error)?;
            FormState::new(&error, &input).flash(&session);
            return Ok(redirect_back(&headers, "/"));
        }
    };
    let requested = filters
        .range
        .as_deref()
        .and_then(Range::parse)
        .unwrap_or(Range::Days7);
    let (time_zone, zone) = resolve_time_zone(filters.time_zone.as_deref());
    let now = Utc::now();
    let config = range_config(
        requested,
        zone,
        filters.start.as_deref(),
        filters.end.as_deref(),
        now,
    );
    let time_zone_label = time_zone_label(&time_zone, zone, now);

    // The dialog sends the dates and times of its fields: they become the
    // two instants of the address, or come back with what is wrong.
    let mut custom = CustomRangeFields::of(&config.start, &config.end);
    let mut custom_error = None;
    if CUSTOM_RANGE_FIELDS
        .iter()
        .any(|field| input.contains_key(*field))
    {
        // Fields that are not text are read as missing.
        custom = CUSTOM_RANGE_FORM
            .validate_as(&Value::Object(input.clone()))
            .unwrap_or_default();
        // The range the dialog was opened on, to keep what was not changed.
        let source = (config.range == Range::Custom).then_some((config.start, config.end));
        match custom_range(&custom, zone, source) {
            Ok((start, end)) => {
                return Ok(navigate(
                    &headers,
                    &custom_range_address(&time_zone, &start, &end),
                ));
            }
            Err(message) if context.is_fetch => {
                return Ok(fragment(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    custom_range_form(
                        &config,
                        &time_zone,
                        &time_zone_label,
                        &custom,
                        Some(message),
                    ),
                ));
            }
            Err(message) => custom_error = Some(message),
        }
    }

    let db = &*state.core.db;
    let buckets = buckets(&config);
    let settings = state.gateway.call_log.settings().await?;
    let analytics = Analytics {
        metrics: metrics(db, &config.start, &config.end).await?,
        timeline: timeline(db, &config, &buckets).await?,
        top_mcps: breakdown(
            db,
            "coalesce(`mcp_name`, `mcp_slug`, 'Unknown MCP')",
            &config,
        )
        .await?,
        top_tools: breakdown(db, "coalesce(`tool_name`, `requested_tool_name`)", &config).await?,
        top_tokens: breakdown(db, "`access_token_name`", &config).await?,
        address: match config.range {
            Range::Custom => custom_range_address(&time_zone, &config.start, &config.end),
            preset => link(
                "/analytics",
                &[("range", preset.as_str()), ("timeZone", &time_zone)],
            ),
        },
        config,
        time_zone,
        time_zone_label,
        logging_off: settings.mcp_log_level == McpLogLevel::Off,
        custom,
        custom_error,
        custom_open: custom_error.is_some()
            || (requested == Range::Custom && config.range != Range::Custom),
    };

    let markup = analytics_page(&context, &analytics);
    Ok(if custom_error.is_some() {
        page_with_status(StatusCode::UNPROCESSABLE_ENTITY, markup)
    } else {
        page(markup)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(iso: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(iso)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn knows_the_zones_a_browser_names() {
        assert_eq!(Zone::parse("UTC"), Some(Zone::Utc));
        assert_eq!(Zone::parse("gmt"), Some(Zone::Utc));
        assert_eq!(
            Zone::parse("Europe/Paris"),
            Some(Zone::Named(Tz::Europe__Paris))
        );
        assert_eq!(
            Zone::parse("europe/paris"),
            Some(Zone::Named(Tz::Europe__Paris))
        );
        assert_eq!(
            Zone::parse("US/Pacific"),
            Some(Zone::Named(Tz::US__Pacific))
        );
        assert_eq!(Zone::parse("+02:00"), Some(Zone::Fixed(7200)));
        assert_eq!(Zone::parse("-0330"), Some(Zone::Fixed(-12600)));
        assert_eq!(Zone::parse("+14"), Some(Zone::Fixed(50400)));
        for unknown in [
            "",
            "Mars/Olympus",
            " Europe/Paris",
            "UTC+2",
            "Z",
            "local",
            "system",
            "+2",
            "+24:00",
            "+02:60",
            "+02:0",
            "02:00",
            "+0២:00",
        ] {
            assert_eq!(Zone::parse(unknown), None, "{unknown:?}");
        }

        assert_eq!(
            resolve_time_zone(Some("europe/paris")),
            ("europe/paris".to_string(), Zone::Named(Tz::Europe__Paris))
        );
        assert_eq!(
            resolve_time_zone(Some("Mars/Olympus")),
            ("UTC".to_string(), Zone::Utc)
        );
        assert_eq!(resolve_time_zone(None), ("UTC".to_string(), Zone::Utc));
    }

    #[test]
    fn names_a_zone_with_its_offset() {
        let summer = at("2026-07-01T12:00:00Z");
        let winter = at("2026-01-15T12:00:00Z");
        let paris = Zone::parse("Europe/Paris").unwrap();
        assert_eq!(
            time_zone_label("Europe/Paris", paris, summer),
            "Europe/Paris (UTC+02:00)"
        );
        assert_eq!(
            time_zone_label("Europe/Paris", paris, winter),
            "Europe/Paris (UTC+01:00)"
        );
        assert_eq!(time_zone_label("UTC", Zone::Utc, summer), "UTC (UTC+00:00)");
        assert_eq!(
            time_zone_label(
                "America/St_Johns",
                Zone::parse("America/St_Johns").unwrap(),
                winter
            ),
            "America/St_Johns (UTC-03:30)"
        );
        assert_eq!(
            time_zone_label(
                "Asia/Kathmandu",
                Zone::parse("Asia/Kathmandu").unwrap(),
                winter
            ),
            "Asia/Kathmandu (UTC+05:45)"
        );
    }

    #[test]
    fn reads_the_dialog_on_the_clock_of_the_zone() {
        let paris = Zone::parse("Europe/Paris").unwrap();
        let fields = |start_date: &str, start_time: &str, end_date: &str, end_time: &str| {
            CustomRangeFields {
                start_date: Some(start_date.into()),
                start_time: Some(start_time.into()),
                end_date: Some(end_date.into()),
                end_time: Some(end_time.into()),
            }
        };
        let range = |fields: &CustomRangeFields, source| {
            custom_range(fields, paris, source)
                .map(|(start, end)| (start.to_utc_iso(), end.to_utc_iso()))
        };

        assert_eq!(
            range(&fields("2026-10-01", "00:00", "2026-10-08", "00:00"), None),
            Ok(("2026-09-30T22:00Z".into(), "2026-10-07T22:00Z".into()))
        );
        // 02:30 happens twice on the day the clocks go back: the first one
        // starts a range, the second one ends it.
        let folded = fields("2026-10-25", "02:30", "2026-10-25", "02:30");
        assert_eq!(
            range(&folded, None),
            Ok(("2026-10-25T00:30Z".into(), "2026-10-25T01:30Z".into()))
        );
        // Unless the dialog was opened on the other one and left as it was.
        let later = ZonedTime::at(at("2026-10-25T01:30:00Z").timestamp(), paris);
        let after = ZonedTime::at(at("2026-10-25T03:00:00Z").timestamp(), paris);
        assert_eq!(
            range(
                &fields("2026-10-25", "02:30", "2026-10-25", "04:00"),
                Some((later, after))
            ),
            Ok(("2026-10-25T01:30Z".into(), "2026-10-25T03:00Z".into()))
        );

        for (fields, message) in [
            (
                CustomRangeFields::default(),
                "Choose both a start and an end time.",
            ),
            (
                CustomRangeFields {
                    end_time: None,
                    ..fields("2026-10-01", "00:00", "2026-10-08", "00:00")
                },
                "Choose both a start and an end time.",
            ),
            // 02:30 does not exist on the day the clocks go forward.
            (
                fields("2026-03-29", "02:30", "2026-03-29", "04:00"),
                "Enter valid dates and times.",
            ),
            (
                fields("2026-02-30", "10:00", "2026-03-29", "04:00"),
                "Enter valid dates and times.",
            ),
            (
                fields("2026-10-01", "10:00:30", "2026-10-02", "04:00"),
                "Enter valid dates and times.",
            ),
            (
                fields("26-10-01", "10:00", "2026-10-02", "04:00"),
                "Enter valid dates and times.",
            ),
            (
                fields("2026-10-01", "24:00", "2026-10-02", "04:00"),
                "Enter valid dates and times.",
            ),
            (
                fields("2026-10-08", "00:00", "2026-10-01", "00:00"),
                "End time must be after start time.",
            ),
            (
                fields("2026-10-08", "00:00", "2026-10-08", "00:00"),
                "End time must be after start time.",
            ),
            (
                fields("2025-01-01", "00:00", "2026-01-02", "00:00"),
                "Custom ranges can span up to 365 days.",
            ),
        ] {
            assert_eq!(
                custom_range(&fields, paris, None),
                Err(message),
                "{fields:?}"
            );
        }
        assert!(range(&fields("2025-01-01", "00:00", "2026-01-01", "00:00"), None).is_ok());
    }

    #[test]
    fn fills_the_dialog_with_the_period_shown() {
        let paris = Zone::parse("Europe/Paris").unwrap();
        let config = range_config(
            Range::Custom,
            paris,
            Some("2026-03-29T00:30Z"),
            Some("2026-03-29T01:30Z"),
            at("2026-10-07T12:00:00Z"),
        );
        let fields = CustomRangeFields::of(&config.start, &config.end);
        assert_eq!(fields.start_date.as_deref(), Some("2026-03-29"));
        assert_eq!(fields.start_time.as_deref(), Some("01:30"));
        assert_eq!(fields.end_time.as_deref(), Some("03:30"));
        assert_eq!(
            custom_range_address("Europe/Paris", &config.start, &config.end),
            "/analytics?range=custom&start=2026-03-29T00%3A30Z&end=2026-03-29T01%3A30Z&timeZone=Europe%2FParis"
        );
    }

    #[test]
    fn rounds_averages_as_javascript_does() {
        assert_eq!(rounded(None), 0);
        assert_eq!(rounded(Some(200.0)), 200);
        assert_eq!(rounded(Some(0.5)), 1);
        assert_eq!(rounded(Some(1.5)), 2);
        assert_eq!(rounded(Some(2.5)), 3);
        assert_eq!(rounded(Some(2.4999)), 2);
    }
}
