//! JavaScript regular expressions, run by the `regex` crate.
//!
//! The two dialects agree on most of the syntax and disagree on what the
//! shorthands mean: in JavaScript `\d` is `[0-9]` and `\w` is
//! `[A-Za-z0-9_]`, while the `regex` crate reads both over all of Unicode.
//! A pattern such as `/^\d{1,19}$/` guards identifiers that end up in request
//! paths, so it is translated rather than passed through.

use regex::Regex;

/// Why a JavaScript pattern cannot be translated.
#[derive(Debug, Clone, thiserror::Error)]
pub enum JsRegexError {
    /// `g`, `y` and `m` change how a match is searched for in ways the
    /// translation does not reproduce.
    #[error("the `{0}` flag of JavaScript regular expressions is not supported")]
    UnsupportedFlag(char),
    /// Lookaround, backreferences and `\B` have no equivalent in the `regex` crate.
    #[error("{0} cannot be translated for the regex crate")]
    Unsupported(&'static str),
    #[error("invalid JavaScript regular expression: {0}")]
    Syntax(&'static str),
    #[error("invalid translated regular expression: {0}")]
    Regex(#[from] regex::Error),
}

/// A JavaScript regular expression translated for the `regex` crate.
///
/// Supported flags are `i`, `s` and `u`. Translated with JavaScript's
/// meaning: `\d`, `\w`, `\s` and their negations, `\b`, `.`, `\uXXXX`,
/// `\u{...}`, `\xHH`, `\cX`, `\/`, braces that are not a quantifier, empty
/// classes, and case-insensitivity that never crosses from ASCII to
/// non-ASCII letters (so `/k/i` does not match the Kelvin sign).
///
/// Not supported, and reported as an error: lookahead, lookbehind,
/// backreferences, `\B`, and the `g`, `y`, `m` and `d` flags.
///
/// One difference remains. Without the `u` flag JavaScript matches UTF-16
/// code units, so a character outside the Basic Multilingual Plane counts as
/// two for `.`, for a negated class and for a quantifier; here it counts as
/// one.
#[derive(Debug, Clone)]
pub struct JsRegex {
    source: String,
    flags: String,
    regex: Regex,
}

impl JsRegex {
    /// Translate `new RegExp(source, flags)`.
    pub fn new(source: &str, flags: &str) -> Result<Self, JsRegexError> {
        let mut options = Options::default();
        for flag in flags.chars() {
            match flag {
                'i' => options.ignore_case = true,
                's' => options.dot_all = true,
                'u' => options.unicode = true,
                other => return Err(JsRegexError::UnsupportedFlag(other)),
            }
        }
        let translated = Translator::new(source, options).run()?;
        Ok(Self {
            source: source.to_owned(),
            flags: flags.to_owned(),
            regex: Regex::new(&translated)?,
        })
    }

    /// `expression.test(text)`.
    pub fn test(&self, text: &str) -> bool {
        self.regex.is_match(text)
    }

    /// `expression.source`: the JavaScript pattern as it was written.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// `expression.flags`.
    pub fn flags(&self) -> &str {
        &self.flags
    }

    /// The translated expression, for captures and replacements.
    pub fn as_regex(&self) -> &Regex {
        &self.regex
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Options {
    ignore_case: bool,
    dot_all: bool,
    unicode: bool,
}

const DIGITS: &str = "0-9";
const WORD: &str = "0-9A-Za-z_";
const SPACE: &str = r"\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";

enum ClassItem {
    Char(char),
    Range(char, char),
    /// The body of a class, and whether it is negated.
    Set(&'static str, bool),
    Property(String),
}

struct Translator {
    chars: Vec<char>,
    at: usize,
    out: String,
    options: Options,
}

impl Translator {
    fn new(source: &str, options: Options) -> Self {
        Self {
            chars: source.chars().collect(),
            at: 0,
            out: String::new(),
            options,
        }
    }

    /// Without the `u` flag, case is folded letter by letter and only within ASCII.
    fn folds_ascii(&self) -> bool {
        self.options.ignore_case && !self.options.unicode
    }

    fn peek(&self, offset: usize) -> Option<char> {
        self.chars.get(self.at + offset).copied()
    }

    fn next(&mut self) -> Option<char> {
        let c = self.peek(0)?;
        self.at += 1;
        Some(c)
    }

    fn run(mut self) -> Result<String, JsRegexError> {
        if self.options.ignore_case && self.options.unicode {
            self.out.push_str("(?i)");
        }
        while let Some(c) = self.next() {
            match c {
                '\\' => self.escape()?,
                '[' => self.class()?,
                '.' if self.options.dot_all => self.out.push_str("(?s:.)"),
                '.' => self.out.push_str(r"[^\n\r\x{2028}\x{2029}]"),
                '(' => self.group()?,
                ')' | '|' | '*' | '+' | '?' | '^' | '$' => self.out.push(c),
                '{' => match self.quantifier_ahead() {
                    Some(length) => {
                        self.out.push('{');
                        self.out.extend(&self.chars[self.at..self.at + length]);
                        self.at += length;
                    }
                    None => self.literal('{'),
                },
                other => self.literal(other),
            }
        }
        Ok(self.out)
    }

    /// The length of what follows the `{` just read when it opens `{n}`,
    /// `{n,}` or `{n,m}`, closing brace included. Any other brace is a
    /// literal one in JavaScript.
    fn quantifier_ahead(&self) -> Option<usize> {
        let mut offset = 0;
        while self.peek(offset).is_some_and(|c| c.is_ascii_digit()) {
            offset += 1;
        }
        if offset == 0 {
            return None;
        }
        if self.peek(offset) == Some(',') {
            offset += 1;
            while self.peek(offset).is_some_and(|c| c.is_ascii_digit()) {
                offset += 1;
            }
        }
        (self.peek(offset) == Some('}')).then_some(offset + 1)
    }

    fn literal(&mut self, c: char) {
        if self.folds_ascii() && c.is_ascii_alphabetic() {
            self.out.push('[');
            self.out.push(c.to_ascii_lowercase());
            self.out.push(c.to_ascii_uppercase());
            self.out.push(']');
        } else if self.folds_ascii() && !c.is_ascii() {
            self.out.push_str("(?i:");
            self.out.push_str(&regex::escape(&c.to_string()));
            self.out.push(')');
        } else {
            self.out.push_str(&regex::escape(&c.to_string()));
        }
    }

    fn group(&mut self) -> Result<(), JsRegexError> {
        if self.peek(0) != Some('?') {
            self.out.push('(');
            return Ok(());
        }
        match (self.peek(1), self.peek(2)) {
            (Some(':'), _) => {
                self.at += 2;
                self.out.push_str("(?:");
                Ok(())
            }
            (Some('=' | '!'), _) => Err(JsRegexError::Unsupported("lookahead")),
            (Some('<'), Some('=' | '!')) => Err(JsRegexError::Unsupported("lookbehind")),
            (Some('<'), _) => {
                let start = self.at + 2;
                let length = self.chars[start..]
                    .iter()
                    .position(|c| *c == '>')
                    .ok_or(JsRegexError::Syntax("unterminated group name"))?;
                self.out.push_str("(?P<");
                self.out.extend(&self.chars[start..=start + length]);
                self.at = start + length + 1;
                Ok(())
            }
            _ => Err(JsRegexError::Syntax("unknown group")),
        }
    }

    fn hex(&mut self, digits: usize) -> Option<u32> {
        let mut value = 0;
        for offset in 0..digits {
            value = value * 16 + self.peek(offset)?.to_digit(16)?;
        }
        self.at += digits;
        Some(value)
    }

    /// The character after `\u`, or `None` when the escape is not one, which
    /// JavaScript then reads as a plain `u`.
    fn unicode_escape(&mut self) -> Result<Option<char>, JsRegexError> {
        if self.options.unicode && self.peek(0) == Some('{') {
            let start = self.at + 1;
            let end = self.chars[start..]
                .iter()
                .position(|c| *c == '}')
                .ok_or(JsRegexError::Syntax("unterminated \\u{...}"))?;
            let digits: String = self.chars[start..start + end].iter().collect();
            self.at = start + end + 1;
            let code = u32::from_str_radix(&digits, 16)
                .map_err(|_| JsRegexError::Syntax("invalid \\u{...}"))?;
            return char::from_u32(code)
                .map(Some)
                .ok_or(JsRegexError::Syntax("invalid code point"));
        }
        let Some(unit) = self.hex(4) else {
            return Ok(None);
        };
        if (0xD800..0xDC00).contains(&unit)
            && self.peek(0) == Some('\\')
            && self.peek(1) == Some('u')
        {
            let saved = self.at;
            self.at += 2;
            if let Some(low) = self.hex(4).filter(|low| (0xDC00..0xE000).contains(low)) {
                let code = 0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00);
                return Ok(char::from_u32(code));
            }
            self.at = saved;
        }
        char::from_u32(unit)
            .map(Some)
            .ok_or(JsRegexError::Unsupported("a lone surrogate"))
    }

    /// An escape that stands for one character, in or out of a class.
    fn character_escape(&mut self, c: char) -> Result<Option<char>, JsRegexError> {
        Ok(Some(match c {
            't' => '\t',
            'n' => '\n',
            'r' => '\r',
            'f' => '\u{000C}',
            'v' => '\u{000B}',
            '0' if !self.peek(0).is_some_and(|next| next.is_ascii_digit()) => '\0',
            'x' => match self.hex(2) {
                Some(code) => char::from_u32(code).unwrap_or('x'),
                None => 'x',
            },
            'u' => self.unicode_escape()?.unwrap_or('u'),
            'c' => match self.peek(0).filter(char::is_ascii_alphabetic) {
                Some(letter) => {
                    self.at += 1;
                    char::from_u32(u32::from(letter) % 32).unwrap_or('c')
                }
                None => return Err(JsRegexError::Syntax("invalid \\c escape")),
            },
            _ => return Ok(None),
        }))
    }

    fn property(&mut self, letter: char) -> Result<String, JsRegexError> {
        if self.peek(0) != Some('{') {
            return Err(JsRegexError::Syntax("invalid property escape"));
        }
        let end = self.chars[self.at..]
            .iter()
            .position(|c| *c == '}')
            .ok_or(JsRegexError::Syntax("unterminated property escape"))?;
        let body: String = self.chars[self.at..=self.at + end].iter().collect();
        self.at += end + 1;
        Ok(format!("\\{letter}{body}"))
    }

    fn escape(&mut self) -> Result<(), JsRegexError> {
        let c = self
            .next()
            .ok_or(JsRegexError::Syntax("\\ at end of pattern"))?;
        match c {
            'd' => self.out.push_str("[0-9]"),
            'D' => self.out.push_str("[^0-9]"),
            'w' => self.out.push_str("[0-9A-Za-z_]"),
            'W' => self.out.push_str("[^0-9A-Za-z_]"),
            's' => {
                self.out.push('[');
                self.out.push_str(SPACE);
                self.out.push(']');
            }
            'S' => {
                self.out.push_str("[^");
                self.out.push_str(SPACE);
                self.out.push(']');
            }
            'b' => self.out.push_str(r"(?-u:\b)"),
            'B' => return Err(JsRegexError::Unsupported("\\B")),
            '1'..='9' => return Err(JsRegexError::Unsupported("a backreference")),
            'k' if self.peek(0) == Some('<') => {
                return Err(JsRegexError::Unsupported("a backreference"));
            }
            'p' | 'P' if self.options.unicode => {
                let property = self.property(c)?;
                self.out.push_str(&property);
            }
            other => match self.character_escape(other)? {
                Some(character) => self.literal(character),
                None => self.literal(other),
            },
        }
        Ok(())
    }

    fn class_escape(&mut self) -> Result<ClassItem, JsRegexError> {
        let c = self
            .next()
            .ok_or(JsRegexError::Syntax("\\ at end of pattern"))?;
        Ok(match c {
            'd' => ClassItem::Set(DIGITS, false),
            'D' => ClassItem::Set(DIGITS, true),
            'w' => ClassItem::Set(WORD, false),
            'W' => ClassItem::Set(WORD, true),
            's' => ClassItem::Set(SPACE, false),
            'S' => ClassItem::Set(SPACE, true),
            'b' => ClassItem::Char('\u{0008}'),
            'p' | 'P' if self.options.unicode => ClassItem::Property(self.property(c)?),
            other => ClassItem::Char(self.character_escape(other)?.unwrap_or(other)),
        })
    }

    fn class_atom(&mut self, c: char) -> Result<ClassItem, JsRegexError> {
        if c == '\\' {
            self.class_escape()
        } else {
            Ok(ClassItem::Char(c))
        }
    }

    fn class(&mut self) -> Result<(), JsRegexError> {
        let negated = self.peek(0) == Some('^');
        if negated {
            self.at += 1;
        }
        let mut items = Vec::new();
        loop {
            let c = self
                .next()
                .ok_or(JsRegexError::Syntax("unterminated character class"))?;
            if c == ']' {
                break;
            }
            let atom = self.class_atom(c)?;
            let opens_range = matches!(atom, ClassItem::Char(_))
                && self.peek(0) == Some('-')
                && self.peek(1).is_some_and(|end| end != ']');
            let ClassItem::Char(low) = atom else {
                items.push(atom);
                continue;
            };
            if !opens_range {
                items.push(ClassItem::Char(low));
                continue;
            }
            self.at += 1;
            let end = self
                .next()
                .ok_or(JsRegexError::Syntax("unterminated character class"))?;
            match self.class_atom(end)? {
                ClassItem::Char(high) if low <= high => items.push(ClassItem::Range(low, high)),
                ClassItem::Char(_) => {
                    return Err(JsRegexError::Syntax(
                        "range out of order in character class",
                    ));
                }
                // A class cannot end a range: the dash is then a dash.
                set => {
                    items.push(ClassItem::Char(low));
                    items.push(ClassItem::Char('-'));
                    items.push(set);
                }
            }
        }

        if items.is_empty() {
            // `[]` matches nothing and `[^]` matches anything.
            self.out
                .push_str(if negated { "(?s:.)" } else { r"[^\s\S]" });
            return Ok(());
        }
        self.out.push('[');
        if negated {
            self.out.push('^');
        }
        let fold = self.folds_ascii();
        for item in items {
            match item {
                ClassItem::Char(c) => {
                    self.out.push_str(&regex::escape(&c.to_string()));
                    if fold && c.is_ascii_alphabetic() {
                        let other = if c.is_ascii_lowercase() {
                            c.to_ascii_uppercase()
                        } else {
                            c.to_ascii_lowercase()
                        };
                        self.out.push(other);
                    }
                }
                ClassItem::Range(low, high) => {
                    self.push_range(low, high);
                    if fold {
                        // The letters of the range, in the other case.
                        if let Some((from, to)) = overlap(low, high, 'a', 'z') {
                            self.push_range(from.to_ascii_uppercase(), to.to_ascii_uppercase());
                        }
                        if let Some((from, to)) = overlap(low, high, 'A', 'Z') {
                            self.push_range(from.to_ascii_lowercase(), to.to_ascii_lowercase());
                        }
                    }
                }
                ClassItem::Set(body, false) => self.out.push_str(body),
                ClassItem::Set(body, true) => {
                    self.out.push_str("[^");
                    self.out.push_str(body);
                    self.out.push(']');
                }
                ClassItem::Property(property) => self.out.push_str(&property),
            }
        }
        self.out.push(']');
        Ok(())
    }

    fn push_range(&mut self, low: char, high: char) {
        self.out.push_str(&regex::escape(&low.to_string()));
        self.out.push('-');
        self.out.push_str(&regex::escape(&high.to_string()));
    }
}

fn overlap(low: char, high: char, from: char, to: char) -> Option<(char, char)> {
    let (start, end) = (low.max(from), high.min(to));
    (start <= end).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(source: &str, flags: &str, text: &str) -> bool {
        JsRegex::new(source, flags).unwrap().test(text)
    }

    #[test]
    fn reads_the_shorthands_as_javascript_does() {
        assert!(matches(r"^\d+$", "", "123"));
        // Fullwidth and Arabic-Indic digits are digits for the regex crate only.
        assert!(!matches(r"^\d+$", "", "５"));
        assert!(!matches(r"^\d+$", "", "٣"));
        assert!(!matches(r"^\w+$", "", "é"));
        assert!(matches(r"^[\w.+-]+$", "", "a.b+c-d_1"));
        assert!(matches(r"^\s$", "", "\u{FEFF}"));
        assert!(!matches(r"^\s$", "", "\u{0085}"));
    }

    #[test]
    fn folds_case_within_ascii_only() {
        assert!(matches("^[a-z]{2}$", "i", "FR"));
        assert!(matches("^select\\s", "i", "SeLeCt x"));
        // U+017F and U+212A fold to `s` and `k` in Unicode, not in JavaScript without `u`.
        assert!(!matches("^[a-z]$", "i", "\u{017F}"));
        assert!(!matches("^k$", "i", "\u{212A}"));
    }

    #[test]
    fn refuses_what_it_cannot_translate() {
        assert!(matches!(
            JsRegex::new("^(?!__).+?__", "s"),
            Err(JsRegexError::Unsupported("lookahead"))
        ));
        assert!(matches!(
            JsRegex::new(r"(a)\1", ""),
            Err(JsRegexError::Unsupported(_))
        ));
        assert!(matches!(
            JsRegex::new("a", "g"),
            Err(JsRegexError::UnsupportedFlag('g'))
        ));
    }

    #[test]
    fn keeps_the_source_as_written() {
        let expression = JsRegex::new(r"^\d{3}-?\d{4}$", "").unwrap();
        assert_eq!(expression.source(), r"^\d{3}-?\d{4}$");
        assert!(expression.test("123-4567"));
    }
}
