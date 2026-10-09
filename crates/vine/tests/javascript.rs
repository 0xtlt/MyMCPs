//! The JavaScript the port had to write for itself, against the real thing.
//!
//! `fixtures/js.json` holds what Node 24 answers for `Number(value)`,
//! `String(number)`, `trim`, regular expressions and `new Date(text)` as
//! Day.js calls it, and what validator.js 13.15 answers for the checks Vine
//! borrows from it. It is written by the same script as `vine.json`.

use mymcps_vine as vine;
use serde_json::Value;
use vine::helpers::{self, FqdnOptions};

const FIXTURES: &str = include_str!("fixtures/js.json");

fn fixtures(name: &str) -> Vec<Value> {
    let all: Value = serde_json::from_str(FIXTURES).unwrap();
    all[name].as_array().unwrap().clone()
}

fn flag(options: &Value, name: &str, default: bool) -> bool {
    options
        .get(name)
        .map_or(default, |value| value.as_bool().unwrap())
}

#[test]
fn converts_values_to_numbers_like_number() {
    for case in fixtures("toNumber") {
        let number = vine::js::to_number(Some(&case[0]));
        assert_eq!(
            vine::js::number_to_string(number),
            case[1].as_str().unwrap(),
            "Number({})",
            case[0]
        );
    }
    assert!(vine::js::to_number(None).is_nan());
}

#[test]
fn writes_numbers_like_string() {
    for case in fixtures("numberToString") {
        let text = case[0].as_str().unwrap();
        let number = vine::js::string_to_number(text);
        assert_eq!(
            vine::js::number_to_string(number),
            case[1].as_str().unwrap(),
            "String({text})"
        );
    }
}

#[test]
fn trims_and_measures_like_string() {
    for case in fixtures("trim") {
        let text = case[0].as_str().unwrap();
        assert_eq!(
            vine::js::trim(text),
            case[1].as_str().unwrap(),
            "{text:?}.trim()"
        );
        assert_eq!(
            vine::js::utf16_len(text) as u64,
            case[2].as_u64().unwrap(),
            "{text:?}.length"
        );
    }
}

#[test]
fn takes_the_email_addresses_validator_js_takes() {
    for case in fixtures("isEmail") {
        let text = case[0].as_str().unwrap();
        assert_eq!(
            helpers::is_email(text),
            case[1].as_bool().unwrap(),
            "isEmail({text:?})"
        );
    }
}

#[test]
fn takes_the_urls_validator_js_takes() {
    let cases = fixtures("isURL");
    assert!(cases.len() > 600);
    for case in cases {
        let text = case[0].as_str().unwrap();
        let options = &case[1];
        let defaults = vine::UrlOptions::default();
        let url = vine::UrlOptions {
            protocols: options
                .get("protocols")
                .map_or(defaults.protocols.clone(), |list| {
                    list.as_array()
                        .unwrap()
                        .iter()
                        .map(|item| item.as_str().unwrap().to_owned())
                        .collect()
                }),
            require_tld: flag(options, "require_tld", defaults.require_tld),
            require_protocol: flag(options, "require_protocol", defaults.require_protocol),
            require_host: flag(options, "require_host", defaults.require_host),
            require_port: flag(options, "require_port", defaults.require_port),
            require_valid_protocol: flag(
                options,
                "require_valid_protocol",
                defaults.require_valid_protocol,
            ),
            allow_underscores: flag(options, "allow_underscores", defaults.allow_underscores),
            allow_trailing_dot: flag(options, "allow_trailing_dot", defaults.allow_trailing_dot),
            allow_protocol_relative_urls: flag(
                options,
                "allow_protocol_relative_urls",
                defaults.allow_protocol_relative_urls,
            ),
            allow_fragments: flag(options, "allow_fragments", defaults.allow_fragments),
            allow_query_components: flag(
                options,
                "allow_query_components",
                defaults.allow_query_components,
            ),
            disallow_auth: flag(options, "disallow_auth", defaults.disallow_auth),
            validate_length: flag(options, "validate_length", defaults.validate_length),
            max_allowed_length: options
                .get("max_allowed_length")
                .map_or(defaults.max_allowed_length, |length| {
                    usize::try_from(length.as_u64().unwrap()).unwrap()
                }),
        };
        assert_eq!(
            helpers::is_url(text, &url),
            case[2].as_bool().unwrap(),
            "isURL({text:?}, {options})"
        );
    }
}

#[test]
fn takes_the_host_names_validator_js_takes() {
    for case in fixtures("isFQDN") {
        let text = case[0].as_str().unwrap();
        let options = &case[1];
        let fqdn = FqdnOptions {
            require_tld: flag(options, "require_tld", true),
            allow_underscores: flag(options, "allow_underscores", false),
            allow_trailing_dot: flag(options, "allow_trailing_dot", false),
            allow_numeric_tld: flag(options, "allow_numeric_tld", false),
            allow_wildcard: flag(options, "allow_wildcard", false),
            ignore_max_length: flag(options, "ignore_max_length", false),
        };
        assert_eq!(
            helpers::is_fqdn(text, &fqdn),
            case[2].as_bool().unwrap(),
            "isFQDN({text:?}, {options})"
        );
    }
}

#[test]
fn takes_the_ip_addresses_validator_js_takes() {
    for case in fixtures("isIP") {
        let text = case[0].as_str().unwrap();
        assert_eq!(
            helpers::is_ipv4(text),
            case[1].as_bool().unwrap(),
            "isIP({text:?}, 4)"
        );
        assert_eq!(
            helpers::is_ipv6(text),
            case[2].as_bool().unwrap(),
            "isIP({text:?}, 6)"
        );
        assert_eq!(
            helpers::is_ip(text),
            case[3].as_bool().unwrap(),
            "isIP({text:?})"
        );
    }
}

/// In JavaScript, without the `u` flag, a character outside the Basic
/// Multilingual Plane is two code units: `.` and a negated class match one
/// half of it, and so do not match it whole. The translation does.
fn counts_code_units(source: &str, flags: &str, text: &str) -> bool {
    text == "😀"
        && !flags.contains('u')
        && matches!(source, "^.$" | "^[^a-c]$" | "^[^]$" | "^[^\\w]$")
}

#[test]
fn matches_what_javascript_regular_expressions_match() {
    let all: Value = serde_json::from_str(FIXTURES).unwrap();
    let texts: Vec<&str> = all["regexTexts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|text| text.as_str().unwrap())
        .collect();
    let mut compared = 0;
    let mut differences = Vec::new();
    for case in all["regex"].as_array().unwrap() {
        let (source, flags) = (
            case["source"].as_str().unwrap(),
            case["flags"].as_str().unwrap(),
        );
        let expression = match vine::js::regex(source, flags) {
            Ok(expression) => expression,
            Err(error) => panic!("/{source}/{flags}: {error}"),
        };
        assert_eq!(expression.source(), source);
        for (text, expected) in texts.iter().zip(case["results"].as_array().unwrap()) {
            let expected = expected.as_bool().unwrap();
            compared += 1;
            if expression.test(text) != expected {
                differences.push((source, flags, *text, expected));
            }
        }
    }
    assert!(compared > 3_500, "only {compared} matches compared");
    // The differences are all of the one kind the translation documents.
    let unexplained: Vec<_> = differences
        .iter()
        .filter(|(source, flags, text, _)| !counts_code_units(source, flags, text))
        .collect();
    assert!(
        unexplained.is_empty(),
        "{} differences: {unexplained:?}",
        unexplained.len()
    );
    assert_eq!(differences.len(), 5);
}

#[test]
fn nulls_the_empty_strings_the_body_parser_nulls() {
    for case in fixtures("emptyStringsToNull") {
        let mut body = case[0].clone();
        vine::empty_strings_to_null(&mut body);
        assert_eq!(body, case[1], "{}", case[0]);
    }
}

/// What V8 reads as a date through the parser it falls back to when a
/// string is not in the ECMAScript format, and the port does not.
const LEGACY_DATES: [&str; 2] = ["2026-10-01T12:00:00 +02:00", "2026-10-01x"];

#[test]
fn reads_the_dates_vine_reads() {
    let validator = vine::Vine::new().create(vine::date_iso8601());
    let cases = fixtures("date");
    assert!(cases.len() > 100);
    let mut differences = Vec::new();
    for case in &cases {
        let actual = validator.validate_opt(&case[0]).ok().flatten();
        if actual.as_ref() != Some(&case[1]).filter(|expected| !expected.is_null()) {
            differences.push(format!("{} vine: {} rust: {actual:?}", case[0], case[1]));
        }
    }
    let unexplained: Vec<_> = differences
        .iter()
        .filter(|difference| {
            !LEGACY_DATES
                .iter()
                .any(|text| difference.starts_with(&format!("\"{text}\" ")))
        })
        .collect();
    assert!(
        unexplained.is_empty(),
        "{} dates differ:\n{}",
        unexplained.len(),
        unexplained
            .iter()
            .map(|d| d.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
}
