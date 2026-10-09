//! Which mailbox is the Sent one, the Trash, the Drafts: the port of
//! `special-use.js` of imapflow 2.2.4. A server that implements RFC 6154
//! says so with a flag. For the others the name of the mailbox is looked up
//! among the names mail software gives these mailboxes, in many languages.

use std::collections::HashSet;

use mymcps_vine::js;
use unicode_normalization::UnicodeNormalization;

use super::special_use_names::{GENERIC_TOKENS, NAMES};

/// The uses a server can flag a mailbox with, in the order they are looked for.
const FLAGS: [&str; 7] = [
    "\\All",
    "\\Archive",
    "\\Drafts",
    "\\Flagged",
    "\\Junk",
    "\\Sent",
    "\\Trash",
];

/// How a special use was found out, from the most to the least trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Source {
    /// The server flagged the mailbox.
    Extension,
    /// The mailbox has a name these mailboxes are known by.
    Name,
    /// The name has one such word in it, among words that say nothing.
    NameGuess,
}

fn by_name(name: &str) -> Option<&'static str> {
    NAMES
        .binary_search_by(|(known, _)| (*known).cmp(name))
        .ok()
        .map(|index| NAMES[index].1)
}

/// `specialUse`: the use of a listed mailbox, from its flags or its name.
pub(super) fn special_use(
    has_extension: bool,
    flags: &HashSet<String>,
    name: &str,
) -> Option<(&'static str, Source)> {
    if has_extension && let Some(flag) = FLAGS.iter().find(|flag| flags.contains(**flag)) {
        return Some((flag, Source::Extension));
    }

    let name: String = js::trim(&name.to_lowercase().replace('\u{200e}', ""))
        .nfkc()
        .collect();
    if let Some(flag) = by_name(&name) {
        return Some((flag, Source::Name));
    }

    // "Sent mail" or "[Trash]": one word that means something, among others that do not.
    let is_separator =
        |character: char| js::is_whitespace(character) || "-_/.,()[]".contains(character);
    let core: Vec<&str> = name
        .split(is_separator)
        .filter(|token| !token.is_empty() && GENERIC_TOKENS.binary_search(token).is_err())
        .collect();
    match core.as_slice() {
        [word] if *word != name => by_name(word).map(|flag| (flag, Source::NameGuess)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_special_mailboxes_by_flag_then_by_name() {
        let flagged: HashSet<String> = ["\\HasNoChildren".to_owned(), "\\Trash".to_owned()].into();
        let none = HashSet::new();

        assert_eq!(
            special_use(true, &flagged, "Whatever"),
            Some(("\\Trash", Source::Extension))
        );
        // Without the extension a flag says nothing, but a name still does.
        assert_eq!(special_use(false, &flagged, "Whatever"), None);
        assert_eq!(
            special_use(true, &none, "Sent Messages"),
            Some(("\\Sent", Source::Name))
        );
        assert_eq!(
            special_use(false, &none, " Deleted Messages\u{200e} "),
            Some(("\\Trash", Source::Name))
        );
        assert_eq!(
            special_use(false, &none, "Entwürfe"),
            Some(("\\Drafts", Source::Name))
        );
        assert_eq!(
            special_use(false, &none, "[Junk] mail"),
            Some(("\\Junk", Source::NameGuess))
        );
        assert_eq!(special_use(false, &none, "Projects"), None);
        assert_eq!(special_use(false, &none, "Sent drafts"), None);

        assert!(
            NAMES.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "the names are sorted"
        );
        assert!(
            GENERIC_TOKENS.windows(2).all(|pair| pair[0] < pair[1]),
            "the words are sorted"
        );
    }
}
