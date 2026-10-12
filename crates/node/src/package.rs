//! The package of an npm MCP on disk: what the MCP asks for, where `npm`
//! installed it, and which of its files Node runs.
//!
//! Each MCP has a directory of its own, `<data dir>/mcp-packages/<mcp id>`:
//!
//! ```text
//! current.json     which install is in use, written by the server
//! <install>/       one `npm install`: node_modules, package.json
//! ```
//!
//! An install is written once, by a sandboxed `npm` that can write nothing
//! else, and never changed afterwards: the package reads it and cannot write
//! it. An update is a new install beside the old one, then a new
//! `current.json`. What the server learns about an install is kept in
//! `current.json`, outside of what `npm` and the package can write.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use mymcps_core::models::Mcp;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::NodeError;
use crate::paths::remove_recursively;
use crate::text::js_trim;

const CURRENT: &str = "current.json";
/// More than any `package.json` weighs. What is read was written by a package.
const MANIFEST_LIMIT_BYTES: u64 = 1024 * 1024;
const NAME_LIMIT_CHARS: usize = 214;
const VERSION_LIMIT_CHARS: usize = 64;

// A name of the registry, with or without a scope. Nothing `npm install`
// would take for a path, a URL, a git repository or an alias.
static PACKAGE_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:@[A-Za-z0-9~][A-Za-z0-9._~-]*/)?[A-Za-z0-9~][A-Za-z0-9._~-]*$")
        .expect("a valid static pattern")
});

// A version, a range of versions or a tag.
static VERSION_REQUEST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9^~<>=*][A-Za-z0-9._+^~<>=*| -]*$").expect("a valid static pattern")
});

/// Whether `name`, trimmed, is one `npm install` takes from the registry.
pub fn is_registry_package_name(name: &str) -> bool {
    name.chars().count() <= NAME_LIMIT_CHARS && PACKAGE_NAME.is_match(name)
}

/// Whether `request`, trimmed and not empty, is a version, a range or a tag.
pub fn is_version_request(request: &str) -> bool {
    request.chars().count() <= VERSION_LIMIT_CHARS && VERSION_REQUEST.is_match(request)
}

/// What an npm MCP asks the registry for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageSpec {
    pub name: String,
    /// `latest` for an MCP that names no version.
    pub requested: String,
}

impl PackageSpec {
    pub fn of(mcp: &Mcp) -> Result<Self, NodeError> {
        let name = js_trim(mcp.npm_package.as_deref().unwrap_or_default());
        if name.is_empty() {
            return Err(NodeError::MissingPackage);
        }
        if !is_registry_package_name(name) {
            return Err(NodeError::InvalidPackage(name.to_owned()));
        }
        let requested = Some(js_trim(mcp.npm_version.as_deref().unwrap_or_default()))
            .filter(|version| !version.is_empty())
            .unwrap_or("latest");
        if !is_version_request(requested) {
            return Err(NodeError::InvalidVersion(requested.to_owned()));
        }
        Ok(Self {
            name: name.to_owned(),
            requested: requested.to_owned(),
        })
    }

    /// What `npm install` is given.
    pub fn argument(&self) -> String {
        format!("{}@{}", self.name, self.requested)
    }

    /// The name without its scope, which is what a package most often calls
    /// its executable.
    fn unscoped_name(&self) -> &str {
        self.name
            .rsplit_once('/')
            .map_or(self.name.as_str(), |(_, name)| name)
    }
}

/// A package where `npm` installed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPackage {
    /// The directory of the install, which holds `node_modules`.
    pub directory: PathBuf,
    /// The version the registry answered the request with.
    pub version: String,
    /// The file of the package Node runs.
    pub entry: PathBuf,
}

/// `current.json`.
#[derive(Debug, Serialize, Deserialize)]
struct Current {
    install: String,
    package: String,
    requested: String,
    version: String,
    /// Relative to the directory of the install.
    entry: String,
}

/// A name the server gave an install, not a path out of the directory.
fn is_install_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
}

fn read_current_record(packages_dir: &Path) -> Option<Current> {
    let content = std::fs::read(packages_dir.join(CURRENT)).ok()?;
    let current: Current = serde_json::from_slice(&content).ok()?;
    is_install_name(&current.install).then_some(current)
}

/// The install in use for `spec`: none when nothing was installed yet, or
/// when the MCP has since been pointed at another package or version.
pub(crate) fn read_current(packages_dir: &Path, spec: &PackageSpec) -> Option<InstalledPackage> {
    let current = read_current_record(packages_dir)?;
    if current.package != spec.name || current.requested != spec.requested {
        return None;
    }
    let directory = packages_dir.join(&current.install);
    let entry = directory.join(&current.entry);
    entry.is_file().then_some(InstalledPackage {
        directory,
        version: current.version,
        entry,
    })
}

/// A path the package wrote, followed to the file it names: that file must
/// lie inside the install.
fn inside_install(path: &Path, install_root: &Path) -> Option<PathBuf> {
    let real = path.canonicalize().ok()?;
    (real.starts_with(install_root) && real.is_file()).then_some(real)
}

/// The executable a `package.json` names for the package: its `bin` when it
/// is one path, else the entry of `bin` called as the package, else the only
/// entry. This is what `npx` starts.
fn executable_of<'a>(manifest: &'a Value, spec: &PackageSpec) -> Option<&'a str> {
    match manifest.get("bin")? {
        Value::String(path) => Some(path.as_str()),
        Value::Object(executables) => executables
            .get(spec.unscoped_name())
            .or_else(|| {
                let mut all = executables.values();
                all.next().filter(|_| all.next().is_none())
            })?
            .as_str(),
        _ => None,
    }
}

/// Read what `npm` installed in `directory` for `spec`: its version and the
/// file to run.
pub(crate) fn inspect(directory: &Path, spec: &PackageSpec) -> Result<InstalledPackage, NodeError> {
    let failure = |detail: &str| NodeError::install(&spec.name, detail);
    let install_root = directory
        .canonicalize()
        .map_err(|error| NodeError::file_system("realpath", directory, error))?;
    let mut package_dir = install_root.join("node_modules");
    package_dir.extend(spec.name.split('/'));

    let manifest_path = inside_install(&package_dir.join("package.json"), &install_root)
        .ok_or_else(|| failure("npm did not install it."))?;
    let weight = std::fs::metadata(&manifest_path).map_or(u64::MAX, |metadata| metadata.len());
    if weight > MANIFEST_LIMIT_BYTES {
        return Err(failure("Its package.json is too large."));
    }
    let manifest: Value = std::fs::read(&manifest_path)
        .ok()
        .and_then(|content| serde_json::from_slice(&content).ok())
        .ok_or_else(|| failure("Its package.json cannot be read."))?;

    let version = manifest
        .get("version")
        .and_then(Value::as_str)
        .map(js_trim)
        .filter(|version| !version.is_empty() && version.chars().count() <= VERSION_LIMIT_CHARS)
        .ok_or_else(|| failure("Its package.json names no version."))?;
    let executable = executable_of(&manifest, spec).ok_or_else(|| {
        failure("It is not a program: its package.json names no executable (\"bin\") to start.")
    })?;
    let entry = inside_install(&package_dir.join(executable), &install_root)
        .ok_or_else(|| failure("The executable its package.json names does not exist."))?;

    Ok(InstalledPackage {
        directory: install_root,
        version: version.to_owned(),
        entry,
    })
}

/// Make `installed`, in the directory `install` of `packages_dir`, the
/// install in use, and delete the ones before the one it replaces. The one it
/// replaces is kept: a request that started before the update still runs it.
pub(crate) async fn make_current(
    packages_dir: &Path,
    install: &str,
    spec: &PackageSpec,
    installed: &InstalledPackage,
) -> Result<(), NodeError> {
    let entry = installed
        .entry
        .strip_prefix(&installed.directory)
        .ok()
        .and_then(Path::to_str)
        .ok_or_else(|| NodeError::install(&spec.name, "The path of its executable is unusable."))?;
    let previous = read_current_record(packages_dir).map(|current| current.install);
    let current = Current {
        install: install.to_owned(),
        package: spec.name.clone(),
        requested: spec.requested.clone(),
        version: installed.version.clone(),
        entry: entry.to_owned(),
    };

    // Written whole, then put in place: a reader finds the old file or the new one.
    let path = packages_dir.join(CURRENT);
    let staged = packages_dir.join(format!("{CURRENT}.{install}"));
    let content = serde_json::to_vec(&current).unwrap_or_default();
    tokio::fs::write(&staged, content)
        .await
        .map_err(|error| NodeError::file_system("open", &staged, error))?;
    tokio::fs::rename(&staged, &path)
        .await
        .map_err(|error| NodeError::file_system("rename", &path, error))?;

    let Ok(mut entries) = tokio::fs::read_dir(packages_dir).await else {
        return Ok(());
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let kept = name == CURRENT
            || name == install
            || previous.as_deref().is_some_and(|previous| name == previous);
        if !kept {
            // Left for the next install to try again.
            let _ = remove_recursively(&entry.path()).await;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn mcp(package: Option<&str>, version: Option<&str>) -> Mcp {
        Mcp {
            npm_package: package.map(str::to_owned),
            npm_version: version.map(str::to_owned),
            ..Default::default()
        }
    }

    fn spec(package: &str, version: Option<&str>) -> PackageSpec {
        PackageSpec::of(&mcp(Some(package), version)).unwrap()
    }

    #[test]
    fn asks_for_the_latest_version_of_an_mcp_that_names_none() {
        for version in [None, Some(""), Some("  ")] {
            assert_eq!(
                spec(" @shopify/dev-mcp ", version).argument(),
                "@shopify/dev-mcp@latest"
            );
        }
        assert_eq!(
            spec("mongodb-mcp-server", Some(" 2.0.0 ")).argument(),
            "mongodb-mcp-server@2.0.0"
        );
        for request in [
            "^1.2.3",
            ">=1.2 <2",
            "1.x",
            "next",
            "1.0.0-beta.1+build.5",
            "*",
        ] {
            assert_eq!(spec("a", Some(request)).requested, request);
        }
    }

    #[test]
    fn refuses_what_npm_would_not_take_from_the_registry() {
        for missing in [None, Some(""), Some("   ")] {
            assert!(matches!(
                PackageSpec::of(&mcp(missing, None)),
                Err(NodeError::MissingPackage)
            ));
        }
        for package in [
            "../outside",
            "/etc/passwd",
            "file:../outside",
            "git+https://example.test/x.git",
            "https://example.test/x.tgz",
            "github:user/repo",
            "user/repo",
            "alias@npm:other",
            "@scope/name/extra",
            "@scope",
            "--registry=http://example.test",
            "-g",
            ".hidden",
            "a b",
            "a\\b",
            "name@1.0.0",
        ] {
            let error = PackageSpec::of(&mcp(Some(package), None)).unwrap_err();
            assert!(matches!(error, NodeError::InvalidPackage(_)), "{package}");
            assert_eq!(
                error.to_string(),
                format!("\"{package}\" is not the name of a package of the npm registry")
            );
        }
        assert!(PackageSpec::of(&mcp(Some(&"a".repeat(215)), None)).is_err());

        for version in [
            "npm:other@1",
            "file:../x",
            "1.0.0/../..",
            "-1",
            "--flag",
            "1.0.0@x",
            "https://example.test/x.tgz",
        ] {
            let error = PackageSpec::of(&mcp(Some("a"), Some(version))).unwrap_err();
            assert!(matches!(error, NodeError::InvalidVersion(_)), "{version}");
        }
        assert!(PackageSpec::of(&mcp(Some("a"), Some(&"1".repeat(65)))).is_err());
    }

    /// What `npm install` leaves for a package: its files under `node_modules`.
    fn installed_files(directory: &Path, name: &str, manifest: &Value, files: &[&str]) -> PathBuf {
        let mut package_dir = directory.join("node_modules");
        package_dir.extend(name.split('/'));
        std::fs::create_dir_all(&package_dir).unwrap();
        std::fs::write(package_dir.join("package.json"), manifest.to_string()).unwrap();
        for file in files {
            let path = package_dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        package_dir.canonicalize().unwrap()
    }

    #[test]
    fn runs_the_executable_the_package_names() {
        let directory = tempfile::tempdir().unwrap();
        let entry_of = |name: &str, bin: Value| {
            let install = directory.path().join(name.replace(['@', '/'], "-"));
            let manifest = json!({ "name": name, "version": " 1.2.3 ", "bin": bin });
            let package_dir =
                installed_files(&install, name, &manifest, &["dist/cli.js", "other.js"]);
            inspect(&install, &spec(name, None)).map(|installed| {
                assert_eq!(installed.version, "1.2.3");
                assert_eq!(installed.directory, install.canonicalize().unwrap());
                installed
                    .entry
                    .strip_prefix(&package_dir)
                    .unwrap()
                    .to_owned()
            })
        };

        assert_eq!(
            entry_of("one-path", json!("./dist/cli.js")).unwrap(),
            Path::new("dist/cli.js")
        );
        assert_eq!(
            entry_of("only-entry", json!({ "whatever": "dist/cli.js" })).unwrap(),
            Path::new("dist/cli.js")
        );
        // Of several, the one called as the package, without its scope.
        assert_eq!(
            entry_of(
                "@scope/called",
                json!({ "other": "other.js", "called": "dist/cli.js" })
            )
            .unwrap(),
            Path::new("dist/cli.js")
        );

        for (name, bin, reason) in [
            (
                "ambiguous",
                json!({ "one": "other.js", "two": "dist/cli.js" }),
                "names no executable",
            ),
            ("library", Value::Null, "names no executable"),
            ("not-a-path", json!(7), "names no executable"),
            ("missing-file", json!("dist/missing.js"), "does not exist"),
            ("a-directory", json!("dist"), "does not exist"),
            ("climbing", json!("../../../other.js"), "does not exist"),
        ] {
            let error = entry_of(name, bin).unwrap_err().to_string();
            assert!(
                error.starts_with(&format!("Failed to install npm package \"{name}\". ")),
                "{error}"
            );
            assert!(error.contains(reason), "{error}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn follows_no_link_out_of_the_install() {
        let directory = tempfile::tempdir().unwrap();
        let outside = directory.path().join("outside.js");
        std::fs::write(&outside, "").unwrap();
        let secret = directory.path().join("db.sqlite3");
        std::fs::write(&secret, "{\"version\":\"1.0.0\",\"bin\":\"x.js\"}").unwrap();

        // The executable is a link to a file of the server.
        let install = directory.path().join("linked-executable");
        let package_dir = installed_files(
            &install,
            "linked",
            &json!({ "version": "1.0.0", "bin": "cli.js" }),
            &[],
        );
        std::os::unix::fs::symlink(&outside, package_dir.join("cli.js")).unwrap();
        let error = inspect(&install, &spec("linked", None)).unwrap_err();
        assert!(error.to_string().contains("does not exist"), "{error}");

        // The package.json is one.
        let install = directory.path().join("linked-manifest");
        let package_dir = install.join("node_modules").join("linked");
        std::fs::create_dir_all(&package_dir).unwrap();
        std::os::unix::fs::symlink(&secret, package_dir.join("package.json")).unwrap();
        let error = inspect(&install, &spec("linked", None)).unwrap_err();
        assert!(
            error.to_string().contains("npm did not install it"),
            "{error}"
        );

        // A link that stays inside is the package's own business.
        let install = directory.path().join("linked-inside");
        let package_dir = installed_files(
            &install,
            "linked",
            &json!({ "version": "1.0.0", "bin": "cli.js" }),
            &["dist/real.js"],
        );
        std::os::unix::fs::symlink("dist/real.js", package_dir.join("cli.js")).unwrap();
        let installed = inspect(&install, &spec("linked", None)).unwrap();
        assert_eq!(installed.entry, package_dir.join("dist/real.js"));
    }

    #[test]
    fn refuses_a_package_json_it_cannot_use() {
        let directory = tempfile::tempdir().unwrap();
        let install = directory.path().join("install");
        let error = |manifest: &str| {
            let package_dir = install.join("node_modules").join("broken");
            std::fs::create_dir_all(&package_dir).unwrap();
            std::fs::write(package_dir.join("package.json"), manifest).unwrap();
            inspect(&install, &spec("broken", None))
                .unwrap_err()
                .to_string()
        };

        assert!(error("{not json").contains("cannot be read"));
        assert!(error(r#"{"bin":"x.js"}"#).contains("names no version"));
        assert!(error(r#"{"version":7,"bin":"x.js"}"#).contains("names no version"));
        assert!(error(r#"{"version":"  ","bin":"x.js"}"#).contains("names no version"));
        let padded = format!(
            r#"{{"version":"1.0.0","bin":"x.js","padding":"{}"}}"#,
            "x".repeat(MANIFEST_LIMIT_BYTES as usize)
        );
        assert!(error(&padded).contains("too large"));

        // Nothing was installed at all.
        std::fs::remove_dir_all(install.join("node_modules")).unwrap();
        assert!(
            inspect(&install, &spec("broken", None))
                .unwrap_err()
                .to_string()
                .contains("npm did not install it")
        );
    }

    #[tokio::test]
    async fn keeps_the_install_in_use_and_the_one_before_it() {
        let directory = tempfile::tempdir().unwrap();
        let packages_dir = directory.path();
        let latest = spec("@example/fake-mcp", None);
        let install = |name: &'static str, version: &'static str| {
            let latest = latest.clone();
            async move {
                let manifest = json!({ "version": version, "bin": "server.js" });
                installed_files(
                    &packages_dir.join(name),
                    "@example/fake-mcp",
                    &manifest,
                    &["server.js"],
                );
                let installed = inspect(&packages_dir.join(name), &latest).unwrap();
                make_current(packages_dir, name, &latest, &installed)
                    .await
                    .unwrap();
            }
        };
        let present = || {
            let mut names: Vec<String> = std::fs::read_dir(packages_dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        };

        assert_eq!(read_current(packages_dir, &latest), None);

        install("first", "1.0.0").await;
        let current = read_current(packages_dir, &latest).unwrap();
        assert_eq!(current.version, "1.0.0");
        assert_eq!(
            current.entry,
            packages_dir.join("first/node_modules/@example/fake-mcp/server.js")
        );

        // What a failed install left behind goes with the next one.
        std::fs::create_dir_all(packages_dir.join("abandoned")).unwrap();
        install("second", "1.1.0").await;
        assert_eq!(
            read_current(packages_dir, &latest).unwrap().version,
            "1.1.0"
        );
        assert_eq!(present(), ["current.json", "first", "second"]);

        install("third", "1.2.0").await;
        assert_eq!(present(), ["current.json", "second", "third"]);

        // Another package or another version is not what is installed.
        assert_eq!(
            read_current(packages_dir, &spec("@example/other", None)),
            None
        );
        assert_eq!(
            read_current(packages_dir, &spec("@example/fake-mcp", Some("1.2.0"))),
            None
        );

        // Nor is an install whose files are gone, or a record that points elsewhere.
        std::fs::remove_dir_all(packages_dir.join("third")).unwrap();
        assert_eq!(read_current(packages_dir, &latest), None);
        std::fs::write(
            packages_dir.join(CURRENT),
            json!({
                "install": "../second",
                "package": "@example/fake-mcp",
                "requested": "latest",
                "version": "1.1.0",
                "entry": "node_modules/@example/fake-mcp/server.js"
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(read_current(packages_dir, &latest), None);
    }
}
