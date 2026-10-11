//! Accounts and money: the port of the first group of
//! `tests/unit/builtin_google_ads.spec.ts`, then the same functions against
//! what the TypeScript answers for many more values.
//!
//! `fixtures/format.json` was written by Node from
//! `app/services/builtin/google_ads/format.ts`. The script that wrote it is
//! not part of the repository, since it runs Node on the TypeScript app.

use mymcps_google_ads::api::resource_id;
use mymcps_google_ads::format::{
    customer_label, customer_number, from_micros, language_code, money, percent, to_micros,
};
use serde_json::{Value, json};

const FIXTURE: &str = include_str!("fixtures/format.json");

fn fixture(part: &str) -> Vec<Value> {
    let fixture: Value = serde_json::from_str(FIXTURE).unwrap();
    fixture[part].as_array().unwrap().clone()
}

/// A JavaScript number the fixture wrote as text, since JSON has no `NaN`.
fn written_number(text: &str) -> f64 {
    match text {
        "NaN" => f64::NAN,
        "Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        text => text.parse().unwrap(),
    }
}

#[test]
fn writes_account_ids_the_way_the_api_takes_them_and_the_way_people_read_them() {
    assert_eq!(customer_number("123-456-7890"), "1234567890");
    assert_eq!(customer_number("1234567890"), "1234567890");
    assert_eq!(customer_label("1234567890"), "123-456-7890");
}

#[test]
fn converts_amounts_to_micros_without_the_errors_of_binary_fractions() {
    assert_eq!(to_micros(250.0), "250000000");
    assert_eq!(to_micros(2.5), "2500000");
    assert_eq!(to_micros(0.07), "70000");
    // 1.1 * 1,000,000 is 1100000.0000000002 in floating point.
    assert_eq!(to_micros(1.1), "1100000");
    assert_eq!(to_micros(19.999), "20000000");

    assert_eq!(from_micros(Some(&json!("2500000"))), Some(2.5));
    assert_eq!(from_micros(Some(&json!(0))), Some(0.0));
    assert_eq!(from_micros(None), None);
    assert_eq!(from_micros(Some(&json!(""))), None);
}

#[test]
fn says_amounts_in_the_currency_of_the_account() {
    assert_eq!(money(250.0, "EUR"), "€250.00");
    assert_eq!(money(2.5, "USD"), "$2.50");
    assert_eq!(money(7600.0, "EUR"), "€7,600.00");
    assert_eq!(money(1500.0, "JPY"), "¥1,500");
    assert_eq!(money(12.0, "not a currency"), "12.00 not a currency");
}

#[test]
fn says_amounts_as_intl_does_for_every_currency() {
    let cases = fixture("money");
    assert!(cases.len() > 1000);
    for case in cases {
        let (amount, currency, said) = (
            case[0].as_str().unwrap(),
            case[1].as_str().unwrap(),
            case[2].as_str().unwrap(),
        );
        assert_eq!(
            money(written_number(amount), currency),
            said,
            "money({amount}, {currency:?})"
        );
    }
}

#[test]
fn converts_amounts_as_the_typescript_does() {
    for case in fixture("toMicros") {
        let amount = case[0].as_f64().unwrap();
        assert_eq!(
            to_micros(amount),
            case[1].as_str().unwrap(),
            "toMicros({amount})"
        );
    }
    for case in fixture("fromMicros") {
        let micros = case[0].as_array().unwrap().first();
        let expected = case[1]
            .as_array()
            .unwrap()
            .first()
            .map(|amount| amount.as_f64().unwrap());
        assert_eq!(from_micros(micros), expected, "fromMicros({micros:?})");
    }
    for case in fixture("percent") {
        let ratio = case[0].as_array().unwrap().first();
        let share = percent(ratio);
        match case[1].as_array().unwrap().first() {
            None => assert_eq!(share, None, "percent({ratio:?})"),
            Some(Value::String(_)) => assert!(share.unwrap().is_nan(), "percent({ratio:?})"),
            Some(expected) => assert_eq!(share, expected.as_f64(), "percent({ratio:?})"),
        }
    }
}

#[test]
fn writes_identifiers_and_language_codes_as_the_typescript_does() {
    type Written = fn(&str) -> String;
    let functions: [(&str, Written); 3] = [
        ("customerNumber", customer_number),
        ("customerLabel", customer_label),
        ("languageCode", language_code),
    ];
    for (name, function) in functions {
        for case in fixture(name) {
            let value = case[0].as_str().unwrap();
            assert_eq!(
                function(value),
                case[1].as_str().unwrap(),
                "{name}({value:?})"
            );
        }
    }
    for case in fixture("resourceId") {
        let value = case[0].as_str().unwrap();
        assert_eq!(
            resource_id(Some(value)).as_deref(),
            case[1].as_str(),
            "resourceId({value:?})"
        );
    }
    assert_eq!(resource_id(None), None);
}
