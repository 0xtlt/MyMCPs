//! What the runner takes from the machine it runs on: the Deno binary and the
//! server's own environment. The Node app read both from globals
//! (`process.env`, and a `denoRuntime` object its tests reassigned); here they
//! are a value handed to [`crate::DenoRunner::with_runtime`], so that each
//! test builds its own.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use mymcps_core::Config;

use crate::error::DenoError;
use crate::paths::{is_executable_file, normalize, resolve};
use crate::text::js_trim;

const DENO_BINARY_NAME: &str = if cfg!(windows) { "deno.exe" } else { "deno" };
const PATH_DELIMITER: char = if cfg!(windows) { ';' } else { ':' };

/// The part of the server's own process the runner depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEnvironment {
    /// `PATH`: searched for `deno`, and handed to every npm MCP.
    pub path: Option<String>,
    /// `DENO_DIR`.
    pub deno_dir: Option<String>,
    /// `XDG_CACHE_HOME`.
    pub xdg_cache_home: Option<String>,
    /// `LOCALAPPDATA`, where Deno caches on Windows.
    pub local_app_data: Option<String>,
    /// The home directory of the server's user.
    pub home_dir: PathBuf,
    /// Where Deno is looked for when it is neither configured nor on the
    /// `PATH`: [`usual_deno_locations`], unless a test wants none.
    pub known_deno_locations: Vec<PathBuf>,
    /// The working directory of the server. Relative paths are resolved
    /// against it, and it is the application directory: `.env` and, unless
    /// `DATA_DIR` says otherwise, the database are in it.
    pub current_dir: PathBuf,
}

impl HostEnvironment {
    /// Read from the environment and the working directory of this process.
    pub fn from_process() -> Self {
        let variable =
            |name: &str| std::env::var_os(name).map(|value| value.to_string_lossy().into_owned());
        let current_dir = std::env::current_dir().unwrap_or_default();
        let home_dir = std::env::home_dir().unwrap_or_else(|| current_dir.clone());
        Self {
            path: variable("PATH"),
            deno_dir: variable("DENO_DIR"),
            xdg_cache_home: variable("XDG_CACHE_HOME"),
            local_app_data: variable("LOCALAPPDATA"),
            known_deno_locations: usual_deno_locations(&home_dir),
            home_dir,
            current_dir,
        }
    }

    /// The one Deno cache (`$DENO_DIR`, packages under `npm/...`) of every npm MCP.
    /// It is handed to each MCP process and to cache reloads explicitly, so what
    /// runs, what Update MCP refreshes and the cached version shown in the UI are
    /// the same files. Packages may read it but not write it.
    /// Docker sets `DENO_DIR=/app/tmp/deno-cache`; other installs use the directory
    /// Deno itself would pick for the server's user.
    pub fn resolve_deno_dir(&self) -> PathBuf {
        let set = |value: &Option<String>| {
            value
                .as_deref()
                .map(js_trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        };
        if let Some(configured) = set(&self.deno_dir) {
            return resolve(&self.current_dir, configured);
        }
        if cfg!(windows) {
            let base = self
                .local_app_data
                .as_deref()
                .filter(|value| !value.is_empty())
                .map_or_else(|| self.home_dir.clone(), PathBuf::from);
            return resolve(&self.current_dir, base.join("deno"));
        }
        // Deno honours XDG_CACHE_HOME on macOS as well.
        if let Some(xdg_cache) = set(&self.xdg_cache_home) {
            return resolve(&self.current_dir, xdg_cache).join("deno");
        }
        let cache = if cfg!(target_os = "macos") {
            self.home_dir.join("Library").join("Caches").join("deno")
        } else {
            self.home_dir.join(".cache").join("deno")
        };
        resolve(&self.current_dir, cache)
    }

    /// [`locate_deno_binary`] on this server: its `PATH`, then the usual
    /// install locations.
    pub fn locate_deno_binary(&self, configured: Option<&str>) -> Result<PathBuf, DenoError> {
        locate_from(
            &self.current_dir,
            configured,
            self.path.as_deref(),
            &self.known_deno_locations,
        )
    }
}

/// Usual install locations, tried after the server's `PATH`.
pub fn usual_deno_locations(home_dir: &Path) -> Vec<PathBuf> {
    vec![
        PathBuf::from("/usr/local/bin/deno"),
        PathBuf::from("/opt/homebrew/bin/deno"),
        normalize(&home_dir.join(".deno").join("bin").join(DENO_BINARY_NAME)),
        PathBuf::from("/home/ubuntu/.deno/bin/deno"),
    ]
}

/// Absolute path of the Deno binary: `configured` (DENO_PATH) when it points at
/// one, else the first `deno` on `search_path`, else one of `known_locations`.
///
/// A bare `deno` must never reach spawn. A bare command is looked up in the
/// `PATH` of the environment the child is given, and that environment carries
/// the variables operators set on the MCP.
pub fn locate_deno_binary(
    configured: Option<&str>,
    search_path: Option<&str>,
    known_locations: &[PathBuf],
) -> Result<PathBuf, DenoError> {
    let current_dir = std::env::current_dir().unwrap_or_default();
    locate_from(&current_dir, configured, search_path, known_locations)
}

fn locate_from(
    current_dir: &Path,
    configured: Option<&str>,
    search_path: Option<&str>,
    known_locations: &[PathBuf],
) -> Result<PathBuf, DenoError> {
    let configured = configured
        .filter(|configured| !configured.is_empty())
        .map(|configured| resolve(current_dir, configured));
    let on_path = search_path
        .unwrap_or_default()
        .split(PATH_DELIMITER)
        .map(Path::new)
        // A relative PATH entry would be resolved against the working directory.
        .filter(|directory| directory.is_absolute())
        .map(|directory| normalize(&directory.join(DENO_BINARY_NAME)));

    configured
        .into_iter()
        .chain(on_path)
        .chain(known_locations.iter().cloned())
        // A relative path, were one handed in, would be resolved by the child.
        .find(|candidate| candidate.is_absolute() && is_executable_file(candidate))
        .ok_or(DenoError::BinaryNotFound)
}

type BinaryLookup = dyn Fn() -> Result<PathBuf, DenoError> + Send + Sync;

/// Where a [`crate::DenoRunner`] finds Deno and the server's environment.
///
/// [`DenoRuntime::new`] is the server's own. Tests, which cannot rely on Deno
/// being installed, replace the lookup of the binary (a stand-in executable,
/// or a lookup that fails) and the environment:
///
/// ```
/// use mymcps_deno::{DenoError, DenoRuntime, HostEnvironment};
///
/// let host = HostEnvironment {
///     deno_dir: Some("/var/cache/mymcps-deno".into()),
///     ..HostEnvironment::from_process()
/// };
/// let runtime = DenoRuntime::with_host(None, host)
///     .binary(|| Err(DenoError::Other("Deno is not started in this test".into())));
/// assert!(runtime.resolve_binary().is_err());
/// ```
#[derive(Clone)]
pub struct DenoRuntime {
    deno_path: Option<String>,
    host: HostEnvironment,
    lookup: Option<Arc<BinaryLookup>>,
    located: OnceLock<PathBuf>,
}

impl DenoRuntime {
    /// The runtime of the server: `DENO_PATH` from its configuration, the
    /// rest from its process, then from its `.env` file.
    pub fn new(config: &Config) -> Self {
        let mut host = HostEnvironment::from_process();
        // A `.env` file may place the cache, as it could for the Node app.
        let from_file = |name: &str| config.dotenv.get(name).cloned();
        host.deno_dir = host.deno_dir.or_else(|| from_file("DENO_DIR"));
        host.xdg_cache_home = host.xdg_cache_home.or_else(|| from_file("XDG_CACHE_HOME"));
        Self::with_host(config.deno_path.clone(), host)
    }

    /// `deno_path` is `DENO_PATH`.
    pub fn with_host(deno_path: Option<String>, host: HostEnvironment) -> Self {
        Self {
            deno_path,
            host,
            lookup: None,
            located: OnceLock::new(),
        }
    }

    /// Replace the lookup of the Deno binary. It is called for every start
    /// and every cache reload, before anything is spawned.
    pub fn binary<F>(mut self, lookup: F) -> Self
    where
        F: Fn() -> Result<PathBuf, DenoError> + Send + Sync + 'static,
    {
        self.lookup = Some(Arc::new(lookup));
        self
    }

    /// Use this executable as the Deno binary.
    pub fn binary_path(self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        self.binary(move || Ok(path.clone()))
    }

    pub fn host(&self) -> &HostEnvironment {
        &self.host
    }

    /// Looked up once on the server's own PATH; a failed lookup is retried.
    pub fn resolve_binary(&self) -> Result<PathBuf, DenoError> {
        if let Some(lookup) = &self.lookup {
            return lookup();
        }
        if let Some(located) = self.located.get() {
            return Ok(located.clone());
        }
        let located = self.host.locate_deno_binary(self.deno_path.as_deref())?;
        Ok(self.located.get_or_init(|| located).clone())
    }
}

impl std::fmt::Debug for DenoRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DenoRuntime")
            .field("deno_path", &self.deno_path)
            .field("host", &self.host)
            .field("replaced_lookup", &self.lookup.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn host() -> HostEnvironment {
        HostEnvironment {
            path: None,
            deno_dir: None,
            xdg_cache_home: None,
            local_app_data: None,
            home_dir: PathBuf::from("/home/server"),
            known_deno_locations: Vec::new(),
            current_dir: PathBuf::from("/srv/app"),
        }
    }

    // "Deno cache directory": is DENO_DIR, else where Deno itself would cache
    // for the server user.
    #[test]
    fn is_deno_dir_else_where_deno_itself_would_cache_for_the_server_user() {
        let mut host = host();
        host.deno_dir = Some("relative/deno-cache".into());
        assert_eq!(
            host.resolve_deno_dir(),
            Path::new("/srv/app/relative/deno-cache")
        );
        host.deno_dir = Some("  /var/cache/deno/  ".into());
        assert_eq!(host.resolve_deno_dir(), Path::new("/var/cache/deno"));

        // An empty or blank DENO_DIR is no DENO_DIR.
        host.deno_dir = Some("  ".into());
        host.xdg_cache_home = Some("/var/cache/server".into());
        assert_eq!(host.resolve_deno_dir(), Path::new("/var/cache/server/deno"));

        host.deno_dir = None;
        host.xdg_cache_home = None;
        let expected = if cfg!(target_os = "macos") {
            "/home/server/Library/Caches/deno"
        } else {
            "/home/server/.cache/deno"
        };
        assert_eq!(host.resolve_deno_dir(), Path::new(expected));
    }

    #[test]
    fn reads_the_environment_of_the_process() {
        let host = HostEnvironment::from_process();
        assert_eq!(host.path, std::env::var("PATH").ok());
        assert!(host.current_dir.is_absolute());
        assert!(host.resolve_deno_dir().is_absolute());
        assert!(host.resolve_deno_dir().ends_with("deno"));
        assert_eq!(
            host.known_deno_locations,
            usual_deno_locations(&host.home_dir)
        );
    }

    #[test]
    fn tries_the_usual_install_locations_last() {
        assert_eq!(
            usual_deno_locations(Path::new("/home/server")),
            [
                "/usr/local/bin/deno",
                "/opt/homebrew/bin/deno",
                "/home/server/.deno/bin/deno",
                "/home/ubuntu/.deno/bin/deno",
            ]
            .map(PathBuf::from)
        );
    }

    #[test]
    fn asks_a_replaced_lookup_every_time_and_keeps_its_own_answer() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let starts = Arc::new(AtomicUsize::new(0));
        let counted = starts.clone();
        let runtime = DenoRuntime::with_host(None, host()).binary(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Err(DenoError::Other("Deno is not started in this test".into()))
        });
        for _ in 0..2 {
            assert_eq!(
                runtime.resolve_binary().unwrap_err().to_string(),
                "Deno is not started in this test"
            );
        }
        assert_eq!(starts.load(Ordering::SeqCst), 2);

        let fixed = DenoRuntime::with_host(None, host()).binary_path("/opt/fake/deno");
        assert_eq!(fixed.resolve_binary().unwrap(), Path::new("/opt/fake/deno"));
    }
}
