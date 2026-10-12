//! npm MCPs with nothing standing in for anything: the `mymcps` binary as
//! the launcher of the sandbox, the real npm and Node.js, and the kernel. A
//! package from a registry on 127.0.0.1 is installed and started as the
//! server does it, then asked to reach what a package must not reach.
//!
//! The sandbox needs Linux. The test that starts the package skips, with a
//! message, on another system, on a kernel without Landlock, and on a
//! machine without Node.js and npm.
#![cfg(unix)]

#[allow(dead_code)]
#[path = "../../node/tests/node/registry.rs"]
mod registry;

use std::process::Command;

const MYMCPS: &str = env!("CARGO_BIN_EXE_mymcps");

/// Say why a test did less than its name promises. Written to stderr itself,
/// so that the test harness shows it for a passing test as well.
#[allow(dead_code)]
fn announce(test: &str, message: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{test}: {message}");
}

#[test]
fn is_the_launcher_of_the_sandbox_and_starts_nothing_without_a_policy() {
    for arguments in [
        vec!["sandbox:exec"],
        vec!["sandbox:exec", "{not a policy", "/bin/echo", "ran"],
    ] {
        let output = Command::new(MYMCPS).args(&arguments).output().unwrap();
        assert_eq!(output.status.code(), Some(126), "{arguments:?}");
        assert!(output.stdout.is_empty(), "{arguments:?}");
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .starts_with("sandbox: the policy cannot be read: "),
            "{arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // It is no command of an operator.
    let help = Command::new(MYMCPS).arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(!String::from_utf8_lossy(&help.stdout).contains("sandbox"));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn holds_a_real_package_in_the_sandbox_of_the_kernel() {
    use std::path::PathBuf;

    use mymcps_core::TestCore;
    use mymcps_core::models::{Mcp, McpTransport};
    use mymcps_core::secrets::{EnvironmentInput, merge_environment};
    use mymcps_node::{HostEnvironment, NodeRunner, NodeRuntime, Sandbox, SandboxStatus};
    use serde_json::{Value, json};

    use crate::registry::{Registry, real_node, registry_variables};

    const TEST: &str = "holds_a_real_package_in_the_sandbox_of_the_kernel";
    let Some(node) = real_node() else {
        announce(TEST, "skipped, Node.js with npm is not installed");
        return;
    };

    // A server: its application directory with a `.env`, its data directory
    // with the database, and the sandbox of another MCP.
    let directory = tempfile::tempdir().unwrap();
    let application = directory.path().canonicalize().unwrap().join("app");
    std::fs::create_dir_all(&application).unwrap();
    let dotenv = application.join(".env");
    std::fs::write(&dotenv, "APP_KEY=not-for-packages\n").unwrap();
    let core = TestCore::with_config(|config| {
        config.data_dir = config.data_dir.canonicalize().unwrap();
    })
    .await;
    let database = core.config.database_path();
    let database_before = std::fs::read(&database).unwrap();

    let host = HostEnvironment {
        path: Some("/usr/bin:/bin".to_owned()),
        known_node_locations: Vec::new(),
        current_dir: application.clone(),
    };
    let runner = NodeRunner::with_runtime(
        core.core.clone(),
        NodeRuntime::with_host(None, host)
            .binary_path(node)
            .sandbox(Sandbox::Enforced {
                launcher: PathBuf::from(MYMCPS),
            }),
    );
    match runner.sandbox_status().await {
        SandboxStatus::Enforced(enforced) => {
            assert!(enforced.starts_with("Landlock ABI "), "{enforced}");
        }
        SandboxStatus::Unavailable(reason) => {
            announce(TEST, &format!("skipped, {reason}"));
            return;
        }
        SandboxStatus::Off => unreachable!("the sandbox is asked for"),
    }

    let registry = Registry::start().await;
    let variables: Vec<EnvironmentInput> = registry_variables(&registry)
        .into_iter()
        .map(|(name, value)| EnvironmentInput {
            name: name.to_owned(),
            value: Some(value),
        })
        .collect();
    let mcp = Mcp {
        id: 7,
        name: "Fake MCP".to_owned(),
        slug: "fake-mcp".to_owned(),
        transport: McpTransport::Npm,
        npm_package: Some("@example/fake-mcp".to_owned()),
        npm_env: merge_environment(&core.encryption, None, &variables),
        ..Default::default()
    };
    let sandbox_dir = runner.sandbox_root_for(mcp.id);
    let neighbour = runner.sandbox_root_for(8);
    std::fs::create_dir_all(&neighbour).unwrap();
    std::fs::write(neighbour.join("token.json"), "of another MCP").unwrap();

    // npm installs the package inside the sandbox, the first time it is asked for.
    let tools = runner.list_tools(&mcp).await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(
        runner.installed_npm_package_version(&mcp).as_deref(),
        Some("1.0.0")
    );
    let packages_dir = runner.packages_root_for(mcp.id);
    let current: Value =
        serde_json::from_slice(&std::fs::read(packages_dir.join("current.json")).unwrap()).unwrap();
    let install_dir = packages_dir.join(current["install"].as_str().unwrap());
    let package_dir = install_dir.join("node_modules/@example/fake-mcp");
    assert!(!package_dir.join("script-ran").exists());

    let arguments = json!({
        "read": {
            "database": database,
            "dotenv": dotenv,
            "neighbour": neighbour.join("token.json"),
            "current": packages_dir.join("current.json"),
            "serverEnvironment": format!("/proc/{}/environ", std::process::id()),
            "ownState": "state.json",
            "hosts": "/etc/hosts",
        },
        "list": {
            "application": application,
            "dataDirectory": core.config.data_dir,
            "sandboxes": sandbox_dir.parent().unwrap(),
            "packages": packages_dir,
            "ownSandbox": sandbox_dir,
            "ownInstall": install_dir,
        },
        "write": {
            "ownState": "state.json",
            "inTmpdir": sandbox_dir.join("scratch.tmp"),
            "application": application.join("written-by-package"),
            "dataDirectory": core.config.data_dir.join("written-by-package"),
            "neighbour": neighbour.join("token.json"),
            "ownInstall": install_dir.join("written-by-package"),
            "current": packages_dir.join("current.json"),
        },
        "chmod": { "database": database, "ownState": "state.json" },
        "truncate": { "database": database },
        // More than the sandbox lets a process hold.
        "allocate": 3_u64 * 1024 * 1024 * 1024,
    });
    let result = runner
        .call_tool(&mcp, "probe", arguments.as_object().unwrap().clone())
        .await
        .unwrap();
    let probe: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();

    // The package does what it is there for: it reads the files it was
    // installed with and the system's, keeps its state, and has threads.
    let allowed = |attempt: &Value| assert_eq!(attempt["ok"], json!(true), "{attempt}");
    assert_eq!(
        probe["asset"],
        json!({ "ok": true, "value": "packaged asset" })
    );
    let written = json!({ "ok": true, "value": "written by the package" });
    assert_eq!(probe["write"]["ownState"], written);
    assert_eq!(probe["write"]["inTmpdir"], written);
    assert_eq!(probe["read"]["ownState"], written);
    allowed(&probe["read"]["hosts"]);
    allowed(&probe["list"]["ownSandbox"]);
    allowed(&probe["list"]["ownInstall"]);
    allowed(&probe["signalItself"]);
    // The network is its to use: it sees the interfaces of the machine.
    assert_eq!(probe["interfaces"], json!({ "ok": true, "value": true }));
    assert_eq!(probe["threads"], json!({ "ok": true, "value": true }));
    assert_eq!(probe["homedir"], json!(sandbox_dir));

    // Everything else is refused by the kernel.
    let refused = |attempt: &Value, code: &str| {
        assert_eq!(attempt["ok"], json!(false), "{attempt}");
        let error = attempt["error"].as_str().unwrap();
        assert!(error.contains(code), "{error}");
    };
    // The files of the server, of other MCPs, and its own code.
    for name in [
        "database",
        "dotenv",
        "neighbour",
        "current",
        "serverEnvironment",
    ] {
        refused(&probe["read"][name], "EACCES");
    }
    for name in ["application", "dataDirectory", "sandboxes", "packages"] {
        refused(&probe["list"][name], "EACCES");
    }
    for name in [
        "application",
        "dataDirectory",
        "neighbour",
        "ownInstall",
        "current",
    ] {
        refused(&probe["write"][name], "EACCES");
    }
    refused(&probe["rewriteItself"], "EACCES");
    refused(&probe["truncate"]["database"], "EACCES");
    // What no opened file is needed for.
    refused(&probe["chmod"]["database"], "EPERM");
    refused(&probe["chmod"]["ownState"], "EPERM");
    // Other processes: none of its own, and no signal to the server.
    refused(&probe["run"], "EPERM");
    refused(&probe["signalServer"], "EPERM");
    // More memory than its limit.
    assert_eq!(
        probe["allocate"]["ok"],
        json!(false),
        "{}",
        probe["allocate"]
    );

    assert_eq!(std::fs::read(&database).unwrap(), database_before);
    assert_eq!(
        std::fs::read_to_string(neighbour.join("token.json")).unwrap(),
        "of another MCP"
    );
    for unwritten in [
        application.join("written-by-package"),
        core.config.data_dir.join("written-by-package"),
        install_dir.join("written-by-package"),
    ] {
        assert!(!unwritten.exists(), "{}", unwritten.display());
    }
}
