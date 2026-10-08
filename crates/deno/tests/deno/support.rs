use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use mymcps_core::TestCore;
use mymcps_core::models::{Mcp, McpTransport};
use mymcps_core::secrets::{EnvironmentInput, merge_environment};
use mymcps_deno::{DenoRunner, DenoRuntime, HostEnvironment};

/// Stands in for the `deno` binary, which a test cannot rely on. As `deno run`
/// it is a minimal MCP stdio server; as `deno cache` it exits at once. Both
/// record the process they run in, in files of their working directory.
pub const FAKE_DENO: &str = r#"#!/bin/sh
if [ "$1" = cache ]; then record=reload; else record=started; fi
for argument in "$@"; do printf '%s\n' "$argument"; done > "$record.argv"
env > "$record.env"
pwd > "$record.cwd"
echo $$ > "$record.pid"
if [ "$1" = cache ]; then
  if [ "$FAKE_BEHAVIOUR" = registry-down ]; then
    echo '  error: could not reach the registry of '"$API_KEY" >&2
    exit 1
  fi
  exit 0
fi
case "$FAKE_BEHAVIOUR" in
  refuse-key)
    printf 'error: cannot start, the key %s was refused\n' "$API_KEY" >&2
    exit 3 ;;
  flood-then-fail)
    head -c 262144 /dev/zero | tr '\0' x >&2
    exit 3 ;;
  noisy)
    head -c 524288 /dev/zero | tr '\0' e >&2 ;;
esac
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      version=$(printf '%s\n' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"fake-deno","version":"1.0.0"}}}\n' "$id" "$version" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"snapshot","title":"Snapshot","description":"Describes the process it runs in","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}},{"name":"bare","inputSchema":{"type":"object","properties":{"q":{"type":"string"}}}}]}}\n' "$id" ;;
    *'"method":"tools/call"'*)
      printf '%s\n' "$line" > call.json
      case "$line" in
        *'"name":"unknown"'*)
          printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32602,"message":"Unknown tool: unknown"}}\n' "$id" ;;
        *'"name":"failing"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"it failed"}],"isError":true}}\n' "$id" ;;
        *)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"called"}]}}\n' "$id" ;;
      esac ;;
  esac
done
"#;

/// What a shell adds to the environment it was started with.
const SHELL_VARIABLES: [&str; 4] = ["PWD", "OLDPWD", "SHLVL", "_"];

pub fn executable(directory: &Path, name: &str, content: &str) -> PathBuf {
    std::fs::create_dir_all(directory).unwrap();
    let path = directory.join(name);
    std::fs::write(&path, content).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

pub fn environment_inputs(environment: &[(&str, &str)]) -> Vec<EnvironmentInput> {
    environment
        .iter()
        .map(|(name, value)| EnvironmentInput {
            name: (*name).to_owned(),
            value: Some((*value).to_owned()),
        })
        .collect()
}

/// What the fake Deno recorded about one of its runs.
#[derive(Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    /// The whole environment the process was started with.
    pub env: BTreeMap<String, String>,
    pub pid: String,
}

impl Snapshot {
    /// `record` is `started` for `deno run` and `reload` for `deno cache`.
    pub fn read(sandbox: &Path, record: &str) -> Self {
        let read = |kind: &str| {
            let path = sandbox.join(format!("{record}.{kind}"));
            std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        };
        Self {
            argv: read("argv").lines().map(str::to_owned).collect(),
            cwd: PathBuf::from(read("cwd").trim_end()),
            env: read("env")
                .lines()
                .filter_map(|line| line.split_once('='))
                .filter(|(name, _)| !SHELL_VARIABLES.contains(name))
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
            pid: read("pid").trim().to_owned(),
        }
    }

    pub fn variable(&self, name: &str) -> Option<&str> {
        self.env.get(name).map(String::as_str)
    }
}

/// A server of its own for one test: a database and data directory, an
/// application directory, a Deno cache directory and a fake Deno.
pub struct Sandbox {
    pub core: TestCore,
    pub fake: PathBuf,
    root: PathBuf,
    _directory: tempfile::TempDir,
}

impl Sandbox {
    pub async fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("mymcps-deno-sandbox-")
            .tempdir()
            .unwrap();
        // Without links on the way, as a deployment is laid out: Deno checks
        // what a package reads against where the files really are, and the
        // temporary directory of macOS is reached through a link.
        let root = directory.path().canonicalize().unwrap();
        let fake = executable(&root.join("bin"), "deno", FAKE_DENO);
        std::fs::create_dir_all(root.join("app")).unwrap();
        let core = TestCore::with_config(|config| {
            config.data_dir = config.data_dir.canonicalize().unwrap();
        })
        .await;
        Self {
            core,
            fake,
            root,
            _directory: directory,
        }
    }

    /// A directory of the test's own, next to the data directory of the server.
    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn deno_dir(&self) -> PathBuf {
        self.path().join("deno-dir")
    }

    /// The `PATH` of the server in these tests: not the one of the test
    /// process, and enough for the fake Deno to find `env` and `sed`.
    pub fn server_path(&self) -> String {
        format!("{}:/usr/bin:/bin", self.path().join("server-bin").display())
    }

    pub fn host(&self) -> HostEnvironment {
        HostEnvironment {
            path: Some(self.server_path()),
            deno_dir: Some(self.deno_dir().display().to_string()),
            xdg_cache_home: None,
            local_app_data: None,
            home_dir: self.path().join("home"),
            // The Deno of this machine, if it has one, is never found by accident.
            known_deno_locations: Vec::new(),
            current_dir: self.path().join("app"),
        }
    }

    /// A runner whose Deno is the fake one.
    pub fn runner(&self) -> DenoRunner {
        self.runner_with(|_| {})
    }

    pub fn runner_with(&self, adjust: impl FnOnce(&mut HostEnvironment)) -> DenoRunner {
        let mut host = self.host();
        adjust(&mut host);
        DenoRunner::with_runtime(
            self.core.core.clone(),
            DenoRuntime::with_host(None, host).binary_path(&self.fake),
        )
    }

    pub fn npm_mcp(&self, environment: &[(&str, &str)]) -> Mcp {
        Mcp {
            id: 7,
            name: "Fake MCP".to_owned(),
            slug: "fake-mcp".to_owned(),
            transport: McpTransport::Npm,
            npm_package: Some("@example/fake-mcp".to_owned()),
            npm_env: merge_environment(
                &self.core.encryption,
                None,
                &environment_inputs(environment),
            ),
            ..Default::default()
        }
    }
}

/// The variables an MCP client hands every stdio server from its own
/// environment, minus the two the runner sets itself.
pub fn inherited_from_the_server() -> BTreeMap<String, String> {
    ["LOGNAME", "SHELL", "TERM", "USER"]
        .into_iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| (name.to_owned(), value))
        })
        .filter(|(_, value)| !value.starts_with("()"))
        .collect()
}

pub fn display(path: &Path) -> String {
    path.display().to_string()
}

/// Say why a test did less than its name promises. Written to stderr itself,
/// so that the test harness shows it for a passing test as well.
pub fn announce(test: &str, message: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{test}: {message}");
}
