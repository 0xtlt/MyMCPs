/// What a release adds to the changelog.
#[derive(Debug, Clone, Copy)]
pub struct ChangelogRelease<'a> {
    /// `YYYY-MM-DD`, the heading the entry goes under.
    pub date: &'a str,
    pub release_url: &'a str,
    pub version: &'a str,
}

const CHANGED_HEADING: &str = "### Changed";

/// Adds "Released version" as the first entry under `### Changed` of the
/// release date, creating the section, or the date as the newest one, when the
/// changelog does not have it yet. A changelog that already names the release
/// is returned unchanged.
pub fn update_changelog(changelog: &str, release: &ChangelogRelease<'_>) -> String {
    let version_entry = format!(
        "- Released version [{}]({}).",
        release.version, release.release_url
    );

    if changelog.contains(&version_entry) {
        return changelog.to_string();
    }

    let date_heading = format!("## {}", release.date);

    let Some(date_start) = changelog.find(&date_heading) else {
        let insertion_point = find_dated_heading(changelog).unwrap_or(changelog.len());
        let prefix = trim_end(&changelog[..insertion_point]);
        let suffix = trim_end(trim_start(&changelog[insertion_point..]));
        let release_section = format!("{date_heading}\n\n{CHANGED_HEADING}\n\n{version_entry}");

        let sections: Vec<&str> = [prefix, release_section.as_str(), suffix]
            .into_iter()
            .filter(|section| !section.is_empty())
            .collect();
        return format!("{}\n", sections.join("\n\n"));
    };

    let after_heading = date_start + date_heading.len();
    let date_end = find_dated_heading(&changelog[after_heading..])
        .map_or(changelog.len(), |offset| after_heading + offset);
    let date_section = &changelog[date_start..date_end];

    let Some(changed_start) = date_section.find(CHANGED_HEADING) else {
        return format!(
            "{}\n\n{CHANGED_HEADING}\n\n{version_entry}{}",
            &changelog[..after_heading],
            &changelog[after_heading..]
        );
    };

    let insertion_point = date_start + changed_start + CHANGED_HEADING.len();
    format!(
        "{}\n\n{version_entry}{}",
        &changelog[..insertion_point],
        &changelog[insertion_point..]
    )
}

/// Byte offset of the first line that is exactly `## YYYY-MM-DD`.
fn find_dated_heading(text: &str) -> Option<usize> {
    let mut at_line_start = true;
    for (index, character) in text.char_indices() {
        if at_line_start && is_dated_heading(&text[index..]) {
            return Some(index);
        }
        at_line_start = is_line_terminator(character);
    }
    None
}

fn is_dated_heading(rest: &str) -> bool {
    const SHAPE: &str = "## dddd-dd-dd";
    rest.get(..SHAPE.len())
        .is_some_and(|start| crate::has_shape(start, SHAPE))
        && rest[SHAPE.len()..]
            .chars()
            .next()
            .is_none_or(is_line_terminator)
}

// The release script this replaces was JavaScript. Its notion of a line end
// and of white space is kept, so the same changelog gives the same result.

fn is_line_terminator(character: char) -> bool {
    matches!(character, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn is_white_space(character: char) -> bool {
    const SPACES: [char; 10] = [
        '\t', '\u{0B}', '\u{0C}', ' ', '\u{A0}', '\u{1680}', '\u{202F}', '\u{205F}', '\u{3000}',
        '\u{FEFF}',
    ];
    is_line_terminator(character)
        || SPACES.contains(&character)
        || ('\u{2000}'..='\u{200A}').contains(&character)
}

fn trim_start(text: &str) -> &str {
    text.trim_start_matches(is_white_space)
}

fn trim_end(text: &str) -> &str {
    text.trim_end_matches(is_white_space)
}
