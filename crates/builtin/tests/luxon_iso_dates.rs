//! The two date arguments against the real Luxon.
//!
//! `fixtures/luxon_iso_dates.json` holds, for some three hundred values,
//! what the rules of `app/validators/builtin_tools.ts` make of them with
//! Luxon 3.7 in a process running in UTC: `isoDate` as
//! `DateTime.fromISO(value.trim(), { zone: 'utc' }).toISO()`, and
//! `localTimestamp` as what it hands the tool. `null` is a value Luxon does
//! not read as a date. The fixture was written by a script that is not part
//! of the repository, since it runs Node on the TypeScript app's
//! `node_modules`.

use chrono::{DateTime, Duration, Utc};
use mymcps_builtin::arguments::{
    TOOL_VINE, iso_date, local_timestamp, read_iso_date, read_local_timestamp, to_iso,
};
use mymcps_builtin::tool_input::tool_input;
use mymcps_vine as vine;
use serde::Deserialize;
use serde_json::{Value, json};

const FIXTURE: &str = include_str!("fixtures/luxon_iso_dates.json");

/// A time zone by its name needs the time zone database to be read.
fn names_a_zone(text: &str) -> bool {
    text.contains('[')
}

/// The year of an instant as Luxon writes it, when chrono cannot hold it.
fn beyond_chrono(iso: &str) -> bool {
    let signed = iso.starts_with('+') || iso.starts_with('-');
    let year: i64 = if signed {
        iso[..7].parse().unwrap()
    } else {
        iso[..4].parse().unwrap()
    };
    !(-262_143..=262_142).contains(&year)
}

#[derive(Debug, Deserialize)]
struct Dated {
    x: DateTime<Utc>,
}

#[test]
fn reads_the_dates_luxon_reads() {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    let now: DateTime<Utc> = fixture["now"].as_str().unwrap().parse().unwrap();
    let another_day = now + Duration::days(40);
    let dates = TOOL_VINE.create(vine::object! { "x" => iso_date() });
    let clocks = TOOL_VINE.create(vine::object! { "x" => local_timestamp() });
    let cases = fixture["cases"].as_array().unwrap();
    assert!(cases.len() > 300);

    let (mut named, mut beyond, mut through_the_rules) = (0, 0, 0);
    for case in cases {
        let (input, instant, clock) = (&case[0], case[1].as_str(), case[2].as_str());
        let arguments = json!({ "x": input });
        let Some(text) = input.as_str() else {
            // What is not text is not a date.
            assert_eq!((instant, clock), (None, None), "{input}");
            assert!(tool_input::<Value>(&dates, &arguments).is_err(), "{input}");
            assert!(tool_input::<Value>(&clocks, &arguments).is_err(), "{input}");
            continue;
        };

        let read_instant = read_iso_date(text, now).map(to_iso);
        let read_clock = read_local_timestamp(text, now);
        if names_a_zone(text) {
            assert_eq!((read_instant, read_clock), (None, None), "{text:?}");
            named += usize::from(instant.is_some());
            continue;
        }
        if instant.is_some_and(beyond_chrono) {
            assert_eq!(read_instant, None, "{text:?}");
            beyond += 1;
        } else {
            assert_eq!(read_instant.as_deref(), instant, "isoDate of {text:?}");
        }
        assert_eq!(read_clock.as_deref(), clock, "localTimestamp of {text:?}");

        // The rules read the same, on any day for what does not depend on it.
        let today = read_local_timestamp(text, another_day) != read_local_timestamp(text, now);
        if today {
            continue;
        }
        through_the_rules += 1;
        let from_rule = tool_input::<Value>(&dates, &arguments)
            .ok()
            .map(|input| input["x"].clone());
        assert_eq!(
            from_rule,
            read_iso_date(text, now).map(|instant| json!(to_iso(instant))),
            "{text:?}"
        );
        if from_rule.is_some() {
            // What the rule hands the tool is what a `DateTime<Utc>` reads.
            let dated: Dated = tool_input(&dates, &arguments).unwrap();
            assert_eq!(Some(dated.x), read_iso_date(text, now), "{text:?}");
        }
        let from_rule = tool_input::<Value>(&clocks, &arguments)
            .ok()
            .map(|input| input["x"].clone());
        assert_eq!(from_rule, clock.map(|clock| json!(clock)), "{text:?}");
    }

    // The two bounds of the port, and nothing else.
    assert_eq!(named, 8);
    assert_eq!(beyond, 10);
    assert!(through_the_rules > 250, "{through_the_rules}");
}

#[test]
fn reads_a_time_alone_as_a_time_of_today() {
    let now: DateTime<Utc> = "2026-10-07T23:30:45.123Z".parse().unwrap();
    assert_eq!(
        read_iso_date("18:00", now).map(to_iso).as_deref(),
        Some("2026-10-07T18:00:00.000Z")
    );
    // Today is the day it is where the time is given.
    assert_eq!(
        read_iso_date("18:00+02:00", now).map(to_iso).as_deref(),
        Some("2026-10-08T16:00:00.000Z")
    );
    assert_eq!(
        read_local_timestamp("18:00+02:00", now).as_deref(),
        Some("2026-10-08T18:00:00Z")
    );

    let validator = TOOL_VINE.create(vine::object! { "x" => iso_date() });
    let before = Utc::now();
    let dated: Dated = tool_input(&validator, &json!({ "x": "00:00" })).unwrap();
    let after = Utc::now();
    let midnights =
        [before, after].map(|now| now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_utc());
    assert!(midnights.contains(&dated.x));
}

#[test]
fn writes_instants_as_luxon_does() {
    for iso in [
        "2026-10-01T10:00:00.000Z",
        "0000-01-01T00:00:00.000Z",
        "9999-12-31T23:59:59.999Z",
        "+010000-01-01T00:00:00.000Z",
        "-000001-12-31T22:00:00.000Z",
        "+262142-12-31T23:59:59.999Z",
        "-262143-01-01T00:00:00.000Z",
    ] {
        // chrono reads the six-digit years Luxon writes.
        let instant: DateTime<Utc> = iso.parse().unwrap();
        assert_eq!(to_iso(instant), iso);
        let dated: Dated = serde_json::from_value(json!({ "x": iso })).unwrap();
        assert_eq!(dated.x, instant);
    }
}
