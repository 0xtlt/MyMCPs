use crate::{Error, Result, has_shape};

/// `major.minor.patch`, each a decimal number without a leading zero and
/// nothing after it: a prerelease or build suffix is not a stable version.
fn parse_stable(version: &str) -> Option<[u64; 3]> {
    let mut numbers = [0u64; 3];
    let mut parts = version.split('.');
    for number in &mut numbers {
        let part = parts.next()?;
        let canonical = part == "0" || (!part.is_empty() && !part.starts_with('0'));
        if !canonical || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        *number = part.parse().ok()?;
    }
    parts.next().is_none().then_some(numbers)
}

pub fn bump_version(current_version: &str, bump: &str) -> Result<String> {
    let not_stable = || {
        Error::Invalid(format!(
            "Expected a stable semantic version, received \"{current_version}\""
        ))
    };
    let [major, minor, patch] = parse_stable(current_version).ok_or_else(not_stable)?;

    let next = match bump {
        "major" => major.checked_add(1).map(|major| [major, 0, 0]),
        "minor" => minor.checked_add(1).map(|minor| [major, minor, 0]),
        "patch" => patch.checked_add(1).map(|patch| [major, minor, patch]),
        _ => {
            return Err(Error::Invalid(format!(
                "Expected bump to be major, minor, or patch, received \"{bump}\""
            )));
        }
    };
    let [major, minor, patch] = next.ok_or_else(not_stable)?;

    Ok(format!("{major}.{minor}.{patch}"))
}

/// The version of a nightly built from `short_sha` on `date`: a prerelease of
/// the next patch version, so it sorts after the current stable release.
pub fn nightly_version(current_version: &str, date: &str, short_sha: &str) -> Result<String> {
    if !has_shape(date, "dddddddd") {
        return Err(Error::Invalid(format!(
            "Expected nightly date in YYYYMMDD format, received \"{date}\""
        )));
    }

    let is_sha = (7..=40).contains(&short_sha.len())
        && short_sha
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
    if !is_sha {
        return Err(Error::Invalid(format!(
            "Expected a 7-40 character lowercase Git SHA, received \"{short_sha}\""
        )));
    }

    Ok(format!(
        "{}-nightly.{date}.g{short_sha}",
        bump_version(current_version, "patch")?
    ))
}
