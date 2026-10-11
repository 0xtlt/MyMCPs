//! The two files that carry the version of the workspace: the root
//! `Cargo.toml` and `Cargo.lock`. Both are edited as text, so comments and
//! layout are kept and no TOML parser has to be built in the release job.

use std::ops::Range;

use crate::{Error, Result};

/// The version every crate of the workspace inherits: `version` in the
/// `[workspace.package]` table of the root `Cargo.toml`.
pub fn workspace_version(manifest: &str) -> Result<&str> {
    Ok(&manifest[workspace_version_range(manifest)?])
}

/// `manifest` with the workspace version replaced, and nothing else touched.
pub fn set_workspace_version(manifest: &str, version: &str) -> Result<String> {
    let range = workspace_version_range(manifest)?;
    Ok(format!(
        "{}{version}{}",
        &manifest[..range.start],
        &manifest[range.end..]
    ))
}

fn workspace_version_range(manifest: &str) -> Result<Range<usize>> {
    let mut in_workspace_package = false;
    let mut line_start = 0;

    for line in manifest.split_inclusive('\n') {
        let start = line_start;
        line_start += line.len();

        if line.trim_start().starts_with('[') {
            in_workspace_package = table_name(line) == Some("workspace.package");
        } else if in_workspace_package && let Some(value) = version_value(line) {
            return Ok(start + value.start..start + value.end);
        }
    }

    Err(Error::Invalid(
        "Expected a version in the [workspace.package] table of Cargo.toml".into(),
    ))
}

/// The name in a `[table]` header line. An `[[array.of.tables]]` has none.
fn table_name(line: &str) -> Option<&str> {
    let inner = line.trim_start().strip_prefix('[')?;
    if inner.starts_with('[') {
        return None;
    }
    Some(inner[..inner.find(']')?].trim())
}

/// Where the string of a `version = "..."` line lies in that line.
fn version_value(line: &str) -> Option<Range<usize>> {
    let value = line
        .trim_start()
        .strip_prefix("version")?
        .trim_start()
        .strip_prefix('=')?
        .trim_start();
    let quote = value
        .chars()
        .next()
        .filter(|quote| matches!(quote, '"' | '\''))?;
    let content = &value[quote.len_utf8()..];
    let start = line.len() - content.len();

    Some(start..start + content.find(quote)?)
}

/// `lockfile` with every workspace crate at version `from` moved to `to`, as
/// Cargo would rewrite it. A workspace crate is a package without a `source`;
/// a crate that sets its own version instead of inheriting it is left alone.
pub fn update_lockfile(lockfile: &str, from: &str, to: &str) -> Result<String> {
    let lines: Vec<&str> = lockfile.split_inclusive('\n').collect();

    let mut packages: Vec<Package<'_>> = Vec::new();
    let mut in_package = false;
    for (index, line) in lines.iter().enumerate() {
        if line.starts_with('[') {
            in_package = line.trim_end() == "[[package]]";
            if in_package {
                packages.push(Package::default());
            }
        } else if in_package && let Some(package) = packages.last_mut() {
            if let Some(name) = string_value(line, "name") {
                package.name = name;
            } else if let Some(version) = string_value(line, "version") {
                package.version = version;
                package.version_line = index;
            } else if string_value(line, "source").is_some() {
                package.has_source = true;
            }
        }
    }

    packages.retain(|package| !package.has_source && package.version == from);
    if packages.is_empty() {
        return Err(Error::Invalid(format!(
            "Expected Cargo.lock to have the workspace crates at version {from}"
        )));
    }

    let mut updated = String::with_capacity(lockfile.len() + packages.len() * to.len());
    for (index, line) in lines.iter().enumerate() {
        let is_version = packages.iter().any(|package| package.version_line == index);
        // A dependency is listed as `"name version"` only when the lock file
        // holds several packages of that name.
        let names_workspace_crate = dependency(line).is_some_and(|(name, version)| {
            version == from && packages.iter().any(|package| package.name == name)
        });

        match line.rfind(from) {
            Some(at) if is_version || names_workspace_crate => {
                updated.push_str(&line[..at]);
                updated.push_str(to);
                updated.push_str(&line[at + from.len()..]);
            }
            _ => updated.push_str(line),
        }
    }

    Ok(updated)
}

/// A `[[package]]` of a lock file.
#[derive(Default)]
struct Package<'a> {
    name: &'a str,
    version: &'a str,
    /// Index of the line the version is on.
    version_line: usize,
    has_source: bool,
}

/// The value of a `key = "value"` line of a lock file.
fn string_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    line.trim_end()
        .strip_prefix(key)?
        .strip_prefix(" = \"")?
        .strip_suffix('"')
}

/// The name and version of a ` "name version",` entry of a dependency list.
fn dependency(line: &str) -> Option<(&str, &str)> {
    line.trim()
        .strip_prefix('"')?
        .strip_suffix("\",")?
        .split_once(' ')
}
