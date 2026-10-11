//! How Google Ads writes accounts and money, and how people read them.

use mymcps_vine as vine;
use serde_json::Value;

use crate::js;

const MICROS: f64 = 1_000_000.0;

/// `1234567890` for `123-456-7890`: the form the API takes.
pub fn customer_number(value: &str) -> String {
    value.replace('-', "")
}

/// `123-456-7890`: the form Google Ads shows, and people recognize.
pub fn customer_label(customer_id: &str) -> String {
    if customer_id.len() == 10 && customer_id.bytes().all(|byte| byte.is_ascii_digit()) {
        format!(
            "{}-{}-{}",
            &customer_id[..3],
            &customer_id[3..6],
            &customer_id[6..]
        )
    } else {
        customer_id.to_owned()
    }
}

/// A language code as Google Ads writes it: `fr`, and with a region `pt_BR`.
pub fn language_code(value: &str) -> String {
    let mut parts = vine::js::trim(value).split(['_', '-']);
    let language = parts.next().unwrap_or_default().to_lowercase();
    match parts.next() {
        Some(region) if !region.is_empty() => format!("{language}_{}", region.to_uppercase()),
        _ => language,
    }
}

/// An amount in the account's currency as the API takes it. Rounded to the
/// cent, which is the billable unit of most currencies: the API refuses an
/// amount that is not a multiple of the currency's own.
pub fn to_micros(amount: f64) -> String {
    vine::js::number_to_string(js::math_round(amount * 100.0) * (MICROS / 100.0))
}

/// `None` for an amount the API left out. `micros` is the field as Google
/// answered it: a text for a 64-bit number, or nothing.
pub fn from_micros(micros: Option<&Value>) -> Option<f64> {
    let micros = js::defined(micros).filter(|micros| micros.as_str() != Some(""))?;
    let amount = vine::js::to_number(Some(micros)) / MICROS;
    amount.is_finite().then_some(amount)
}

/// An amount as a person reads it, such as `€250.00`. The currency comes from
/// the account, never from the agent.
///
/// The TypeScript asks `Intl.NumberFormat('en', { style: 'currency' })`,
/// which Rust has no counterpart for: the signs and the decimals of the
/// currencies are the ones of ICU 78 (CLDR 48), which Node 24 ships.
pub fn money(amount: f64, currency_code: &str) -> String {
    match Currency::of(currency_code) {
        Some(currency) => currency.format(amount),
        // Not a currency `Intl` knows: say the code as Google gave it.
        None => format!("{} {currency_code}", to_fixed(amount, 2)),
    }
}

/// A share, such as a click-through rate, as a percentage with two decimals.
/// `ratio` is the field as Google answered it.
pub fn percent(ratio: Option<&Value>) -> Option<f64> {
    ratio.map(|ratio| js::math_round(vine::js::to_number(Some(ratio)) * 10_000.0) / 100.0)
}

/// The currencies English has a sign for. Any other is written as its code,
/// such as `CHF 12.00`.
const SIGNS: &[(&str, &str)] = &[
    ("AUD", "A$"),
    ("BRL", "R$"),
    ("CAD", "CA$"),
    ("CNY", "CN¥"),
    ("EUR", "€"),
    ("GBP", "£"),
    ("HKD", "HK$"),
    ("ILS", "₪"),
    ("INR", "₹"),
    ("JPY", "¥"),
    ("KRW", "₩"),
    ("MXN", "MX$"),
    ("NZD", "NZ$"),
    ("PHP", "₱"),
    ("TWD", "NT$"),
    ("USD", "$"),
    ("VND", "₫"),
    ("XAF", "FCFA"),
    ("XCD", "EC$"),
    ("XCG", "Cg."),
    ("XOF", "F\u{202f}CFA"),
    ("XPF", "CFPF"),
    ("XXX", "¤"),
];

/// The currencies that are not written with two decimals.
const NO_DECIMALS: &[&str] = &[
    "ADP", "AFN", "ALL", "BIF", "BYR", "CLP", "COP", "DJF", "ESP", "GNF", "HUF", "IDR", "IQD",
    "IRR", "ISK", "ITL", "JPY", "KMF", "KPW", "KRW", "LAK", "LBP", "LUF", "MGA", "MGF", "MMK",
    "MRO", "PKR", "PYG", "RWF", "SLL", "SOS", "STD", "SYP", "TMM", "TRL", "UGX", "UYI", "VND",
    "VUV", "XAF", "XOF", "XPF", "YER", "ZMK", "ZWD",
];
const THREE_DECIMALS: &[&str] = &["BHD", "JOD", "KWD", "LYD", "OMR", "TND"];
const FOUR_DECIMALS: &[&str] = &["CLF", "UYW"];

struct Currency {
    /// What stands before the digits.
    sign: String,
    decimals: usize,
}

impl Currency {
    /// `None` for what `Intl` refuses as a currency code: anything but three
    /// letters. It takes three letters it has never heard of.
    fn of(code: &str) -> Option<Self> {
        if code.len() != 3 || !code.bytes().all(|byte| byte.is_ascii_alphabetic()) {
            return None;
        }
        let code = code.to_ascii_uppercase();
        let sign = SIGNS
            .iter()
            .find(|(known, _)| *known == code)
            .map_or(code.as_str(), |(_, sign)| sign)
            .to_owned();
        let decimals = match code.as_str() {
            code if NO_DECIMALS.contains(&code) => 0,
            code if THREE_DECIMALS.contains(&code) => 3,
            code if FOUR_DECIMALS.contains(&code) => 4,
            _ => 2,
        };
        Some(Self { sign, decimals })
    }

    fn format(&self, amount: f64) -> String {
        let minus = if amount.is_sign_negative() { "-" } else { "" };
        if amount.is_nan() {
            return format!("{}NaN", self.sign);
        }
        if amount.is_infinite() {
            return format!("{minus}{}∞", self.sign);
        }
        // `Intl` rounds the number as it is written, not the binary
        // fraction behind it: 1.005 is $1.01.
        let (whole, fraction) = rounded(shortest_digits(amount.abs()), self.decimals);
        let whole = grouped(&whole);
        let digits = if fraction.is_empty() {
            whole
        } else {
            format!("{whole}.{fraction}")
        };
        // A sign that ends in a letter or a period is kept apart from the digits.
        let apart = self
            .sign
            .ends_with(|last: char| last.is_alphanumeric() || last == '.');
        format!(
            "{minus}{}{}{digits}",
            self.sign,
            if apart { "\u{a0}" } else { "" }
        )
    }
}

/// The digits before and after the decimal point.
type Decimal = (String, String);

/// The shortest digits that read back as this number, which is how
/// JavaScript writes one. Read from the JSON of the number: when two such
/// writings are as close, `serde_json` settles for the same one as
/// JavaScript, and Rust's own formatting for the other.
fn shortest_digits(amount: f64) -> Decimal {
    let written = serde_json::Number::from_f64(amount)
        .map(|number| number.to_string())
        .unwrap_or_default();
    let (mantissa, exponent) = written.split_once('e').unwrap_or((&written, "0"));
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{whole}{fraction}");
    let point =
        i64::try_from(whole.len()).unwrap_or(i64::MAX) + exponent.parse::<i64>().unwrap_or(0);
    let length = i64::try_from(digits.len()).unwrap_or(i64::MAX);
    if point <= 0 {
        let zeros = usize::try_from(-point).unwrap_or(0);
        ("0".to_owned(), format!("{}{digits}", "0".repeat(zeros)))
    } else if point >= length {
        let zeros = usize::try_from(point - length).unwrap_or(0);
        (format!("{digits}{}", "0".repeat(zeros)), String::new())
    } else {
        let split = usize::try_from(point).unwrap_or(0);
        (digits[..split].to_owned(), digits[split..].to_owned())
    }
}

/// Every digit of the number, which a binary fraction has at most 1074 of
/// after the point.
fn exact_digits(amount: f64) -> Decimal {
    let written = format!("{amount:.1100}");
    match written.split_once('.') {
        Some((whole, fraction)) => (whole.to_owned(), fraction.to_owned()),
        None => (written, String::new()),
    }
}

/// Round to `decimals` digits after the point, a half going up.
fn rounded((whole, fraction): Decimal, decimals: usize) -> Decimal {
    let up = fraction
        .as_bytes()
        .get(decimals)
        .is_some_and(|digit| *digit >= b'5');
    let mut kept: Vec<u8> = fraction
        .bytes()
        .chain(std::iter::repeat(b'0'))
        .take(decimals)
        .collect();
    let mut whole = whole.into_bytes();
    if up {
        let mut carry = true;
        for digit in kept.iter_mut().rev().chain(whole.iter_mut().rev()) {
            if *digit == b'9' {
                *digit = b'0';
            } else {
                *digit += 1;
                carry = false;
                break;
            }
        }
        if carry {
            whole.insert(0, b'1');
        }
    }
    // Only ASCII digits were put in either.
    (
        String::from_utf8_lossy(&whole).into_owned(),
        String::from_utf8_lossy(&kept).into_owned(),
    )
}

/// `1234567` as `1,234,567`.
fn grouped(whole: &str) -> String {
    let mut grouped = String::with_capacity(whole.len() + whole.len() / 3);
    for (index, digit) in whole.chars().enumerate() {
        if index > 0 && (whole.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// `amount.toFixed(decimals)`, which rounds the binary fraction itself:
/// 1.005 is `1.00`.
pub(crate) fn to_fixed(amount: f64, decimals: usize) -> String {
    if amount.is_nan() || amount.abs() >= 1e21 {
        return vine::js::number_to_string(amount);
    }
    let minus = if amount < 0.0 { "-" } else { "" };
    let (whole, fraction) = rounded(exact_digits(amount.abs()), decimals);
    if fraction.is_empty() {
        format!("{minus}{whole}")
    } else {
        format!("{minus}{whole}.{fraction}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_number_with_a_fixed_number_of_decimals_as_to_fixed_does() {
        // What Node answers for `amount.toFixed(2)`.
        for (amount, written) in [
            (12.0, "12.00"),
            (0.125, "0.13"),
            (2.5, "2.50"),
            (-2.5, "-2.50"),
            (0.005, "0.01"),
            (1.005, "1.00"),
            (-0.001, "-0.00"),
            (-0.0, "0.00"),
            (1.45, "1.45"),
            (8.345, "8.35"),
            (0.000_001, "0.00"),
            (1_234_567.891, "1234567.89"),
            (0.995, "0.99"),
            (-0.995, "-0.99"),
            (5e-324, "0.00"),
            (999.995, "1000.00"),
            (1e21, "1e+21"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "Infinity"),
        ] {
            assert_eq!(to_fixed(amount, 2), written, "{amount}");
        }
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(1.5, 0), "2");
    }

    #[test]
    fn groups_thousands() {
        assert_eq!(grouped("0"), "0");
        assert_eq!(grouped("999"), "999");
        assert_eq!(grouped("1000"), "1,000");
        assert_eq!(grouped("1234567"), "1,234,567");
    }
}
