//! Timestamps as the database stores them.
//!
//! Lucid wrote every datetime as wall-clock text without a zone,
//! `2026-10-07 12:19:57`, with the process pinned to UTC. A few raw queries
//! of the Node app added milliseconds (`2026-10-07 12:19:57.123`), and knex
//! binds a JavaScript `Date` as epoch milliseconds. [`Timestamp`] writes the
//! first form and reads all of them.

use std::fmt;

use chrono::{DateTime, Duration, NaiveDateTime, SubsecRound, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::sqlite::{Sqlite, SqliteArgumentsBuffer, SqliteTypeInfo, SqliteValueRef};

/// The format Lucid uses for SQLite.
const SQL_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// A point in time with the precision the database keeps: whole seconds, UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(DateTime<Utc>);

impl Timestamp {
    pub fn now() -> Self {
        Self::from(Utc::now())
    }

    pub fn from_millis(millis: i64) -> Option<Self> {
        Utc.timestamp_millis_opt(millis).single().map(Self::from)
    }

    pub fn as_datetime(&self) -> DateTime<Utc> {
        self.0
    }

    pub fn timestamp_millis(&self) -> i64 {
        self.0.timestamp_millis()
    }

    /// The text stored in the database, also the right operand for comparing
    /// against a stored column in SQL.
    pub fn to_sql(&self) -> String {
        self.0.format(SQL_FORMAT).to_string()
    }

    /// ISO 8601 with milliseconds and `Z`, as the Node app serialised dates
    /// for the browser.
    pub fn to_iso(&self) -> String {
        self.0.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
    }

    pub fn is_past(&self) -> bool {
        *self < Self::now()
    }

    pub fn is_future(&self) -> bool {
        *self > Self::now()
    }

    /// Read any of the forms a datetime column may hold.
    pub fn parse_sql(value: &str) -> Option<Self> {
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        if value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Self::from_millis(value.parse().ok()?);
        }
        if let Ok(with_zone) = DateTime::parse_from_rfc3339(value) {
            return Some(Self::from(with_zone.with_timezone(&Utc)));
        }
        if let Ok(with_zone) = DateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f %:z") {
            return Some(Self::from(with_zone.with_timezone(&Utc)));
        }
        ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"]
            .iter()
            .find_map(|format| NaiveDateTime::parse_from_str(value, format).ok())
            .map(|naive| Self::from(naive.and_utc()))
    }
}

impl Default for Timestamp {
    /// The epoch. A model sets its own timestamps when it is inserted.
    fn default() -> Self {
        Self(DateTime::UNIX_EPOCH)
    }
}

impl From<DateTime<Utc>> for Timestamp {
    fn from(value: DateTime<Utc>) -> Self {
        Self(value.trunc_subsecs(0))
    }
}

impl From<Timestamp> for DateTime<Utc> {
    fn from(value: Timestamp) -> Self {
        value.0
    }
}

impl std::ops::Add<Duration> for Timestamp {
    type Output = Timestamp;

    fn add(self, duration: Duration) -> Timestamp {
        Timestamp::from(self.0 + duration)
    }
}

impl std::ops::Sub<Duration> for Timestamp {
    type Output = Timestamp;

    fn sub(self, duration: Duration) -> Timestamp {
        Timestamp::from(self.0 - duration)
    }
}

impl std::ops::Sub<Timestamp> for Timestamp {
    type Output = Duration;

    fn sub(self, other: Timestamp) -> Duration {
        self.0 - other.0
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_sql())
    }
}

impl sqlx::Type<Sqlite> for Timestamp {
    fn type_info() -> SqliteTypeInfo {
        <String as sqlx::Type<Sqlite>>::type_info()
    }

    /// Any storage class: old rows hold text or epoch milliseconds.
    fn compatible(_: &SqliteTypeInfo) -> bool {
        true
    }
}

impl sqlx::Encode<'_, Sqlite> for Timestamp {
    fn encode_by_ref(&self, buffer: &mut SqliteArgumentsBuffer) -> Result<IsNull, BoxDynError> {
        sqlx::Encode::<Sqlite>::encode(self.to_sql(), buffer)
    }
}

impl<'r> sqlx::Decode<'r, Sqlite> for Timestamp {
    fn decode(value: SqliteValueRef<'r>) -> Result<Self, BoxDynError> {
        let text = <String as sqlx::Decode<Sqlite>>::decode(value)?;
        Timestamp::parse_sql(&text)
            .ok_or_else(|| format!("unreadable datetime in the database: {text:?}").into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_lucid_format() {
        let time = Timestamp::parse_sql("2026-10-07 12:19:57").unwrap();
        assert_eq!(time.to_sql(), "2026-10-07 12:19:57");
        assert_eq!(time.to_iso(), "2026-10-07T12:19:57.000Z");
    }

    #[test]
    fn reads_every_form_the_node_app_wrote() {
        let expected = Timestamp::parse_sql("2026-10-07 12:19:57").unwrap();
        for stored in [
            "2026-10-07 12:19:57.123",
            "2026-10-07 12:19:57.000 +00:00",
            "2026-10-07 14:19:57.000 +02:00",
            "2026-10-07T12:19:57.123Z",
            "2026-10-07T12:19:57",
            "1791375597123",
        ] {
            assert_eq!(Timestamp::parse_sql(stored), Some(expected), "{stored}");
        }
        assert_eq!(Timestamp::parse_sql(""), None);
        assert_eq!(Timestamp::parse_sql("yesterday"), None);
    }

    #[test]
    fn keeps_whole_seconds() {
        let now = Timestamp::now();
        assert_eq!(Timestamp::parse_sql(&now.to_sql()), Some(now));
    }
}
