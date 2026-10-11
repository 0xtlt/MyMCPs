use std::path::Path;

use crate::changelog::{ChangelogRelease, update_changelog};
use crate::{Error, Result, bump_version, has_shape, read, write};
use crate::{set_workspace_version, update_lockfile, workspace_version};

#[derive(Debug, Clone, Copy)]
pub struct ReleaseOptions<'a> {
    /// `major`, `minor` or `patch`.
    pub bump: &'a str,
    /// `YYYY-MM-DD`, the day of the release.
    pub date: &'a str,
    /// `owner/name` of the GitHub repository, for the link in the changelog.
    pub repository: &'a str,
    /// The directory that holds `Cargo.toml`, `Cargo.lock` and `CHANGELOG.md`.
    pub root_directory: &'a Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRelease {
    pub release_url: String,
    pub tag: String,
    pub version: String,
}

impl PreparedRelease {
    pub fn to_json(&self) -> String {
        format!(
            "{{\"releaseUrl\":{},\"tag\":{},\"version\":{}}}",
            json_string(&self.release_url),
            json_string(&self.tag),
            json_string(&self.version)
        )
    }
}

/// Moves the workspace to its next version: `Cargo.toml`, the workspace
/// crates in `Cargo.lock`, and a "Released version" entry in `CHANGELOG.md`.
/// Nothing is written when any of the three cannot be updated.
pub fn prepare_release(options: &ReleaseOptions<'_>) -> Result<PreparedRelease> {
    let ReleaseOptions {
        bump,
        date,
        repository,
        root_directory,
    } = *options;

    if !has_shape(date, "dddd-dd-dd") {
        return Err(Error::Invalid(format!(
            "Expected date in YYYY-MM-DD format, received \"{date}\""
        )));
    }

    if !is_repository(repository) {
        return Err(Error::Invalid(format!(
            "Expected repository in owner/name format, received \"{repository}\""
        )));
    }

    let manifest_path = root_directory.join("Cargo.toml");
    let lockfile_path = root_directory.join("Cargo.lock");
    let changelog_path = root_directory.join("CHANGELOG.md");
    let manifest = read(&manifest_path)?;
    let lockfile = read(&lockfile_path)?;
    let changelog = read(&changelog_path)?;

    let current_version = workspace_version(&manifest)?;
    let version = bump_version(current_version, bump)?;
    let tag = format!("v{version}");
    let release_url = format!("https://github.com/{repository}/releases/tag/{tag}");
    let updated_manifest = set_workspace_version(&manifest, &version)?;
    let updated_lockfile = update_lockfile(&lockfile, current_version, &version)?;
    let updated_changelog = update_changelog(
        &changelog,
        &ChangelogRelease {
            date,
            release_url: &release_url,
            version: &version,
        },
    );

    write(&manifest_path, &updated_manifest)?;
    write(&lockfile_path, &updated_lockfile)?;
    write(&changelog_path, &updated_changelog)?;

    Ok(PreparedRelease {
        release_url,
        tag,
        version,
    })
}

fn is_repository(repository: &str) -> bool {
    let is_name = |name: &str| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
    };
    repository
        .split_once('/')
        .is_some_and(|(owner, name)| is_name(owner) && is_name(name))
}

fn json_string(value: &str) -> String {
    let mut json = String::with_capacity(value.len() + 2);
    json.push('"');
    for character in value.chars() {
        match character {
            '"' => json.push_str("\\\""),
            '\\' => json.push_str("\\\\"),
            control if u32::from(control) < 0x20 => {
                json.push_str(&format!("\\u{:04x}", u32::from(control)));
            }
            other => json.push(other),
        }
    }
    json.push('"');
    json
}
