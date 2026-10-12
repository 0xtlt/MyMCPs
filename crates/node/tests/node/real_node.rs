//! The runner with the real Node.js and npm: a package from an npm registry
//! on 127.0.0.1 is installed, started and asked what it sees. It is what
//! shows that the command lines still mean, to npm and to Node, what the
//! runner intends. What the sandbox refuses the same package is tested in
//! `crates/cli`, with the real launcher.
//!
//! Each test skips, with a message, on a machine without Node.js and npm.

use std::collections::BTreeMap;
use std::path::PathBuf;

use mymcps_node::{NodeError, NodeRunner, NodeRuntime};
use serde_json::{Value, json};

use crate::registry::{Registry, SERVER_JS, real_node, registry_variables};
use crate::support::{Sandbox, announce, display, inherited_from_the_server};

/// What macOS adds to the environment of every process.
const SYSTEM_VARIABLES: [&str; 1] = ["__CF_USER_TEXT_ENCODING"];

fn runner_with_real_node(sandbox: &Sandbox, test: &str) -> Option<NodeRunner> {
    let Some(node) = real_node() else {
        announce(
            test,
            "skipped, Node.js with npm is not installed (looked at NODE_BINARY, the PATH and the usual locations)",
        );
        return None;
    };
    Some(
        sandbox.runner_with(
            NodeRuntime::with_host(None, sandbox.host())
                .binary_path(node)
                .unsandboxed(),
        ),
    )
}

fn as_refs<'a>(variables: &'a [(&'static str, String)]) -> Vec<(&'a str, &'a str)> {
    variables
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect()
}

#[tokio::test]
async fn installs_a_package_from_the_registry_and_runs_its_executable() {
    let sandbox = Sandbox::new().await;
    let Some(runner) = runner_with_real_node(
        &sandbox,
        "installs_a_package_from_the_registry_and_runs_its_executable",
    ) else {
        return;
    };
    let registry = Registry::start().await;

    let mut variables = registry_variables(&registry);
    variables.extend([
        ("API_KEY", "kept-secret".to_owned()),
        // Saved before these names were refused.
        ("HOME", "/member/home".to_owned()),
        ("NODE_OPTIONS", "--require=/member/evil.js".to_owned()),
        // What npm would obey, were it not told otherwise on its command line.
        ("NPM_CONFIG_IGNORE_SCRIPTS", "false".to_owned()),
        ("NPM_CONFIG_BIN_LINKS", "true".to_owned()),
        ("NPM_CONFIG_GLOBAL", "true".to_owned()),
        (
            "NPM_CONFIG_PREFIX",
            display(&sandbox.path().join("member-prefix")),
        ),
        (
            "NPM_CONFIG_CACHE",
            display(&sandbox.path().join("member-cache")),
        ),
    ]);
    let mut mcp = sandbox.npm_mcp(&as_refs(&variables));
    mcp.set_npm_args_list(&["--flag".to_owned()]);
    let sandbox_dir = runner.sandbox_root_for(mcp.id);

    // What an earlier run of the package may have left in its sandbox says
    // nothing to npm about where packages come from: nothing listens there.
    std::fs::create_dir_all(&sandbox_dir).unwrap();
    std::fs::write(sandbox_dir.join(".npmrc"), "registry=http://127.0.0.1:9/\n").unwrap();

    // Update MCP: the package is installed in a directory of the MCP's own.
    let installed = runner.install_npm_package(&mcp).await.unwrap();
    // npm asked the registry of the MCP, and nobody else, for the package:
    // its description, as often as it likes, then its files.
    let mut requests = registry.requests();
    let fetched = requests.len();
    requests.dedup();
    assert_eq!(
        requests,
        [
            "GET /@example%2ffake-mcp",
            "GET /@example/fake-mcp/-/fake-mcp-1.0.0.tgz"
        ]
    );
    assert_eq!(installed.version, "1.0.0");
    assert_eq!(
        runner.installed_npm_package_version(&mcp).as_deref(),
        Some("1.0.0")
    );
    let install_dir = sandbox.install_dir(&runner, &mcp);
    let package_dir = install_dir.join("node_modules/@example/fake-mcp");
    assert_eq!(installed.entry, package_dir.join("server.js"));
    assert_eq!(
        std::fs::read_to_string(package_dir.join("server.js")).unwrap(),
        SERVER_JS
    );
    // None of its scripts ran, no executable was linked, and npm kept its
    // cache and everything else where it was told to.
    assert!(!package_dir.join("script-ran").exists());
    assert!(!install_dir.join("node_modules/.bin").exists());
    assert!(!install_dir.join(".npm").exists());
    for elsewhere in ["member-prefix", "member-cache"] {
        assert!(!sandbox.path().join(elsewhere).exists(), "{elsewhere}");
    }

    let tools = runner.list_tools(&mcp).await.unwrap();
    assert_eq!(
        serde_json::to_value(&tools).unwrap(),
        json!([{
            "name": "probe",
            "description": "Tries the limits of the sandbox",
            "inputSchema": { "type": "object" }
        }])
    );
    // Listing started the package that was installed, and fetched nothing.
    assert_eq!(registry.requests().len(), fetched);

    let arguments = json!({
        "read": { "ownState": "state.json" },
        "list": { "ownSandbox": sandbox_dir },
        "write": { "ownState": "state.json", "inTmpdir": sandbox_dir.join("scratch.tmp") },
    });
    let result = runner
        .call_tool(&mcp, "probe", arguments.as_object().unwrap().clone())
        .await
        .unwrap();
    let probe: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();

    // The package runs in its sandbox, which is also its home and its temporary directory.
    assert_eq!(probe["argv"], json!(["--flag"]));
    let canonical = |value: &Value| {
        PathBuf::from(value.as_str().unwrap())
            .canonicalize()
            .unwrap()
    };
    assert_eq!(
        canonical(&probe["cwd"]),
        sandbox_dir.canonicalize().unwrap()
    );
    assert_eq!(probe["homedir"], json!(sandbox_dir));
    assert_eq!(probe["tmpdir"], json!(sandbox_dir));

    // Its environment, whole: the variables of the MCP that are not reserved,
    // what the gateway sets, and the four names every stdio server inherits.
    let environment: BTreeMap<String, String> = probe["env"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(name, _)| !SYSTEM_VARIABLES.contains(&name.as_str()))
        .map(|(name, value)| (name.clone(), value.as_str().unwrap().to_owned()))
        .collect();
    let mut expected = inherited_from_the_server();
    expected.extend(
        variables
            .iter()
            .filter(|(name, _)| !["HOME", "NODE_OPTIONS"].contains(name))
            .map(|(name, value)| ((*name).to_owned(), value.clone())),
    );
    expected.extend(
        [
            ("PATH", sandbox.server_path()),
            ("HOME", display(&sandbox_dir)),
            ("TMPDIR", display(&sandbox_dir)),
            ("NO_COLOR", "1".to_owned()),
        ]
        .map(|(name, value)| (name.to_owned(), value)),
    );
    assert_eq!(environment, expected);

    // It reads the files it was installed with, and reads and writes its sandbox.
    assert_eq!(
        probe["asset"],
        json!({ "ok": true, "value": "packaged asset" })
    );
    let written = json!({ "ok": true, "value": "written by the package" });
    assert_eq!(probe["write"]["ownState"], written);
    assert_eq!(probe["write"]["inTmpdir"], written);
    assert_eq!(probe["read"]["ownState"], written);
    assert_eq!(probe["list"]["ownSandbox"]["ok"], json!(true));
    assert_eq!(
        std::fs::read_to_string(sandbox_dir.join("state.json")).unwrap(),
        "written by the package"
    );
}

#[tokio::test]
async fn explains_a_package_that_npm_could_not_install() {
    let sandbox = Sandbox::new().await;
    let Some(runner) =
        runner_with_real_node(&sandbox, "explains_a_package_that_npm_could_not_install")
    else {
        return;
    };
    let registry = Registry::start().await;
    let mut mcp = sandbox.npm_mcp(&as_refs(&registry_variables(&registry)));
    mcp.npm_package = Some("@example/missing-mcp".to_owned());

    for failure in [
        runner.list_tools(&mcp).await.unwrap_err(),
        runner.install_npm_package(&mcp).await.unwrap_err(),
    ] {
        assert!(matches!(failure, NodeError::Install { .. }));
        let message = failure.to_string();
        assert!(
            message
                .starts_with("Failed to install npm package \"@example/missing-mcp\". npm error"),
            "{message}"
        );
        assert!(message.contains("404"), "{message}");
        // No colours, thanks to NO_COLOR.
        assert!(!message.contains('\u{1B}'), "{message:?}");
    }
    assert!(
        registry
            .requests()
            .iter()
            .any(|request| request.eq_ignore_ascii_case("GET /@example%2fmissing-mcp")),
        "{:?}",
        registry.requests()
    );
    assert_eq!(runner.installed_npm_package_version(&mcp), None);
}
