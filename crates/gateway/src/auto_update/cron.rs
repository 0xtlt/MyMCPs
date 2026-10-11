//! The schedule npm MCPs are refreshed on: a 5-field cron expression, read
//! in UTC.
//!
//! The Node app read expressions with Croner 10, whose parser has a manner
//! of its own: `?` stands for `*` in any field, a weekday may be numbered
//! (`FRI#2`) or last (`5L`), a day of the month may be the last (`L`) or the
//! weekday nearest to a date (`15W`), a step larger than its field is
//! refused, and so on. A saved expression has to mean here what it meant
//! there, and the Settings page has to take and refuse the same ones, so the
//! parser is written again after Croner's, decision for decision, and
//! compared with it in the tests.

use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Timelike, Utc};
use mymcps_vine as vine;

pub use mymcps_core::models::DEFAULT_MCP_AUTO_UPDATE_CRON;

/// Any occurrence of a weekday in its month: the first to the fifth, and the last.
const ANY_WEEK: u8 = 63;
/// The last occurrence of a weekday in its month.
const LAST_WEEK: u8 = 32;
/// The first to the fifth occurrence of a weekday in its month.
const NTH_WEEK: [u8; 5] = [1, 2, 4, 8, 16];

/// The search for a next run gives up at this year, as Croner does: an
/// expression such as `0 0 31 2 *` names a date that never comes.
const LAST_YEAR: i32 = 3000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Minute,
    Hour,
    Day,
    Month,
    DayOfWeek,
    /// The days of the month written with `W`.
    NearestWeekdays,
}

impl Part {
    /// How many values the part has.
    fn len(self) -> i64 {
        match self {
            Self::Minute => 60,
            Self::Hour => 24,
            Self::Day | Self::NearestWeekdays => 31,
            Self::Month => 12,
            Self::DayOfWeek => 7,
        }
    }

    /// What turns a value as it is written into its place in the part:
    /// days and months are written from one.
    fn offset(self) -> i64 {
        match self {
            Self::Day | Self::NearestWeekdays | Self::Month => -1,
            _ => 0,
        }
    }
}

/// Why an expression is not one: the caller only needs to know that it is not.
struct Invalid;

type Parsed<T = ()> = Result<T, Invalid>;

/// A parsed 5-field cron expression (minute, hour, day of month, month, day
/// of week).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CronPattern {
    minute: [bool; 60],
    hour: [bool; 24],
    day: [bool; 31],
    month: [bool; 12],
    /// For each weekday from Sunday, the weeks of the month it runs in.
    day_of_week: [u8; 7],
    last_day_of_month: bool,
    last_weekday: bool,
    nearest_weekdays: [bool; 31],
    star_day: bool,
    star_day_of_week: bool,
    /// `+` before the day of week: the day of month and the day of week
    /// both have to match, where either would do.
    use_and_logic: bool,
}

/// `parseInt(text, 10)` for a text that holds no sign and no whitespace:
/// the number its leading digits write.
fn parse_int(text: &str) -> Parsed<i64> {
    let digits = text.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return Err(Invalid);
    }
    // No part has a value this large: more digits are still too large.
    Ok(text[..digits].parse().unwrap_or(i64::MAX / 2))
}

fn replace_ignoring_case(text: &str, name: &str, replacement: &str) -> String {
    let (bytes, name) = (text.as_bytes(), name.as_bytes());
    let mut replaced = String::with_capacity(text.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index..]
            .get(..name.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(name))
        {
            replaced.push_str(replacement);
            index += name.len();
        } else {
            // A name is ASCII, so what is copied here is whole characters.
            let character = text[index..].chars().next().map_or(1, char::len_utf8);
            replaced.push_str(&text[index..index + character]);
            index += character;
        }
    }
    replaced
}

fn replace_alpha_months(text: &str) -> String {
    [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .zip(1..)
    .fold(text.to_owned(), |text, (name, number): (_, u32)| {
        replace_ignoring_case(&text, name, &number.to_string())
    })
}

fn replace_alpha_days(text: &str) -> String {
    // A range that ends on Sunday ends on the seventh day.
    let text = replace_ignoring_case(text, "-sun", "-7");
    ["sun", "mon", "tue", "wed", "thu", "fri", "sat"]
        .iter()
        .zip(0..)
        .fold(text, |text, (name, number): (_, u32)| {
            replace_ignoring_case(&text, name, &number.to_string())
        })
}

fn has_illegal_characters(text: &str, also: &str) -> bool {
    text.chars().any(|character| {
        !(character.is_ascii_digit() || "/*,-".contains(character) || also.contains(character))
    })
}

/// The value of an entry and, for a weekday, the week of the month written
/// after it: `5#2` is the second Friday, `5L` the last.
fn extract_nth(entry: &str, part: Part) -> Parsed<(&str, Option<&str>)> {
    if entry.contains('#') {
        if part != Part::DayOfWeek {
            return Err(Invalid);
        }
        let mut pieces = entry.split('#');
        let value = pieces.next().unwrap_or_default();
        return Ok((value, pieces.next()));
    }
    if entry.ends_with(['L', 'l']) {
        if part != Part::DayOfWeek {
            return Err(Invalid);
        }
        return Ok((&entry[..entry.len() - 1], Some("L")));
    }
    Ok((entry, None))
}

impl CronPattern {
    fn empty() -> Self {
        Self {
            minute: [false; 60],
            hour: [false; 24],
            day: [false; 31],
            month: [false; 12],
            day_of_week: [0; 7],
            last_day_of_month: false,
            last_weekday: false,
            nearest_weekdays: [false; 31],
            star_day: false,
            star_day_of_week: false,
            use_and_logic: false,
        }
    }

    /// Read the five fields of an expression.
    fn parse(fields: [&str; 5]) -> Parsed<Self> {
        let [minute, hour, day, month, day_of_week] = fields;
        let mut pattern = Self::empty();

        let mut day = day.to_owned();
        if day.eq_ignore_ascii_case("LW") {
            pattern.last_weekday = true;
            day.clear();
        } else if day.contains(['L', 'l']) {
            day = day.replace(['L', 'l'], "");
            pattern.last_day_of_month = true;
        }
        pattern.star_day = day == "*";

        let month = if vine::js::utf16_len(month) >= 3 {
            replace_alpha_months(month)
        } else {
            month.to_owned()
        };
        let mut day_of_week = if vine::js::utf16_len(day_of_week) >= 3 {
            replace_alpha_days(day_of_week)
        } else {
            day_of_week.to_owned()
        };
        if let Some(rest) = day_of_week.strip_prefix('+') {
            pattern.use_and_logic = true;
            day_of_week = rest.to_owned();
            if day_of_week.is_empty() {
                return Err(Invalid);
            }
        }
        pattern.star_day_of_week = day_of_week == "*";

        // `?` reads as `*`, but a field written with it is not one left
        // open: `0 0 ? * 1` runs every day, as it did.
        let open = |field: &str| field.replace('?', "*");
        let (minute, hour, day, month, day_of_week) = (
            open(minute),
            open(hour),
            open(&day),
            open(&month),
            open(&day_of_week),
        );

        if has_illegal_characters(&minute, "")
            || has_illegal_characters(&hour, "")
            || has_illegal_characters(&day, "WwLl")
            || has_illegal_characters(&month, "")
            || has_illegal_characters(&day_of_week, "#Ll")
        {
            return Err(Invalid);
        }

        pattern.read_part(Part::Minute, &minute)?;
        pattern.read_part(Part::Hour, &hour)?;
        pattern.read_part(Part::Day, &day)?;
        pattern.read_part(Part::Month, &month)?;
        pattern.read_part(Part::DayOfWeek, &day_of_week)?;
        Ok(pattern)
    }

    fn read_part(&mut self, part: Part, entry: &str) -> Parsed {
        let names_a_last_day = part == Part::Day && (self.last_day_of_month || self.last_weekday);
        if entry.is_empty() && !names_a_last_day {
            return Err(Invalid);
        }
        if entry == "*" {
            match part {
                Part::Minute => self.minute.fill(true),
                Part::Hour => self.hour.fill(true),
                Part::Day => self.day.fill(true),
                Part::Month => self.month.fill(true),
                Part::DayOfWeek => self.day_of_week.fill(ANY_WEEK),
                Part::NearestWeekdays => self.nearest_weekdays.fill(true),
            }
            return Ok(());
        }

        if entry.contains(',') {
            for item in entry.split(',') {
                self.read_part(part, item)?;
            }
            Ok(())
        } else if entry.contains('-') && entry.contains('/') {
            self.read_range_with_stepping(part, entry)
        } else if entry.contains('-') {
            self.read_range(part, entry)
        } else if entry.contains('/') {
            self.read_stepping(part, entry)
        } else if !entry.is_empty() {
            self.read_number(part, entry)
        } else {
            Ok(())
        }
    }

    fn read_number(&mut self, part: Part, entry: &str) -> Parsed {
        let (value, nth) = extract_nth(entry, part)?;
        let nearest_weekday = entry.contains(['W', 'w']);
        if nearest_weekday && part != Part::Day {
            return Err(Invalid);
        }
        let part = if nearest_weekday {
            Part::NearestWeekdays
        } else {
            part
        };
        self.set(part, parse_int(value)? + part.offset(), nth)
    }

    fn read_range(&mut self, part: Part, entry: &str) -> Parsed {
        if entry.contains(['W', 'w']) {
            return Err(Invalid);
        }
        let (value, nth) = extract_nth(entry, part)?;
        let bounds: Vec<&str> = value.split('-').collect();
        let [lower, upper] = bounds[..] else {
            return Err(Invalid);
        };
        let lower = parse_int(lower)? + part.offset();
        let upper = parse_int(upper)? + part.offset();
        if lower > upper {
            return Err(Invalid);
        }
        // A value out of the part is refused as it is reached.
        for index in lower..=upper {
            self.set(part, index, nth)?;
        }
        Ok(())
    }

    fn read_range_with_stepping(&mut self, part: Part, entry: &str) -> Parsed {
        if entry.contains(['W', 'w']) {
            return Err(Invalid);
        }
        let (value, nth) = extract_nth(entry, part)?;
        // `lower-upper/step`, each made of digits only.
        let (range, step) = value.split_once('/').ok_or(Invalid)?;
        let (lower, upper) = range.split_once('-').ok_or(Invalid)?;
        if [lower, upper, step]
            .iter()
            .any(|number| number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return Err(Invalid);
        }
        let lower = parse_int(lower)? + part.offset();
        let upper = parse_int(upper)? + part.offset();
        let step = parse_int(step)?;
        if lower > upper || step == 0 || step > part.len() {
            return Err(Invalid);
        }
        let mut index = lower;
        while index <= upper {
            self.set(part, index, nth)?;
            index += step;
        }
        Ok(())
    }

    fn read_stepping(&mut self, part: Part, entry: &str) -> Parsed {
        if entry.contains(['W', 'w']) {
            return Err(Invalid);
        }
        let (value, nth) = extract_nth(entry, part)?;
        let pieces: Vec<&str> = value.split('/').collect();
        // A step counts from the start of the part: `*/5`, never `3/5`.
        let ["*", step] = pieces[..] else {
            return Err(Invalid);
        };
        let step = parse_int(step)?;
        if step == 0 || step > part.len() {
            return Err(Invalid);
        }
        let mut index = 0;
        while index < part.len() {
            self.set(part, index, nth)?;
            index += step;
        }
        Ok(())
    }

    fn set(&mut self, part: Part, index: i64, nth: Option<&str>) -> Parsed {
        if part == Part::DayOfWeek {
            // Sunday is the day 0, and the day 7 too.
            let index = if index == 7 { 0 } else { index };
            let day = usize::try_from(index).map_err(|_| Invalid)?;
            let weeks = self.day_of_week.get_mut(day).ok_or(Invalid)?;
            return match nth.filter(|nth| !nth.is_empty()) {
                None => {
                    *weeks = ANY_WEEK;
                    Ok(())
                }
                Some(nth) if nth.eq_ignore_ascii_case("L") => {
                    *weeks |= LAST_WEEK;
                    Ok(())
                }
                Some(nth) if nth.bytes().all(|byte| byte.is_ascii_digit()) => {
                    let week = parse_int(nth)?;
                    let week = usize::try_from(week - 1).map_err(|_| Invalid)?;
                    *weeks |= NTH_WEEK.get(week).ok_or(Invalid)?;
                    Ok(())
                }
                Some(_) => Err(Invalid),
            };
        }

        let values: &mut [bool] = match part {
            Part::Minute => &mut self.minute,
            Part::Hour => &mut self.hour,
            Part::Day => &mut self.day,
            Part::Month => &mut self.month,
            Part::NearestWeekdays => &mut self.nearest_weekdays,
            Part::DayOfWeek => return Err(Invalid),
        };
        let index = usize::try_from(index).map_err(|_| Invalid)?;
        *values.get_mut(index).ok_or(Invalid)? = true;
        Ok(())
    }

    /// Whether the expression runs on this day, whatever the time.
    fn runs_on(&self, date: NaiveDate) -> bool {
        if !self.month[date.month0() as usize] {
            return false;
        }
        let day = date.day();
        let last_day = last_day_of_month(date);
        let weekday_of = |day: u32| {
            date.with_day(day)
                .map_or(0, |date| date.weekday().num_days_from_sunday())
        };

        let mut by_day_of_month = self.day[date.day0() as usize];
        if !by_day_of_month {
            // The weekday nearest to a date, within its month.
            by_day_of_month = (1..=last_day)
                .filter(|wanted| self.nearest_weekdays[*wanted as usize - 1])
                .any(|wanted| {
                    let nearest = match weekday_of(wanted) {
                        0 if wanted == last_day => wanted - 2,
                        0 => wanted + 1,
                        6 if wanted == 1 => wanted + 2,
                        6 => wanted - 1,
                        _ => wanted,
                    };
                    nearest == day
                });
        }
        if self.last_weekday {
            let last_weekday = match weekday_of(last_day) {
                0 => last_day - 2,
                6 => last_day - 1,
                _ => last_day,
            };
            by_day_of_month |= day == last_weekday;
        }
        if self.last_day_of_month {
            by_day_of_month |= day == last_day;
        }
        if self.star_day_of_week {
            return by_day_of_month;
        }

        let weeks = self.day_of_week[weekday_of(day) as usize];
        // Which of its weekday this day is in the month: the first, the second...
        let week = NTH_WEEK.get(date.day0() as usize / 7).copied().unwrap_or(0);
        let by_day_of_week = weeks & week != 0 || (weeks & LAST_WEEK != 0 && day + 7 > last_day);

        if self.use_and_logic || self.star_day {
            by_day_of_month && by_day_of_week
        } else {
            by_day_of_month || by_day_of_week
        }
    }

    /// The first minute of a day the expression runs at, from this minute of
    /// the day on.
    fn first_run_of_day(&self, from_minute: u32) -> Option<(u32, u32)> {
        (from_minute..24 * 60)
            .map(|minute| (minute / 60, minute % 60))
            .find(|(hour, minute)| self.hour[*hour as usize] && self.minute[*minute as usize])
    }

    /// The next time the expression runs after `after`, in UTC. `None` for
    /// an expression that names a date that never comes.
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        // The first whole minute past `after`.
        let earliest = after.with_nanosecond(0)? + Duration::seconds(1);
        let earliest = if earliest.second() == 0 {
            earliest
        } else {
            earliest.with_second(0)? + Duration::minutes(1)
        };

        let mut date = earliest.date_naive();
        let mut from_minute = earliest.hour() * 60 + earliest.minute();
        while date.year() < LAST_YEAR {
            if self.runs_on(date)
                && let Some((hour, minute)) = self.first_run_of_day(from_minute)
            {
                let run = date.and_hms_opt(hour, minute, 0)?;
                return Some(Utc.from_utc_datetime(&run));
            }
            date = if self.month[date.month0() as usize] {
                date.succ_opt()?
            } else {
                // Nothing runs in this month: on to the next.
                first_of_next_month(date)?
            };
            from_minute = 0;
        }
        None
    }
}

fn first_of_next_month(date: NaiveDate) -> Option<NaiveDate> {
    if date.month() == 12 {
        NaiveDate::from_ymd_opt(date.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(date.year(), date.month() + 1, 1)
    }
}

fn last_day_of_month(date: NaiveDate) -> u32 {
    first_of_next_month(date)
        .and_then(|next| next.pred_opt())
        .map_or(28, |last| last.day())
}

/// Parse a 5-field cron expression (minute, hour, day of month, month, day
/// of week), or `None` when it is not one.
///
/// Croner reads a text with a colon in it as a date to run once at, and
/// takes the ones Node can read as a date. Here such a text is what it is
/// to the parser, an expression with a character no field has.
pub fn parse_five_field_cron(value: &str) -> Option<CronPattern> {
    let fields: Vec<&str> = value
        .split(vine::js::is_whitespace)
        .filter(|field| !field.is_empty())
        .collect();
    CronPattern::parse(fields.try_into().ok()?).ok()
}

/// True when `value` is a 5-field cron expression Croner can parse.
pub fn is_valid_five_field_cron(value: &str) -> bool {
    parse_five_field_cron(value).is_some()
}
