//! Configuration read from the environment.
//!
//! The variable names are the ones the AdonisJS app used, so that an existing
//! deployment (a `.env` file, a Compose file, a Coolify resource) starts the
//! Rust server unchanged.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use rand::RngExt;

use crate::client_ip::TrustProxy;
use crate::error::{Error, Result};

/// The shortest key the encryption accepts.
const MIN_APP_KEY_CHARS: usize = 16;

/// APP_KEY values published in this repository. They are long enough to pass
/// validation, but anyone can read them.
const PUBLISHED_APP_KEYS: &[&str] = &[
    // The Docker build stage of the Node app.
    "build-only-not-for-runtime-use",
    // The key the test suites use.
    TEST_APP_KEY,
];

/// The key every test uses. Never valid in production.
pub const TEST_APP_KEY: &str = "base64:btJw8RPglRbr2TIbpi4nR2AMmmpc/gO1Weadd2mSSaI=";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Environment {
    Development,
    Production,
    Test,
}

impl Environment {
    fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "development" => Ok(Self::Development),
            "production" => Ok(Self::Production),
            "test" => Ok(Self::Test),
            other => Err(Error::Config(format!(
                "NODE_ENV must be development, production or test, not \"{other}\""
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub environment: Environment,
    pub host: String,
    pub port: u16,
    pub log_level: String,
    pub trust_proxy: TrustProxy,
    /// The key secrets are encrypted with. Never logged.
    pub app_key: String,
    /// `APP_URL` as configured. Use [`crate::public_url`] to read it.
    pub app_url: Option<String>,
    /// Holds the database, the generated key, the MCP sandboxes and uploads.
    /// `tmp` under the working directory, as in the Node app (`/app/tmp` in
    /// the container image).
    pub data_dir: PathBuf,
    /// `NODE_BINARY`: path to the Node.js binary that runs npm MCPs, when it
    /// is not the `node` of the `PATH`.
    pub node_binary: Option<String>,
    /// `TAILSCALED_BINARY` as configured: a path, or `off`. Use
    /// [`Config::tailscaled`] to read it.
    pub tailscaled_binary: Option<String>,
    /// `TAILNET_CONTROL_URLS`: the control servers a tailnet may use besides
    /// Tailscale's own. Unset, there are none.
    pub tailnet_control: TailnetControl,
    /// `NPM_SANDBOX`: whether npm MCPs run in the sandbox of the kernel. On
    /// unless it is `off`; a server that cannot enforce it then starts none.
    pub npm_sandbox: bool,
}

impl Config {
    /// Read the process environment, after a `.env` file in the working
    /// directory if there is one. Variables already set win over the file.
    pub fn from_env() -> Result<Self> {
        let mut vars: HashMap<String, String> = std::env::vars().collect();
        if let Ok(file) = fs::read_to_string(".env") {
            for (name, value) in parse_dotenv(&file, &vars) {
                vars.entry(name).or_insert(value);
            }
        }
        Self::from_vars(&vars)
    }

    pub fn from_vars(vars: &HashMap<String, String>) -> Result<Self> {
        let get = |name: &str| {
            vars.get(name)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
        };

        // A compiled server is most often a deployment: default to the strict mode.
        let environment = match get("APP_ENV").or(get("NODE_ENV")) {
            Some(value) => Environment::parse(value)?,
            None => Environment::Production,
        };
        let port = match get("PORT") {
            Some(value) => value.parse().map_err(|_| {
                Error::Config(format!("PORT must be a port number, not \"{value}\""))
            })?,
            None => 3333,
        };
        let data_dir = PathBuf::from(get("DATA_DIR").unwrap_or("tmp"));
        let trust_proxy = TrustProxy::parse(get("TRUST_PROXY")).map_err(Error::Config)?;
        let npm_sandbox = match get("NPM_SANDBOX").map(str::to_ascii_lowercase).as_deref() {
            None | Some("on") => true,
            Some("off") => false,
            Some(other) => {
                return Err(Error::Config(format!(
                    "NPM_SANDBOX must be on or off, not \"{other}\""
                )));
            }
        };

        let app_key = match get("APP_KEY") {
            Some(value) => value.to_string(),
            None => load_or_create_app_key(&data_dir)?,
        };
        if app_key.chars().count() < MIN_APP_KEY_CHARS {
            return Err(Error::Config(
                "The value of your key should be at least 16 characters long".into(),
            ));
        }
        if environment == Environment::Production {
            assert_private_app_key(&app_key)?;
        }

        Ok(Self {
            environment,
            host: get("HOST").unwrap_or("0.0.0.0").to_string(),
            port,
            log_level: get("LOG_LEVEL").unwrap_or("info").to_string(),
            trust_proxy,
            app_key,
            app_url: get("APP_URL").map(str::to_string),
            data_dir,
            node_binary: get("NODE_BINARY").map(str::to_string),
            tailscaled_binary: get("TAILSCALED_BINARY").map(str::to_string),
            tailnet_control: TailnetControl::parse(get("TAILNET_CONTROL_URLS"))?,
            npm_sandbox,
        })
    }

    /// A configuration for tests: the test key, a data directory of the
    /// caller's, and the loopback APP_URL the Node test suites used.
    pub fn for_tests(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            environment: Environment::Test,
            host: "localhost".into(),
            port: 3333,
            log_level: "info".into(),
            trust_proxy: TrustProxy::default(),
            app_key: TEST_APP_KEY.into(),
            app_url: Some("http://localhost:3333".into()),
            data_dir: data_dir.into(),
            node_binary: None,
            // Whatever the machine has installed, tests join no tailnet.
            tailscaled_binary: Some("off".into()),
            tailnet_control: TailnetControl::default(),
            npm_sandbox: true,
        }
    }

    pub fn is_production(&self) -> bool {
        self.environment == Environment::Production
    }

    pub fn is_development(&self) -> bool {
        self.environment == Environment::Development
    }

    pub fn is_test(&self) -> bool {
        self.environment == Environment::Test
    }

    /// The `tailscaled` binary that joins tailnets, or `None` when the
    /// feature is off: see [`locate_tailscaled`].
    pub fn tailscaled(&self) -> Option<PathBuf> {
        let search_path = std::env::var_os("PATH");
        locate_tailscaled(self.tailscaled_binary.as_deref(), search_path.as_deref())
    }

    /// `db.sqlite3` in the data directory, `test.sqlite3` under test.
    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join(if self.is_test() {
            "test.sqlite3"
        } else {
            "db.sqlite3"
        })
    }
}

/// The control servers a tailnet may use (`TAILNET_CONTROL_URLS`).
///
/// A tailnet's `tailscaled` talks to its control server, and that server
/// steers it: redirects, ping requests and node-to-node calls make the daemon
/// send HTTP requests of its own, which the routing of MyMCPs does not see.
/// A server that anyone may point at their own control server would let a team
/// aim those requests at the network of the instance, so only the operator
/// decides which other servers are allowed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum TailnetControl {
    /// Tailscale's own control server only: the default.
    #[default]
    Official,
    /// Any `http` or `https` control server (`any`).
    Any,
    /// Tailscale's, and the control servers of these origins, such as
    /// `https://headscale.example.com`.
    Origins(Vec<String>),
}

impl TailnetControl {
    /// Reads `TAILNET_CONTROL_URLS`: unset or blank is [`Official`](Self::Official),
    /// `any` is [`Any`](Self::Any), and otherwise a comma-separated list of
    /// `http` or `https` origins (no user name, password, path or query).
    pub fn parse(value: Option<&str>) -> Result<Self> {
        let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(Self::Official);
        };
        let items: Vec<&str> = value
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .collect();
        if items.iter().any(|item| item.eq_ignore_ascii_case("any")) {
            return if items.len() == 1 {
                Ok(Self::Any)
            } else {
                Err(Error::Config(
                    "TAILNET_CONTROL_URLS is either `any` or a list of origins, not both".into(),
                ))
            };
        }
        let mut origins: Vec<String> = Vec::new();
        for item in items {
            let origin = origin_of(item).ok_or_else(|| {
                Error::Config(format!(
                    "TAILNET_CONTROL_URLS must be `any` or comma-separated http(s) origins \
                     such as https://headscale.example.com, not \"{item}\""
                ))
            })?;
            if !origins.contains(&origin) {
                origins.push(origin);
            }
        }
        Ok(Self::Origins(origins))
    }

    /// Whether a tailnet may use this control server. `None`, which is
    /// Tailscale's, is always allowed.
    pub fn allows(&self, control_url: Option<&str>) -> bool {
        let Some(url) = control_url.map(str::trim).filter(|url| !url.is_empty()) else {
            return true;
        };
        match self {
            Self::Official => false,
            Self::Any => url::Url::parse(url)
                .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.has_host()),
            Self::Origins(origins) => {
                origin_of_url(url).is_some_and(|origin| origins.contains(&origin))
            }
        }
    }

    /// Whether a tailnet may name a control server other than Tailscale's:
    /// what the Connect dialog offers a field for.
    pub fn allows_other_servers(&self) -> bool {
        !matches!(self, Self::Official)
    }

    /// The origins to choose from, when the operator named them.
    pub fn origins(&self) -> &[String] {
        match self {
            Self::Origins(origins) => origins,
            _ => &[],
        }
    }

    /// Why a tailnet may not use this control server, or `None` when it may.
    pub fn refusal(&self, control_url: Option<&str>) -> Option<String> {
        if self.allows(control_url) {
            return None;
        }
        let shown = control_url
            .and_then(origin_of_url)
            .unwrap_or_else(|| "this control server".to_owned());
        Some(match self {
            Self::Official => format!(
                "{shown} is not allowed: this server only connects tailnets to the Tailscale control server. An operator allows others with TAILNET_CONTROL_URLS"
            ),
            _ => format!(
                "{shown} is not an allowed control server on this server. An operator allows it with TAILNET_CONTROL_URLS"
            ),
        })
    }
}

/// The origin of `text` when it is a bare `http(s)` origin.
fn origin_of(text: &str) -> Option<String> {
    let url = url::Url::parse(text).ok()?;
    let bare = url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && matches!(url.path(), "" | "/");
    bare.then(|| origin_of_parsed(&url)).flatten()
}

/// The origin of an `http(s)` URL, whatever its path.
fn origin_of_url(text: &str) -> Option<String> {
    origin_of_parsed(&url::Url::parse(text.trim()).ok()?)
}

fn origin_of_parsed(url: &url::Url) -> Option<String> {
    (matches!(url.scheme(), "http" | "https") && url.has_host())
        .then(|| url.origin().ascii_serialization())
}

/// The name of the Tailscale daemon.
const TAILSCALED_NAME: &str = if cfg!(windows) {
    "tailscaled.exe"
} else {
    "tailscaled"
};

/// Absolute path of `tailscaled`, or `None` when tailnets are off.
///
/// `TAILSCALED_BINARY` (`configured`) decides: `off` turns the feature off,
/// a path is the binary, and when it is unset the first `tailscaled` on
/// `search_path` is. A path that is not an executable file turns the
/// feature off, as does no `tailscaled` on the `PATH`.
///
/// The result is always absolute: a bare name must never reach spawn, where
/// the child's own `PATH` would decide what it is. Relative entries of the
/// `PATH` are skipped for the same reason.
pub fn locate_tailscaled(
    configured: Option<&str>,
    search_path: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    let configured = configured.map(str::trim).filter(|value| !value.is_empty());
    if configured.is_some_and(|value| value.eq_ignore_ascii_case("off")) {
        return None;
    }
    if let Some(path) = configured {
        let path = std::path::absolute(path).ok()?;
        return is_executable_file(&path).then_some(path);
    }
    std::env::split_paths(search_path?)
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(TAILSCALED_NAME))
        .find(|candidate| is_executable_file(candidate))
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

pub fn is_published_app_key(app_key: &str) -> bool {
    PUBLISHED_APP_KEYS.contains(&app_key.trim())
}

/// Fails when the key is one of the published values.
pub fn assert_private_app_key(app_key: &str) -> Result<()> {
    if !is_published_app_key(app_key) {
        return Ok(());
    }
    Err(Error::Config(
        "Refusing to start: APP_KEY is a placeholder published in the MyMCPs repository \
         (the Docker build key or the test key), so MCP credentials and sessions would be \
         protected by a key anyone can read. Set APP_KEY to a private value from \
         `mymcps generate:key`, or leave it empty so the server generates one in its data directory."
            .into(),
    ))
}

/// A new key in the form the Node app generated: `base64:` and 32 random bytes.
pub fn generate_app_key() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill(&mut bytes);
    format!("base64:{}", STANDARD.encode(bytes))
}

/// The key the container entrypoint of the Node app kept in `app.key` when
/// `APP_KEY` was left empty. Read it, or create it on the first start.
fn load_or_create_app_key(data_dir: &Path) -> Result<String> {
    let path = data_dir.join("app.key");
    match fs::read_to_string(&path) {
        Ok(existing) if !existing.trim().is_empty() => return Ok(existing.trim().to_string()),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(Error::Config(format!(
                "Cannot read {}: {error}",
                path.display()
            )));
        }
    }

    fs::create_dir_all(data_dir)?;
    let key = generate_app_key();
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path)?;
    writeln!(file, "{key}")?;
    Ok(key)
}

/// The subset of the dotenv format the project's `.env.example` uses:
/// comments, `NAME=value`, optional quotes, and `${NAME}` references.
fn parse_dotenv(content: &str, environment: &HashMap<String, String>) -> Vec<(String, String)> {
    let mut parsed: Vec<(String, String)> = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim().trim_start_matches("export ").trim();
        let value = value.trim();
        let (value, interpolate) = match (value.chars().next(), value.chars().last()) {
            (Some('\''), Some('\'')) if value.len() >= 2 => (&value[1..value.len() - 1], false),
            (Some('"'), Some('"')) if value.len() >= 2 => (&value[1..value.len() - 1], true),
            _ => (value.split(" #").next().unwrap_or(value).trim(), true),
        };

        let mut resolved = String::new();
        let mut rest = value;
        while interpolate && let Some(start) = rest.find("${") {
            let Some(length) = rest[start..].find('}') else {
                break;
            };
            let reference = &rest[start + 2..start + length];
            resolved.push_str(&rest[..start]);
            let known = environment.get(reference).or_else(|| {
                parsed
                    .iter()
                    .rev()
                    .find(|(name, _)| name == reference)
                    .map(|(_, value)| value)
            });
            resolved.push_str(known.map(String::as_str).unwrap_or(""));
            rest = &rest[start + length + 1..];
        }
        resolved.push_str(rest);
        parsed.push((name.to_string(), resolved));
    }
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn refuses_published_keys_in_production_only() {
        for key in PUBLISHED_APP_KEYS {
            assert!(is_published_app_key(key));
            assert!(is_published_app_key(&format!("  {key}\n")));
            let production =
                Config::from_vars(&vars(&[("NODE_ENV", "production"), ("APP_KEY", key)]));
            assert!(
                production
                    .unwrap_err()
                    .to_string()
                    .starts_with("Refusing to start: APP_KEY")
            );
            assert!(Config::from_vars(&vars(&[("NODE_ENV", "test"), ("APP_KEY", key)])).is_ok());
        }
        assert!(!is_published_app_key(
            "base64:C0ffee0nlyF0rThisTestN0tPublishedAnywhere0000="
        ));
    }

    #[test]
    fn generates_and_reuses_a_key_when_none_is_set() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("tmp");
        let env = vars(&[("DATA_DIR", data_dir.to_str().unwrap())]);

        let first = Config::from_vars(&env).unwrap();
        assert!(first.app_key.starts_with("base64:"));
        assert_eq!(
            STANDARD
                .decode(&first.app_key["base64:".len()..])
                .unwrap()
                .len(),
            32
        );
        assert_eq!(
            fs::read_to_string(data_dir.join("app.key")).unwrap(),
            format!("{}\n", first.app_key)
        );

        let second = Config::from_vars(&env).unwrap();
        assert_eq!(first.app_key, second.app_key);
    }

    #[test]
    fn reads_the_example_dotenv_file() {
        let parsed = parse_dotenv(
            "# Node\nPORT=3333\nHOST=localhost\nAPP_URL=http://${HOST}:${PORT}\nAPP_KEY=\nNAME='a b' \n",
            &HashMap::new(),
        );
        assert_eq!(
            parsed,
            vec![
                ("PORT".to_string(), "3333".to_string()),
                ("HOST".to_string(), "localhost".to_string()),
                ("APP_URL".to_string(), "http://localhost:3333".to_string()),
                ("APP_KEY".to_string(), String::new()),
                ("NAME".to_string(), "a b".to_string()),
            ]
        );
    }

    #[test]
    fn sandboxes_npm_mcps_unless_told_not_to() {
        let config = |sandbox: Option<&str>| {
            let mut pairs = vec![("NODE_ENV", "test"), ("APP_KEY", TEST_APP_KEY)];
            pairs.extend(sandbox.map(|value| ("NPM_SANDBOX", value)));
            Config::from_vars(&vars(&pairs))
        };
        for on in [None, Some(""), Some("on"), Some(" ON ")] {
            assert!(config(on).unwrap().npm_sandbox, "{on:?}");
        }
        for off in ["off", "OFF"] {
            assert!(!config(Some(off)).unwrap().npm_sandbox);
        }
        // A value that could be read either way is not guessed at.
        for unclear in ["false", "0", "no", "disabled"] {
            assert_eq!(
                config(Some(unclear)).unwrap_err().to_string(),
                format!("NPM_SANDBOX must be on or off, not \"{unclear}\"")
            );
        }
        assert!(Config::for_tests("tmp").npm_sandbox);
    }

    #[cfg(unix)]
    #[test]
    fn finds_tailscaled_unless_it_is_off() {
        use std::ffi::OsString;
        use std::os::unix::fs::PermissionsExt;

        let write = |directory: &Path, name: &str, mode: u32| {
            let path = directory.join(name);
            fs::write(&path, b"#!/bin/sh\n").unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            path
        };
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_binary = write(first.path(), "tailscaled", 0o755);
        let second_binary = write(second.path(), "tailscaled", 0o755);
        let search = |directories: &[&Path]| -> OsString {
            std::env::join_paths(directories.iter().map(|directory| directory.as_os_str())).unwrap()
        };
        let both = search(&[first.path(), second.path()]);

        // Unset: the first one of the PATH.
        assert_eq!(
            locate_tailscaled(None, Some(&both)),
            Some(first_binary.clone())
        );
        assert_eq!(
            locate_tailscaled(Some("  "), Some(&both)),
            Some(first_binary)
        );
        // None on the PATH, or no PATH: off.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            locate_tailscaled(None, Some(&search(&[empty.path()]))),
            None
        );
        assert_eq!(locate_tailscaled(None, None), None);
        // A file that is not executable is not found, and the search goes on.
        let locked = tempfile::tempdir().unwrap();
        write(locked.path(), "tailscaled", 0o644);
        assert_eq!(
            locate_tailscaled(None, Some(&search(&[locked.path(), second.path()]))),
            Some(second_binary.clone())
        );
        // A relative entry of the PATH is never searched.
        assert_eq!(
            locate_tailscaled(None, Some(std::ffi::OsStr::new("relative/bin"))),
            None
        );

        // A path is the binary, wherever the PATH points.
        assert_eq!(
            locate_tailscaled(second_binary.to_str(), Some(&both)),
            Some(second_binary.clone())
        );
        let missing = first.path().join("missing");
        assert_eq!(locate_tailscaled(missing.to_str(), Some(&both)), None);
        let locked_binary = locked.path().join("tailscaled");
        assert_eq!(locate_tailscaled(locked_binary.to_str(), Some(&both)), None);

        // `off` is off, whatever else is there.
        for off in ["off", "OFF", " Off "] {
            assert_eq!(locate_tailscaled(Some(off), Some(&both)), None, "{off}");
        }
    }

    #[test]
    fn reads_the_control_servers_a_tailnet_may_use() {
        let parse = |value: Option<&str>| TailnetControl::parse(value);
        assert_eq!(parse(None).unwrap(), TailnetControl::Official);
        assert_eq!(parse(Some("  ")).unwrap(), TailnetControl::Official);
        for any in ["any", " ANY "] {
            assert_eq!(parse(Some(any)).unwrap(), TailnetControl::Any);
        }
        let listed = parse(Some(
            "https://Headscale.example.com/, http://10.0.0.2:8080 ,https://headscale.example.com",
        ))
        .unwrap();
        assert_eq!(
            listed,
            TailnetControl::Origins(vec![
                "https://headscale.example.com".into(),
                "http://10.0.0.2:8080".into()
            ])
        );
        for bad in [
            "any,https://hs.example.com",
            "hs.example.com",
            "ftp://hs.example.com",
            "https://user:pw@hs.example.com",
            "https://hs.example.com/path",
            "https://hs.example.com/?x=1",
            "https://hs.example.com,nope",
        ] {
            assert!(parse(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn decides_which_control_servers_are_allowed() {
        let official = TailnetControl::Official;
        let any = TailnetControl::Any;
        let listed =
            TailnetControl::parse(Some("https://hs.example.com,http://10.0.0.2:8080")).unwrap();
        // Tailscale's own is always allowed.
        for control in [&official, &any, &listed] {
            assert!(control.allows(None));
            assert!(control.allows(Some("")));
            assert!(control.refusal(None).is_none());
        }
        assert!(!official.allows(Some("https://hs.example.com/")));
        assert!(!official.allows_other_servers());
        assert!(any.allows(Some("https://hs.example.com/")));
        assert!(any.allows(Some("http://127.0.0.1:8080/")));
        assert!(!any.allows(Some("ftp://hs.example.com")));
        assert!(any.allows_other_servers());
        assert!(listed.allows(Some("https://HS.example.com/anything")));
        assert!(listed.allows(Some("https://hs.example.com:443/")));
        assert!(listed.allows(Some("http://10.0.0.2:8080/")));
        assert!(!listed.allows(Some("http://10.0.0.2:8081/")));
        assert!(!listed.allows(Some("http://hs.example.com/")));
        assert!(!listed.allows(Some("https://evil.example.com/")));
        assert!(!listed.allows(Some("not a url")));
        assert_eq!(listed.origins().len(), 2);

        let message = official.refusal(Some("https://hs.example.com/")).unwrap();
        assert!(message.contains("https://hs.example.com"), "{message}");
        assert!(message.contains("TAILNET_CONTROL_URLS"), "{message}");
        assert!(
            listed
                .refusal(Some("https://evil.example.com/x?k=secret"))
                .unwrap()
                .contains("TAILNET_CONTROL_URLS")
        );
        assert!(
            !listed
                .refusal(Some("https://evil.example.com/x?k=secret"))
                .unwrap()
                .contains("secret")
        );
    }

    #[test]
    fn reads_the_tailnet_settings_from_the_environment() {
        let config = |name: &str, value: Option<&str>| {
            let mut pairs = vec![("NODE_ENV", "test"), ("APP_KEY", TEST_APP_KEY)];
            pairs.extend(value.map(|value| (name, value)));
            Config::from_vars(&vars(&pairs))
        };
        let binary = |value| config("TAILSCALED_BINARY", value).unwrap();
        assert_eq!(binary(None).tailscaled_binary, None);
        assert_eq!(binary(Some("")).tailscaled_binary, None);
        assert_eq!(
            binary(Some(" /usr/local/bin/tailscaled ")).tailscaled_binary,
            Some("/usr/local/bin/tailscaled".to_string())
        );
        assert_eq!(binary(Some("off")).tailscaled(), None);
        assert_eq!(Config::for_tests("tmp").tailscaled(), None);

        let control = |value| config("TAILNET_CONTROL_URLS", value);
        assert_eq!(
            control(None).unwrap().tailnet_control,
            TailnetControl::Official
        );
        assert_eq!(
            control(Some("any")).unwrap().tailnet_control,
            TailnetControl::Any
        );
        assert!(control(Some("nope")).is_err());
        assert_eq!(
            Config::for_tests("tmp").tailnet_control,
            TailnetControl::Official
        );
    }

    #[test]
    fn rejects_short_keys_and_bad_ports() {
        assert!(Config::from_vars(&vars(&[("APP_KEY", "short")])).is_err());
        assert!(
            Config::from_vars(&vars(&[
                ("NODE_ENV", "test"),
                ("APP_KEY", TEST_APP_KEY),
                ("PORT", "http")
            ]))
            .is_err()
        );
    }
}
