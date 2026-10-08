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
    /// Path to the Deno binary used to sandbox npm MCPs.
    pub deno_path: Option<String>,
    /// What the `.env` file sets that the process environment does not. The
    /// Node app copied the file into its environment, where the Deno runner
    /// read `DENO_DIR` and `XDG_CACHE_HOME`.
    pub dotenv: HashMap<String, String>,
}

impl Config {
    /// Read the process environment, after a `.env` file in the working
    /// directory if there is one. Variables already set win over the file.
    pub fn from_env() -> Result<Self> {
        let mut vars: HashMap<String, String> = std::env::vars().collect();
        let mut dotenv = HashMap::new();
        if let Ok(file) = fs::read_to_string(".env") {
            for (name, value) in parse_dotenv(&file, &vars) {
                if let std::collections::hash_map::Entry::Vacant(unset) = vars.entry(name.clone()) {
                    unset.insert(value.clone());
                    dotenv.insert(name, value);
                }
            }
        }
        let mut config = Self::from_vars(&vars)?;
        config.dotenv = dotenv;
        Ok(config)
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
            deno_path: get("DENO_PATH").map(str::to_string),
            dotenv: HashMap::new(),
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
            deno_path: None,
            dotenv: HashMap::new(),
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

    /// `db.sqlite3` in the data directory, `test.sqlite3` under test.
    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join(if self.is_test() {
            "test.sqlite3"
        } else {
            "db.sqlite3"
        })
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
