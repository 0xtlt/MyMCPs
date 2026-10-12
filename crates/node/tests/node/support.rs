use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mymcps_core::TestCore;
use mymcps_core::models::{Mcp, McpTransport};
use mymcps_core::secrets::{EnvironmentInput, merge_environment};
use mymcps_node::{HostEnvironment, NodeRunner, NodeRuntime};

/// Stands in for the `node` binary, which a test cannot rely on. Given npm's
/// `install` it writes the files of the package where npm would; given
/// anything else it is a minimal MCP stdio server. Both record the process
/// they run in, in files of their working directory.
pub const FAKE_NODE: &str = r#"#!/bin/sh
if [ "$2" = install ]; then record=install; else record=started; fi
for argument in "$@"; do printf '%s\n' "$argument"; done > "$record.argv"
env > "$record.env"
pwd > "$record.cwd"
echo $$ > "$record.pid"
if [ "$2" = install ]; then
  if [ -n "$FAKE_INSTALLS" ]; then echo installed >> "$FAKE_INSTALLS"; fi
  if [ "$FAKE_BEHAVIOUR" = registry-down ]; then
    echo '  npm error could not reach the registry of '"$API_KEY" >&2
    exit 1
  fi
  if [ "$FAKE_BEHAVIOUR" = slow-install ]; then sleep 1; fi
  prefix=
  while [ $# -gt 1 ]; do
    if [ "$1" = --prefix ]; then prefix=$2; fi
    shift
  done
  package=${1%@*}
  mkdir -p "$prefix/node_modules/$package" "$HOME/cache"
  echo 'console.log("the real node would run this")' > "$prefix/node_modules/$package/server.js"
  if [ "$FAKE_BEHAVIOUR" = library ]; then bin=; else bin=',"bin":{"fake-mcp":"./server.js"}'; fi
  printf '{"name":"%s","version":"%s"%s}\n' "$package" "${FAKE_VERSION:-1.0.0}" "$bin" \
    > "$prefix/node_modules/$package/package.json"
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
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"%s","capabilities":{"tools":{}},"serverInfo":{"name":"fake-node","version":"1.0.0"}}}\n' "$id" "$version" ;;
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

/// Stands in for the launcher of the sandbox, `mymcps sandbox:exec <policy>
/// <program> <argument>...`: it records the policy it was given in the
/// working directory and becomes the program, held by nothing.
pub const FAKE_LAUNCHER: &str = r#"#!/bin/sh
if [ "$1" != sandbox:exec ]; then echo "not the launcher command: $1" >&2; exit 2; fi
if [ $# -eq 2 ]; then echo 'a pretended sandbox'; exit 0; fi
printf '%s\n' "$2" > launch.policy
shift 2
exec "$@"
"#;

/// What a shell adds to the environment it was started with.
const SHELL_VARIABLES: [&str; 4] = ["PWD", "OLDPWD", "SHLVL", "_"];

pub fn executable(directory: &Path, name: &str, content: &str) -> PathBuf {
    std::fs::create_dir_all(directory).unwrap();
    let path = directory.join(name);
    // Written by a child process, never by this one: a test on another thread
    // that starts a command while a script is open for writing here would
    // hand that descriptor to its child for a moment, and Linux refuses to
    // run a file anyone has open for writing (ETXTBSY).
    let mut writer = std::process::Command::new("/bin/sh")
        .args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"])
        .arg(&path)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(&mut writer.stdin.take().unwrap(), content.as_bytes()).unwrap();
    assert!(writer.wait().unwrap().success());
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

/// What the fake Node recorded about one of its runs.
#[derive(Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    /// The whole environment the process was started with.
    pub env: BTreeMap<String, String>,
    pub pid: String,
}

impl Snapshot {
    /// `record` is `started` for the package and `install` for npm.
    pub fn read(directory: &Path, record: &str) -> Self {
        let read = |kind: &str| {
            let path = directory.join(format!("{record}.{kind}"));
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
/// application directory, and a fake Node with its npm.
pub struct Sandbox {
    pub core: TestCore,
    pub fake: PathBuf,
    pub npm_cli: PathBuf,
    root: PathBuf,
    _directory: tempfile::TempDir,
}

impl Sandbox {
    pub async fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("mymcps-node-sandbox-")
            .tempdir()
            .unwrap();
        // Without links on the way, as a deployment is laid out: the
        // temporary directory of macOS is reached through a link.
        let root = directory.path().canonicalize().unwrap();
        let fake = executable(&root.join("bin"), "node", FAKE_NODE);
        let npm_cli = root.join("npm").join("bin").join("npm-cli.js");
        std::fs::create_dir_all(npm_cli.parent().unwrap()).unwrap();
        std::fs::write(&npm_cli, "").unwrap();
        std::fs::create_dir_all(root.join("app")).unwrap();
        let core = TestCore::with_config(|config| {
            config.data_dir = config.data_dir.canonicalize().unwrap();
        })
        .await;
        Self {
            core,
            fake,
            npm_cli,
            root,
            _directory: directory,
        }
    }

    /// A directory of the test's own, next to the data directory of the server.
    pub fn path(&self) -> &Path {
        &self.root
    }

    /// The `PATH` of the server in these tests: not the one of the test
    /// process, and enough for the fake Node to find `env` and `sed`.
    pub fn server_path(&self) -> String {
        format!("{}:/usr/bin:/bin", self.path().join("server-bin").display())
    }

    pub fn host(&self) -> HostEnvironment {
        HostEnvironment {
            path: Some(self.server_path()),
            // The Node.js of this machine, if it has one, is never found by accident.
            known_node_locations: Vec::new(),
            current_dir: self.path().join("app"),
        }
    }

    /// The runtime of these tests: the fake Node and its npm, and no sandbox.
    pub fn runtime(&self) -> NodeRuntime {
        NodeRuntime::with_host(None, self.host())
            .binary_path(&self.fake)
            .npm_cli(&self.npm_cli)
            .unsandboxed()
    }

    /// A runner whose Node is the fake one.
    pub fn runner(&self) -> NodeRunner {
        self.runner_with(self.runtime())
    }

    pub fn runner_with(&self, runtime: NodeRuntime) -> NodeRunner {
        NodeRunner::with_runtime(self.core.core.clone(), runtime)
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

    /// The directory of the install in use for `mcp`, as `current.json` names it.
    pub fn install_dir(&self, runner: &NodeRunner, mcp: &Mcp) -> PathBuf {
        let packages_dir = runner.packages_root_for(mcp.id);
        let current: serde_json::Value = serde_json::from_slice(
            &std::fs::read(packages_dir.join("current.json")).expect("a package is installed"),
        )
        .unwrap();
        packages_dir.join(current["install"].as_str().unwrap())
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
