//! The release preparation: version bumps, the changelog entry, and the files
//! a stable release rewrites.

mod common;

use common::{TemporaryDirectory, read_repository_file};
use xtask::{
    ChangelogRelease, Error, PreparedRelease, ReleaseOptions, bump_version, nightly_version,
    prepare_release, set_workspace_version, update_changelog, update_lockfile, workspace_version,
};

const RELEASE: ChangelogRelease<'static> = ChangelogRelease {
    date: "2026-08-08",
    release_url: "https://github.com/acme/project/releases/tag/v1.2.4",
    version: "1.2.4",
};

const RELEASE_ENTRY: &str =
    "- Released version [1.2.4](https://github.com/acme/project/releases/tag/v1.2.4).";

fn message<T: std::fmt::Debug>(result: Result<T, Error>) -> String {
    result.unwrap_err().to_string()
}

#[test]
fn bumps_stable_semantic_versions() {
    assert_eq!(bump_version("1.2.3", "patch").unwrap(), "1.2.4");
    assert_eq!(bump_version("1.2.3", "minor").unwrap(), "1.3.0");
    assert_eq!(bump_version("1.2.3", "major").unwrap(), "2.0.0");
    assert_eq!(bump_version("0.9.99", "patch").unwrap(), "0.9.100");
}

#[test]
fn rejects_invalid_versions_and_bump_types() {
    assert_eq!(
        message(bump_version("1.2.3-beta.1", "patch")),
        "Expected a stable semantic version, received \"1.2.3-beta.1\""
    );
    assert_eq!(
        message(bump_version("1.2.3", "banana")),
        "Expected bump to be major, minor, or patch, received \"banana\""
    );

    for version in [
        "", "1.2", "1.2.3.4", "01.2.3", "1.02.3", "1.2.03", "v1.2.3", "1.2.3\n", " 1.2.3", "1..3",
        "1.2.x", "1.2.+3", "1.2.٣",
    ] {
        assert!(
            message(bump_version(version, "patch")).contains("stable semantic version"),
            "{version:?} is not a stable version"
        );
    }
    // The version is checked before the bump type.
    assert!(message(bump_version("1.2", "banana")).contains("stable semantic version"));
}

#[test]
fn creates_valid_nightly_versions_even_for_numeric_hashes_with_a_leading_zero() {
    assert_eq!(
        nightly_version("1.2.3", "20260808", "0123456").unwrap(),
        "1.2.4-nightly.20260808.g0123456"
    );
    assert_eq!(
        message(nightly_version("1.2.3", "2026-08-08", "abcdef0")),
        "Expected nightly date in YYYYMMDD format, received \"2026-08-08\""
    );
    assert_eq!(
        message(nightly_version("1.2.3", "20260808", "ABCDEF0")),
        "Expected a 7-40 character lowercase Git SHA, received \"ABCDEF0\""
    );

    let full_sha = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(
        nightly_version("1.2.3", "20260808", full_sha).unwrap(),
        format!("1.2.4-nightly.20260808.g{full_sha}")
    );
    for sha in ["abcdef", &format!("{full_sha}0"), "abcdefg", "abcdef0\n"] {
        assert!(message(nightly_version("1.2.3", "20260808", sha)).contains("lowercase Git SHA"));
    }
    assert!(message(nightly_version("1.2.3-rc.1", "20260808", "abcdef0")).contains("stable"));
}

#[test]
fn adds_a_release_to_an_existing_changed_section() {
    let changelog = "# Changelog

Intro.

## 2026-08-08

### Changed

- Existing change.
";

    let updated = update_changelog(changelog, &RELEASE);

    assert_eq!(
        updated,
        format!(
            "# Changelog

Intro.

## 2026-08-08

### Changed

{RELEASE_ENTRY}

- Existing change.
"
        )
    );
}

#[test]
fn adds_a_changed_section_to_an_existing_date() {
    let changelog = "# Changelog

## 2026-08-08

### Fixed

- Existing fix.
";

    let updated = update_changelog(changelog, &RELEASE);

    assert_eq!(
        updated,
        format!(
            "# Changelog

## 2026-08-08

### Changed

{RELEASE_ENTRY}

### Fixed

- Existing fix.
"
        )
    );
}

#[test]
fn creates_a_new_newest_date_section_and_remains_idempotent() {
    let changelog = "# Changelog

Intro.

## 2026-08-07

### Added

- Existing feature.
";

    let updated = update_changelog(changelog, &RELEASE);

    assert!(updated.find("## 2026-08-08").unwrap() < updated.find("## 2026-08-07").unwrap());
    assert!(!updated.ends_with("\n\n"));
    assert_eq!(update_changelog(&updated, &RELEASE), updated);
    assert_eq!(
        updated,
        format!(
            "# Changelog

Intro.

## 2026-08-08

### Changed

{RELEASE_ENTRY}

## 2026-08-07

### Added

- Existing feature.
"
        )
    );
}

#[test]
fn only_looks_for_a_changed_section_under_the_release_date() {
    // The older date has a Changed section; the release date does not.
    let changelog = "# Changelog

## 2026-08-08

### Added

- New feature.

## 2026-08-07

### Changed

- Older change.
";

    let updated = update_changelog(changelog, &RELEASE);

    assert!(updated.contains(&format!(
        "## 2026-08-08\n\n### Changed\n\n{RELEASE_ENTRY}\n\n### Added"
    )));
    assert!(updated.contains("## 2026-08-07\n\n### Changed\n\n- Older change.\n"));
}

#[test]
fn starts_the_changelog_of_a_file_without_dated_sections() {
    assert_eq!(
        update_changelog("# Changelog\n", &RELEASE),
        format!("# Changelog\n\n## 2026-08-08\n\n### Changed\n\n{RELEASE_ENTRY}\n")
    );
    assert_eq!(
        update_changelog("", &RELEASE),
        format!("## 2026-08-08\n\n### Changed\n\n{RELEASE_ENTRY}\n")
    );
    // A heading with anything after the date is not a dated section.
    assert_eq!(
        update_changelog(
            "# Changelog\n\n## 2026-08-07 (draft)\n\n- Note.\n\n\n",
            &RELEASE
        ),
        format!(
            "# Changelog\n\n## 2026-08-07 (draft)\n\n- Note.\n\n## 2026-08-08\n\n### Changed\n\n{RELEASE_ENTRY}\n"
        )
    );
}

const MANIFEST: &str = r#"[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.package]
# The version of every crate, bumped by `cargo xtask prepare-release`.
version = "1.2.3" # stable
edition = "2024"

[workspace.dependencies]
app-core = { path = "crates/core" }
serde = { version = "1.2.3", features = ["derive"] }

[profile.release]
strip = true
"#;

const LOCKFILE: &str = r#"# This file is automatically @generated by Cargo.
# It is not intended for manual editing.
version = 4

[[package]]
name = "app"
version = "1.2.3"
dependencies = [
 "app-core",
 "pinned 1.2.3",
 "serde",
]

[[package]]
name = "app-core"
version = "1.2.3"

[[package]]
name = "own-version"
version = "0.1.0"

[[package]]
name = "pinned"
version = "1.2.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0000000000000000000000000000000000000000000000000000000000000000"

[[package]]
name = "pinned"
version = "2.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1111111111111111111111111111111111111111111111111111111111111111"

[[package]]
name = "serde"
version = "1.2.3"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "2222222222222222222222222222222222222222222222222222222222222222"
"#;

/// The lines of `after` that differ from the line at the same place in `before`.
fn changed_lines<'a>(before: &str, after: &'a str) -> Vec<&'a str> {
    assert_eq!(before.lines().count(), after.lines().count());
    before
        .lines()
        .zip(after.lines())
        .filter(|(before, after)| before != after)
        .map(|(_, after)| after)
        .collect()
}

#[test]
fn reads_and_replaces_only_the_workspace_version_of_the_manifest() {
    assert_eq!(workspace_version(MANIFEST).unwrap(), "1.2.3");

    let updated = set_workspace_version(MANIFEST, "1.3.0").unwrap();
    assert_eq!(workspace_version(&updated).unwrap(), "1.3.0");
    assert_eq!(
        changed_lines(MANIFEST, &updated),
        ["version = \"1.3.0\" # stable"]
    );

    // A literal string, and a table that is not the last one.
    let literal = "[workspace.package]\nedition = '2024'\nversion='0.1.0'\n[workspace]\n";
    assert_eq!(workspace_version(literal).unwrap(), "0.1.0");
    assert_eq!(
        set_workspace_version(literal, "0.1.1").unwrap(),
        "[workspace.package]\nedition = '2024'\nversion='0.1.1'\n[workspace]\n"
    );
}

#[test]
fn refuses_a_manifest_without_a_workspace_version() {
    for manifest in [
        "",
        "[package]\nname = \"app\"\nversion = \"1.2.3\"\n",
        "[workspace.package]\nedition = \"2024\"\n\n[package]\nversion = \"1.2.3\"\n",
        "[workspace.package]\nversion.workspace = true\n",
        "[workspace.package]\n# version = \"1.2.3\"\n",
        "[[workspace.package]]\nversion = \"1.2.3\"\n",
    ] {
        assert_eq!(
            message(workspace_version(manifest)),
            "Expected a version in the [workspace.package] table of Cargo.toml",
            "{manifest:?}"
        );
    }
}

#[test]
fn moves_only_the_workspace_crates_of_the_lockfile() {
    let updated = update_lockfile(LOCKFILE, "1.2.3", "1.3.0").unwrap();

    // `app` and `app-core`. Not the crate with its own version, the registry
    // packages at the same version, or the dependency on one of them.
    assert_eq!(
        changed_lines(LOCKFILE, &updated),
        ["version = \"1.3.0\"", "version = \"1.3.0\""]
    );
    assert!(updated.contains("name = \"app\"\nversion = \"1.3.0\"\n"));
    assert!(updated.contains("name = \"app-core\"\nversion = \"1.3.0\"\n"));
    assert!(updated.contains(" \"pinned 1.2.3\",\n"));
    assert!(updated.starts_with("# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n"));
}

#[test]
fn moves_a_dependency_that_names_the_version_of_a_workspace_crate() {
    // A registry package with the name of a workspace crate makes Cargo spell
    // out the version of both.
    let lockfile = r#"version = 4

[[package]]
name = "app"
version = "1.2.3"
dependencies = [
 "app-core 0.3.0",
 "app-core 1.2.3",
]

[[package]]
name = "app-core"
version = "0.3.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0000000000000000000000000000000000000000000000000000000000000000"

[[package]]
name = "app-core"
version = "1.2.3"
"#;

    let updated = update_lockfile(lockfile, "1.2.3", "1.2.4").unwrap();

    assert_eq!(
        changed_lines(lockfile, &updated),
        [
            "version = \"1.2.4\"",
            " \"app-core 1.2.4\",",
            "version = \"1.2.4\""
        ]
    );
}

#[test]
fn refuses_a_lockfile_that_does_not_have_the_workspace_at_its_version() {
    assert_eq!(
        message(update_lockfile(LOCKFILE, "1.2.2", "1.2.3")),
        "Expected Cargo.lock to have the workspace crates at version 1.2.2"
    );
    assert!(update_lockfile("version = 4\n", "1.2.3", "1.2.4").is_err());
}

fn workspace() -> TemporaryDirectory {
    let directory = TemporaryDirectory::new("release");
    directory.write("Cargo.toml", MANIFEST);
    directory.write("Cargo.lock", LOCKFILE);
    directory.write("CHANGELOG.md", "# Changelog\n");
    directory
}

#[test]
fn updates_the_workspace_version_the_lockfile_and_the_changelog_together() {
    let directory = workspace();

    let result = prepare_release(&ReleaseOptions {
        bump: "minor",
        date: "2026-08-08",
        repository: "acme/project",
        root_directory: directory.path(),
    })
    .unwrap();

    assert_eq!(
        result,
        PreparedRelease {
            release_url: "https://github.com/acme/project/releases/tag/v1.3.0".into(),
            tag: "v1.3.0".into(),
            version: "1.3.0".into(),
        }
    );
    assert_eq!(
        result.to_json(),
        r#"{"releaseUrl":"https://github.com/acme/project/releases/tag/v1.3.0","tag":"v1.3.0","version":"1.3.0"}"#
    );
    assert_eq!(
        workspace_version(&directory.read("Cargo.toml")).unwrap(),
        "1.3.0"
    );
    assert_eq!(
        directory.read("Cargo.lock"),
        update_lockfile(LOCKFILE, "1.2.3", "1.3.0").unwrap()
    );
    assert_eq!(
        directory.read("CHANGELOG.md"),
        "# Changelog\n\n## 2026-08-08\n\n### Changed\n\n- Released version [1.3.0](https://github.com/acme/project/releases/tag/v1.3.0).\n"
    );
}

#[test]
fn refuses_a_release_it_cannot_prepare_and_writes_nothing() {
    let directory = workspace();
    let options = ReleaseOptions {
        bump: "patch",
        date: "2026-08-08",
        repository: "acme/project",
        root_directory: directory.path(),
    };
    let refused = |options: ReleaseOptions<'_>| message(prepare_release(&options));

    assert_eq!(
        refused(ReleaseOptions {
            date: "08/08/2026",
            ..options
        }),
        "Expected date in YYYY-MM-DD format, received \"08/08/2026\""
    );
    for repository in [
        "project",
        "acme/project/extra",
        "acme/",
        "/project",
        "acme/pro ject",
    ] {
        assert_eq!(
            refused(ReleaseOptions {
                repository,
                ..options
            }),
            format!("Expected repository in owner/name format, received \"{repository}\"")
        );
    }
    assert!(
        refused(ReleaseOptions {
            bump: "banana",
            ..options
        })
        .contains("major, minor, or patch")
    );

    // The lock file is behind the manifest: releasing would leave it stale.
    directory.write("Cargo.lock", &LOCKFILE.replace("1.2.3", "1.2.2"));
    assert!(refused(options).contains("Expected Cargo.lock to have the workspace crates"));

    assert_eq!(directory.read("Cargo.toml"), MANIFEST);
    assert_eq!(directory.read("CHANGELOG.md"), "# Changelog\n");

    // A missing file names itself.
    std::fs::remove_file(directory.path().join("CHANGELOG.md")).unwrap();
    directory.write("Cargo.lock", LOCKFILE);
    assert!(refused(options).contains("CHANGELOG.md"));
    assert_eq!(directory.read("Cargo.toml"), MANIFEST);
    assert_eq!(directory.read("Cargo.lock"), LOCKFILE);
}

#[test]
fn prepares_a_release_of_this_workspace() {
    let directory = TemporaryDirectory::new("workspace-release");
    let manifest = read_repository_file("Cargo.toml");
    let lockfile = read_repository_file("Cargo.lock");
    directory.write("Cargo.toml", &manifest);
    directory.write("Cargo.lock", &lockfile);
    directory.write("CHANGELOG.md", &read_repository_file("CHANGELOG.md"));
    let current = workspace_version(&manifest).unwrap();

    let release = prepare_release(&ReleaseOptions {
        bump: "patch",
        date: "2026-08-08",
        repository: "0xtlt/MyMCPs",
        root_directory: directory.path(),
    })
    .unwrap();

    assert_eq!(release.version, bump_version(current, "patch").unwrap());
    let version_line = format!("version = \"{}\"", release.version);

    // One line of the manifest.
    assert_eq!(
        changed_lines(&manifest, &directory.read("Cargo.toml")),
        [version_line.as_str()]
    );

    // In the lock file, the version of each workspace crate and nothing else.
    let workspace_crates = lockfile
        .split("[[package]]\n")
        .filter(|package| {
            package.contains(&format!("\nversion = \"{current}\"\n"))
                && !package.contains("\nsource = ")
        })
        .count();
    let updated = directory.read("Cargo.lock");
    let changed = changed_lines(&lockfile, &updated);
    assert!(workspace_crates >= 2);
    assert_eq!(changed.len(), workspace_crates);
    assert!(changed.iter().all(|line| *line == version_line));
    for name in ["mymcps", "xtask"] {
        assert!(updated.contains(&format!("name = \"{name}\"\n{version_line}\n")));
    }

    assert!(directory.read("CHANGELOG.md").contains(&format!(
        "- Released version [{0}](https://github.com/0xtlt/MyMCPs/releases/tag/v{0}).",
        release.version
    )));
}

fn run(directory: &TemporaryDirectory, arguments: &[&str]) -> Result<String, Error> {
    let arguments: Vec<String> = arguments
        .iter()
        .map(|argument| argument.to_string())
        .collect();
    xtask::cli::run(&arguments, directory.path())
}

#[test]
fn the_command_line_prints_versions_and_prepares_a_release() {
    let directory = workspace();

    assert_eq!(run(&directory, &["version"]).unwrap(), "1.2.3");
    assert_eq!(
        run(
            &directory,
            &["nightly-version", "--date", "20260808", "--sha", "0123456"]
        )
        .unwrap(),
        "1.2.4-nightly.20260808.g0123456"
    );
    // A nightly does not change the version of the workspace.
    assert_eq!(directory.read("Cargo.toml"), MANIFEST);

    assert_eq!(
        run(
            &directory,
            &[
                "prepare-release",
                "--repository",
                "acme/project",
                "--bump",
                "major",
                "--date",
                "2026-08-08"
            ]
        )
        .unwrap(),
        r#"{"releaseUrl":"https://github.com/acme/project/releases/tag/v2.0.0","tag":"v2.0.0","version":"2.0.0"}"#
    );
    assert_eq!(run(&directory, &["version"]).unwrap(), "2.0.0");
}

#[test]
fn the_command_line_refuses_incomplete_commands() {
    let directory = workspace();
    let usage = |arguments: &[&str]| match run(&directory, arguments) {
        Err(Error::Usage(usage)) => usage,
        other => panic!("expected a usage error for {arguments:?}, got {other:?}"),
    };

    assert!(usage(&[]).starts_with("Usage: cargo xtask <command>"));
    assert!(usage(&["release"]).starts_with("Usage: cargo xtask <command>"));
    assert!(usage(&["version", "--verbose"]).starts_with("Usage: cargo xtask <command>"));

    let prepare_release =
        "Usage: cargo xtask prepare-release --bump <type> --date <date> --repository <owner/name>";
    for arguments in [
        &["prepare-release"][..],
        &["prepare-release", "--bump", "patch", "--date", "2026-08-08"],
        &[
            "prepare-release",
            "--bump",
            "patch",
            "--date",
            "2026-08-08",
            "--repository",
        ],
        &[
            "prepare-release",
            "--bump",
            "patch",
            "--date",
            "2026-08-08",
            "--repository",
            "",
        ],
        &[
            "prepare-release",
            "bump",
            "patch",
            "--date",
            "2026-08-08",
            "--repository",
            "a/b",
        ],
    ] {
        assert_eq!(usage(arguments), prepare_release);
    }
    assert_eq!(
        usage(&["nightly-version", "--date", "20260808"]),
        "Usage: cargo xtask nightly-version --date <YYYYMMDD> --sha <sha>"
    );

    assert_eq!(directory.read("Cargo.toml"), MANIFEST);
    assert_eq!(directory.read("Cargo.lock"), LOCKFILE);
}
