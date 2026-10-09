//! What `vine.date({ formats: ['iso8601'] })` reads as a date.
//!
//! Vine hands the value to Day.js: a number is a timestamp in milliseconds,
//! a string goes through Day.js's own pattern when it has no `Z` at its end
//! and to `new Date(string)` otherwise. Both are ported here with their
//! quirks. See [`date_iso8601`](crate::date_iso8601) for the two bounds of
//! the port.

use std::sync::LazyLock;

use serde_json::Value;

use crate::js_regex::JsRegex;

const MS_PER_DAY: i64 = 86_400_000;
/// The largest time value a `Date` holds, on either side of the epoch.
const MAX_TIME: f64 = 8.64e15;

/// `REGEX_PARSE` of Day.js.
static DAYJS_PATTERN: LazyLock<JsRegex> = LazyLock::new(|| {
    JsRegex::new(
        r"^(\d{4})[-/]?(\d{1,2})?[-/]?(\d{0,2})[Tt\s]*(\d{1,2})?:?(\d{1,2})?:?(\d{1,2})?[.:]?(\d+)?$",
        "",
    )
    .expect("static regex")
});

/// Days since the epoch of the first day of a month (0 to 11) of a year.
fn days_from_civil(year: i64, month: i64) -> i64 {
    let year = year + month.div_euclid(12);
    let month = month.rem_euclid(12) + 1;
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y.rem_euclid(400);
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Year, month (1 to 12) and day of a number of days since the epoch.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// `MakeDate(MakeDay(year, month, day), MakeTime(...))`, where every part
/// may be out of its range and rolls over into the next.
fn make_date(year: i64, month: i64, day: i64, time: [i64; 4]) -> i64 {
    let [hours, minutes, seconds, milliseconds] = time;
    let days = days_from_civil(year, month) + day - 1;
    days * MS_PER_DAY + hours * 3_600_000 + minutes * 60_000 + seconds * 1000 + milliseconds
}

/// `TimeClip`.
fn time_clip(time: f64) -> Option<f64> {
    (time.is_finite() && time.abs() <= MAX_TIME).then(|| time.trunc() + 0.0)
}

fn clip(time: i64) -> Option<f64> {
    // Exact: a time value in range is far below 2^53.
    time_clip(time as f64)
}

/// The time value Day.js builds from a string its pattern matches.
fn parse_dayjs(text: &str) -> Option<Option<f64>> {
    let captures = DAYJS_PATTERN.as_regex().captures(text)?;
    let part = |index: usize| {
        captures
            .get(index)
            .map(|matched| matched.as_str())
            .filter(|digits| !digits.is_empty())
            .and_then(|digits| digits.parse::<i64>().ok())
    };
    let mut year = part(1)?;
    // `new Date(year, ...)` reads a year below 100 as one of the 1900s.
    if (0..=99).contains(&year) {
        year += 1900;
    }
    // `d[2] - 1 || 0`: a month of zero is the month before January.
    let month = part(2).map_or(0, |month| month - 1);
    // Day.js keeps the first three digits of the fraction as a count of
    // milliseconds, so `.5` is 5 ms.
    let milliseconds = captures
        .get(7)
        .map(|matched| matched.as_str())
        .and_then(|digits| digits.get(..digits.len().min(3)))
        .and_then(|digits| digits.parse::<i64>().ok())
        .unwrap_or(0);
    let time = [
        part(4).unwrap_or(0),
        part(5).unwrap_or(0),
        part(6).unwrap_or(0),
        milliseconds,
    ];
    Some(clip(make_date(year, month, part(3).unwrap_or(1), time)))
}

struct Scanner<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Scanner<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn eat(&mut self, byte: u8) -> bool {
        let found = self.peek() == Some(byte);
        if found {
            self.at += 1;
        }
        found
    }

    fn done(&self) -> bool {
        self.at == self.bytes.len()
    }

    /// The digits at the cursor, however many there are.
    fn digits(&mut self) -> &[u8] {
        let start = self.at;
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.at += 1;
        }
        &self.bytes[start..self.at]
    }

    /// A number written with exactly `length` digits.
    fn fixed(&mut self, length: usize) -> Option<i64> {
        let digits = self.digits();
        if digits.len() != length {
            return None;
        }
        Some(
            digits
                .iter()
                .fold(0, |number, digit| number * 10 + i64::from(digit - b'0')),
        )
    }
}

/// `new Date(text)` for the ECMAScript date time format, as V8 reads it:
/// `YYYY[-MM[-DD]]`, optionally followed by `THH:mm[:ss[.fraction]]` and by
/// `Z`, `±HH:mm` or `±HHmm`. A date alone is in UTC.
fn parse_ecmascript(text: &str) -> Option<f64> {
    let mut scanner = Scanner {
        bytes: text.as_bytes(),
        at: 0,
    };
    let year = match scanner.peek()? {
        sign @ (b'+' | b'-') => {
            scanner.at += 1;
            let year = scanner.fixed(6)?;
            if sign == b'-' && year == 0 {
                return None;
            }
            if sign == b'-' { -year } else { year }
        }
        _ => scanner.fixed(4)?,
    };
    let (mut month, mut day) = (1, 1);
    if scanner.eat(b'-') {
        month = scanner.fixed(2).filter(|month| (1..=12).contains(month))?;
        if scanner.eat(b'-') {
            // Any day up to 31, whatever the month: the 30th of February is the 2nd of March.
            day = scanner.fixed(2).filter(|day| (1..=31).contains(day))?;
        }
    }

    let mut time = [0; 4];
    let mut offset_minutes = 0;
    if matches!(scanner.peek(), Some(b'T' | b't')) {
        scanner.at += 1;
        let hours = scanner.fixed(2).filter(|hours| (0..=24).contains(hours))?;
        // 24:00:00 is the end of the day, and the only time of hour 24.
        let is_24 = hours == 24;
        if !scanner.eat(b':') {
            return None;
        }
        let minutes = scanner
            .fixed(2)
            .filter(|minutes| (0..=59).contains(minutes))?;
        let (mut seconds, mut milliseconds) = (0, 0);
        if scanner.eat(b':') {
            seconds = scanner
                .fixed(2)
                .filter(|seconds| (0..=59).contains(seconds))?;
            if scanner.eat(b'.') {
                let digits = scanner.digits();
                if digits.is_empty() {
                    return None;
                }
                // More or fewer than three digits are allowed: the first three count.
                for index in 0..3 {
                    let digit = digits.get(index).map_or(0, |digit| i64::from(digit - b'0'));
                    milliseconds = milliseconds * 10 + digit;
                }
                if is_24 && digits.iter().take(9).any(|digit| *digit != b'0') {
                    return None;
                }
            }
        }
        if is_24 && (minutes > 0 || seconds > 0) {
            return None;
        }
        time = [hours, minutes, seconds, milliseconds];

        match scanner.peek() {
            Some(b'Z' | b'z') => scanner.at += 1,
            Some(sign @ (b'+' | b'-')) => {
                scanner.at += 1;
                let digits = scanner.digits().len();
                scanner.at -= digits;
                let (hours, minutes) = match digits {
                    4 => {
                        let both = scanner.fixed(4)?;
                        (both / 100, both % 100)
                    }
                    2 => {
                        let hours = scanner.fixed(2)?;
                        if !scanner.eat(b':') {
                            return None;
                        }
                        (hours, scanner.fixed(2)?)
                    }
                    _ => return None,
                };
                if hours > 23 || minutes > 59 {
                    return None;
                }
                offset_minutes = hours * 60 + minutes;
                if sign == b'-' {
                    offset_minutes = -offset_minutes;
                }
            }
            _ => {}
        }
    } else if matches!(scanner.peek(), Some(b'Z' | b'z')) {
        scanner.at += 1;
    }
    if !scanner.done() {
        return None;
    }
    clip(make_date(year, month - 1, day, time) - offset_minutes * 60_000)
}

fn parse_text(text: &str) -> Option<f64> {
    let ends_with_z = matches!(text.as_bytes().last(), Some(b'Z' | b'z'));
    if !ends_with_z && let Some(time) = parse_dayjs(text) {
        return time;
    }
    parse_ecmascript(text)
}

/// The time value, in milliseconds since the epoch, of what Vine would read
/// as a date. `None` when it would not.
pub(crate) fn parse(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => time_clip(number.as_f64()?),
        Value::String(text) => parse_text(text),
        _ => None,
    }
}

/// `Date.prototype.toISOString()`.
pub(crate) fn to_iso_string(time: f64) -> String {
    // A time value is a whole number within ±8.64e15.
    let time = time as i64;
    let (year, month, day) = civil_from_days(time.div_euclid(MS_PER_DAY));
    let in_day = time.rem_euclid(MS_PER_DAY);
    let (hours, minutes) = (in_day / 3_600_000, in_day / 60_000 % 60);
    let (seconds, milliseconds) = (in_day / 1000 % 60, in_day % 1000);
    let year = if (0..=9999).contains(&year) {
        format!("{year:04}")
    } else {
        format!("{}{:06}", if year < 0 { '-' } else { '+' }, year.abs())
    };
    format!("{year}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{milliseconds:03}Z")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn iso(value: Value) -> Option<String> {
        parse(&value).map(to_iso_string)
    }

    #[test]
    fn reads_iso_dates_with_and_without_an_offset() {
        assert_eq!(
            iso(json!("2026-10-01T12:00:00+02:00")).as_deref(),
            Some("2026-10-01T10:00:00.000Z")
        );
        assert_eq!(
            iso(json!("2026-10-01T12:00:00.123Z")).as_deref(),
            Some("2026-10-01T12:00:00.123Z")
        );
        assert_eq!(
            iso(json!("2026-10-01")).as_deref(),
            Some("2026-10-01T00:00:00.000Z")
        );
        assert_eq!(
            iso(json!("2026-10-01T12:00")).as_deref(),
            Some("2026-10-01T12:00:00.000Z")
        );
    }

    #[test]
    fn reads_a_number_as_milliseconds() {
        assert_eq!(
            iso(json!(1_767_225_600)).as_deref(),
            Some("1970-01-21T10:53:45.600Z")
        );
        assert_eq!(iso(json!(8.64e15 + 1.0)), None);
    }

    #[test]
    fn refuses_what_is_not_a_date() {
        for value in [
            json!("tomorrow"),
            json!(""),
            json!(true),
            json!(null),
            json!([2026]),
        ] {
            assert_eq!(iso(value), None);
        }
    }

    #[test]
    fn converts_days_both_ways() {
        for days in [
            -1_000_000, -719_468, -1, 0, 1, 59, 60, 365, 20_000, 1_000_000,
        ] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month - 1) + day - 1, days);
        }
    }
}
