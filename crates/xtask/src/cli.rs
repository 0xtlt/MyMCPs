//! The command line of `cargo xtask`.

use std::collections::HashMap;
use std::path::Path;

use crate::{Error, ReleaseOptions, Result};
use crate::{nightly_version, prepare_release, read, workspace_version};

const USAGE: &str = "\
Usage: cargo xtask <command>

Commands:
  version                                                     Print the version of the workspace
  nightly-version --date <YYYYMMDD> --sha <sha>               Print the version of a nightly build
  prepare-release --bump <type> --date <date> --repository <owner/name>
                                                              Bump the version in Cargo.toml and Cargo.lock,
                                                              and add the release to CHANGELOG.md";

const NIGHTLY_VERSION_USAGE: &str =
    "Usage: cargo xtask nightly-version --date <YYYYMMDD> --sha <sha>";
const PREPARE_RELEASE_USAGE: &str =
    "Usage: cargo xtask prepare-release --bump <type> --date <date> --repository <owner/name>";

/// Runs the command `arguments` name on the workspace at `root_directory`
/// and returns the line to print.
pub fn run(arguments: &[String], root_directory: &Path) -> Result<String> {
    let Some((command, options)) = arguments.split_first() else {
        return Err(Error::Usage(USAGE.into()));
    };

    match command.as_str() {
        "version" if options.is_empty() => current_version(root_directory),
        "nightly-version" => {
            let options = parse_options(options, NIGHTLY_VERSION_USAGE)?;
            let (Some(date), Some(sha)) = (options.get("date"), options.get("sha")) else {
                return Err(Error::Usage(NIGHTLY_VERSION_USAGE.into()));
            };
            nightly_version(&current_version(root_directory)?, date, sha)
        }
        "prepare-release" => {
            let options = parse_options(options, PREPARE_RELEASE_USAGE)?;
            let (Some(bump), Some(date), Some(repository)) = (
                options.get("bump"),
                options.get("date"),
                options.get("repository"),
            ) else {
                return Err(Error::Usage(PREPARE_RELEASE_USAGE.into()));
            };
            let release = prepare_release(&ReleaseOptions {
                bump,
                date,
                repository,
                root_directory,
            })?;
            Ok(release.to_json())
        }
        _ => Err(Error::Usage(USAGE.into())),
    }
}

fn current_version(root_directory: &Path) -> Result<String> {
    let manifest = read(&root_directory.join("Cargo.toml"))?;
    workspace_version(&manifest).map(str::to_string)
}

/// The `--name value` pairs of a command line.
struct Options<'a>(HashMap<&'a str, &'a str>);

impl<'a> Options<'a> {
    /// The value of `--name`. An empty value counts as a missing option.
    fn get(&self, name: &str) -> Option<&'a str> {
        self.0.get(name).copied().filter(|value| !value.is_empty())
    }
}

fn parse_options<'a>(arguments: &'a [String], usage: &str) -> Result<Options<'a>> {
    let mut values = HashMap::new();

    for pair in arguments.chunks(2) {
        let (Some(name), [_, value]) = (pair[0].strip_prefix("--"), pair) else {
            return Err(Error::Usage(usage.into()));
        };
        values.insert(name, value.as_str());
    }

    Ok(Options(values))
}
