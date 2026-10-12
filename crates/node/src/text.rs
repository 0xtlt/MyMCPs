//! The string operations of JavaScript the runner's messages and checks were
//! written with, where Rust's own differ.

use std::sync::LazyLock;

use regex::Regex;

/// What `String.prototype.trim` removes and `\s` matches: Rust's notion of
/// white space leaves U+FEFF out and takes U+0085 in.
fn is_js_whitespace(character: char) -> bool {
    matches!(
        character,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// `value.trim()`.
pub(crate) fn js_trim(value: &str) -> &str {
    value.trim_matches(is_js_whitespace)
}

/// `value.replace(/\s+/g, ' ').trim()`.
pub(crate) fn collapse_whitespace(value: &str) -> String {
    let mut collapsed = String::with_capacity(value.len());
    let mut in_whitespace = false;
    for character in value.chars() {
        if is_js_whitespace(character) {
            in_whitespace = true;
            continue;
        }
        if in_whitespace && !collapsed.is_empty() {
            collapsed.push(' ');
        }
        in_whitespace = false;
        collapsed.push(character);
    }
    collapsed
}

/// `value.length`.
pub(crate) fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

/// `value.slice(0, limit)`, without cutting a character in two.
pub(crate) fn utf16_prefix(value: &str, limit: usize) -> &str {
    let mut units = 0;
    for (index, character) in value.char_indices() {
        units += character.len_utf16();
        if units > limit {
            return &value[..index];
        }
    }
    value
}

/// `value.slice(-limit)`, without cutting a character in two.
pub(crate) fn utf16_suffix(value: &str, limit: usize) -> &str {
    let mut units = 0;
    for (index, character) in value.char_indices().rev() {
        units += character.len_utf16();
        if units > limit {
            return &value[index + character.len_utf8()..];
        }
    }
    value
}

// The pattern of Node's `util.stripVTControlCharacters` (Node 24), itself
// taken from the `ansi-regex` package: an OSC sequence up to its terminator,
// or a CSI sequence.
static ANSI_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?s:\x1B\].*?(?:\x07|\x1B\\|\x{9C}))",
        r"|[\x1B\x{9B}][\[\]()#;?]*",
        r"(?:[0-9]{1,4}(?:[;:][0-9]{0,4})*)?",
        r"[0-9A-PR-TZcf-nq-uy=><~]",
    ))
    .expect("a valid static pattern")
});

/// Node's `util.stripVTControlCharacters`.
pub(crate) fn strip_vt_control_characters(value: &str) -> String {
    if !value.contains(['\u{1B}', '\u{9B}']) {
        return value.to_owned();
    }
    ANSI_PATTERN.replace_all(value, "").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_what_javascript_trims() {
        assert_eq!(
            js_trim(" \t\r\n\u{FEFF}\u{A0}latest\u{2028}\u{3000} "),
            "latest"
        );
        // U+0085 is white space for Rust, not for JavaScript.
        assert_eq!(js_trim("\u{85}1.0.0\u{85}"), "\u{85}1.0.0\u{85}");
        assert_eq!(js_trim("   "), "");
    }

    #[test]
    fn joins_lines_as_the_startup_message_does() {
        assert_eq!(
            collapse_whitespace("  error:\n\tnot  found\r\n\u{A0}at x \n"),
            "error: not found at x"
        );
        assert_eq!(collapse_whitespace(" \n "), "");
        assert_eq!(collapse_whitespace("a\u{85}b"), "a\u{85}b");
    }

    #[test]
    fn counts_and_cuts_in_utf16_code_units() {
        assert_eq!(utf16_len("a😀é"), 4);
        assert_eq!(utf16_prefix("abcdef", 3), "abc");
        assert_eq!(utf16_prefix("abc", 10), "abc");
        assert_eq!(utf16_prefix("a😀b", 2), "a");
        assert_eq!(utf16_prefix("a😀b", 3), "a😀");
        assert_eq!(utf16_suffix("abcdef", 3), "def");
        assert_eq!(utf16_suffix("abc", 10), "abc");
        assert_eq!(utf16_suffix("a😀b", 2), "b");
        assert_eq!(utf16_suffix("a😀b", 3), "😀b");
        assert_eq!(utf16_suffix("abc", 0), "");
    }

    // The expected values were produced with `util.stripVTControlCharacters`
    // of Node 24.21.
    #[test]
    fn strips_terminal_sequences_as_node_does() {
        let cases = [
            ("plain text", "plain text"),
            ("\u{1B}[31merror\u{1B}[0m: boom", "error: boom"),
            ("\u{1B}[1;38;5;196mbold\u{1B}[22;39m", "bold"),
            (
                "\u{1B}]8;;https://example.test\u{7}link\u{1B}]8;;\u{7}",
                "link",
            ),
            ("\u{1B}]0;title\u{1B}\\after", "after"),
            ("\u{9B}2Kcleared", "cleared"),
            ("\u{1B}[?25lhidden\u{1B}[?25h", "hidden"),
            ("lone \u{1B} escape", "lone \u{1B} escape"),
            ("\u{1B}[38:2:1:2:3mcolon", "colon"),
            ("\u{1B}7saved\u{1B}8", "aved"),
            ("\u{1B}]unterminated title", "nterminated title"),
            ("\u{1B}[12345mlong", "mlong"),
            ("\u{1B}]0;multi\nline\u{9C}done", "done"),
            ("a\u{1B}[Kb\u{1B}[2Jc\u{1B}(Bd", "abcd"),
        ];
        for (input, expected) in cases {
            assert_eq!(strip_vt_control_characters(input), expected, "{input:?}");
        }
    }
}
