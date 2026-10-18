//! The runner with a stand-in for Node.js: where it finds the binary, what
//! it installs and starts, with which arguments and environment, and what it
//! says when one of them fails.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mymcps_node::{
    NodeError, NodeRunner, NodeRuntime, PackageSpec, Sandbox as SandboxMode, SandboxStatus,
    locate_node_binary,
};
use mymcps_sandbox::Policy;

use crate::support::{
    FAKE_LAUNCHER, FAKE_NODE, Sandbox, Snapshot, display, environment_inputs, executable,
    inherited_from_the_server,
};

const NOT_FOUND: &str = "Node.js was not found. Install Node.js, or set NODE_BINARY to the absolute path of the node binary.";

mod node_binary_resolution {
    use super::*;

    fn directory() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("mymcps-node-binary-")
            .tempdir()
            .unwrap()
    }

    #[test]
    fn prefers_node_binary_then_the_server_path_then_the_usual_locations() {
        let directory = directory();
        let directory = directory.path();
        let configured = executable(&directory.join("configured"), "node", FAKE_NODE);
        let on_path = executable(&directory.join("bin"), "node", FAKE_NODE);
        let known = [executable(&directory.join("known"), "node", FAKE_NODE)];
        let search_path = format!(
            "{}:{}",
            directory.join("empty").display(),
            directory.join("bin").display()
        );
        let missing = display(&directory.join("missing").join("node"));
        let empty = display(&directory.join("empty"));

        assert_eq!(
            locate_node_binary(Some(&display(&configured)), Some(&search_path), &known).unwrap(),
            configured
        );
        // A NODE_BINARY that does not exist falls back instead of being spawned.
        assert_eq!(
            locate_node_binary(Some(&missing), Some(&search_path), &known).unwrap(),
            on_path
        );
        assert_eq!(
            locate_node_binary(Some(""), Some(&search_path), &known).unwrap(),
            on_path
        );
        assert_eq!(
            locate_node_binary(Some(""), Some(&empty), &known).unwrap(),
            known[0]
        );
        assert_eq!(locate_node_binary(None, None, &known).unwrap(), known[0]);
    }

    #[test]
    fn never_answers_with_a_bare_or_relative_command() {
        let directory = directory();
        let directory = directory.path();
        executable(&directory.join("bin"), "node", FAKE_NODE);
        std::fs::write(directory.join("not-executable"), "").unwrap();
        std::fs::create_dir_all(directory.join("folder").join("node")).unwrap();

        // Relative PATH entries would be searched from the sandbox working directory.
        let relative_only = ["", ".", "bin", "node_modules/.bin"].join(":");
        let error = locate_node_binary(Some(""), Some(&relative_only), &[]).unwrap_err();
        assert!(matches!(error, NodeError::BinaryNotFound));
        assert_eq!(error.to_string(), NOT_FOUND);
        assert!(matches!(
            locate_node_binary(Some("node"), Some(""), &[]),
            Err(NodeError::BinaryNotFound)
        ));
        // Neither a plain file without the executable bit nor a directory called node.
        assert!(matches!(
            locate_node_binary(
                Some(&display(&directory.join("not-executable"))),
                Some(&display(&directory.join("folder"))),
                &[]
            ),
            Err(NodeError::BinaryNotFound)
        ));
        // Nor a usual location that is not an absolute path.
        assert!(matches!(
            locate_node_binary(None, None, &[PathBuf::from("bin/node")]),
            Err(NodeError::BinaryNotFound)
        ));

        let search_path = format!("bin:{}", directory.join("bin").display());
        let found = locate_node_binary(Some(""), Some(&search_path), &[]).unwrap();
        assert!(found.is_absolute());
        assert_eq!(found, directory.join("bin").join("node"));
    }

    #[tokio::test]
    async fn looks_node_up_on_the_server_once_and_again_after_a_failure() {
        let sandbox = Sandbox::new().await;
        let mut host = sandbox.host();
        host.path = Some(display(&sandbox.path().join("late-bin")));
        let known = executable(&sandbox.path().join("known"), "node", FAKE_NODE);
        let runtime = NodeRuntime::with_host(None, host.clone());

        // A failed lookup is retried.
        assert!(matches!(
            runtime.resolve_binary(),
            Err(NodeError::BinaryNotFound)
        ));
        let installed = executable(&sandbox.path().join("late-bin"), "node", FAKE_NODE);
        assert_eq!(runtime.resolve_binary().unwrap(), installed);
        // A successful one is kept.
        std::fs::remove_file(&installed).unwrap();
        assert_eq!(runtime.resolve_binary().unwrap(), installed);

        // NODE_BINARY first, then the server's PATH, then the usual locations.
        host.known_node_locations = vec![known.clone()];
        assert_eq!(
            NodeRuntime::with_host(None, host.clone())
                .resolve_binary()
                .unwrap(),
            known
        );
        let on_path = executable(&sandbox.path().join("late-bin"), "node", FAKE_NODE);
        assert_eq!(
            NodeRuntime::with_host(None, host.clone())
                .resolve_binary()
                .unwrap(),
            on_path
        );
        let configured = executable(&sandbox.path().join("configured"), "node", FAKE_NODE);
        assert_eq!(
            NodeRuntime::with_host(Some(display(&configured)), host)
                .resolve_binary()
                .unwrap(),
            configured
        );
    }
}

mod npm_mcp_process {
    use super::*;

    #[tokio::test]
    async fn drops_reserved_names_saved_before_they_were_refused_and_sets_its_own_last() {
        let sandbox = Sandbox::new().await;
        // The validator refuses these now; rows saved earlier can still hold them.
        let mcp = sandbox.npm_mcp(&[
            ("PATH", "/member/bin"),
            ("path", "/member/bin"),
            ("HOME", "/member/home"),
            ("TMPDIR", "/member/tmp"),
            ("NO_COLOR", "0"),
            ("LD_PRELOAD", "/member/evil.so"),
            ("DYLD_INSERT_LIBRARIES", "/member/evil.dylib"),
            ("NODE_OPTIONS", "--require=/member/evil.js"),
            ("NODE_PATH", "/member/modules"),
            ("GLIBC_TUNABLES", "glibc.malloc.check=3"),
            ("API_KEY", "kept-secret"),
            ("NPM_CONFIG_REGISTRY", "https://registry.example/"),
        ]);

        let environment = sandbox
            .runner()
            .build_environment(&mcp, Path::new("/safe/sandbox"))
            .unwrap();

        assert_eq!(
            environment,
            [
                ("API_KEY", "kept-secret".to_owned()),
                (
                    "NPM_CONFIG_REGISTRY",
                    "https://registry.example/".to_owned()
                ),
                ("PATH", sandbox.server_path()),
                ("HOME", "/safe/sandbox".to_owned()),
                ("TMPDIR", "/safe/sandbox".to_owned()),
                ("NO_COLOR", "1".to_owned()),
            ]
            .map(|(name, value)| (name.to_owned(), value))
        );
    }

    #[tokio::test]
    async fn hands_the_package_an_empty_path_when_the_server_has_none() {
        let sandbox = Sandbox::new().await;
        let mut host = sandbox.host();
        host.path = None;
        let runner = sandbox.runner_with(NodeRuntime::with_host(None, host));
        let environment = runner
            .build_environment(&sandbox.npm_mcp(&[]), Path::new("/safe/sandbox"))
            .unwrap();
        assert_eq!(environment[0], ("PATH".to_owned(), String::new()));
    }

    #[test]
    fn installs_without_running_a_script_reading_a_configuration_file_or_linking_an_executable() {
        let mcp = mymcps_core::models::Mcp {
            npm_package: Some("@shopify/dev-mcp".to_owned()),
            npm_version: Some("1.14.4".to_owned()),
            ..Default::default()
        };
        assert_eq!(
            NodeRunner::build_install_args(
                Path::new("/opt/node/lib/node_modules/npm/bin/npm-cli.js"),
                Path::new("/app/tmp/mcp-packages/7/abc"),
                &PackageSpec::of(&mcp).unwrap(),
            ),
            [
                "/opt/node/lib/node_modules/npm/bin/npm-cli.js",
                "install",
                "--prefix",
                "/app/tmp/mcp-packages/7/abc",
                "--cache",
                "/app/tmp/mcp-packages/7/abc/.npm/cache",
                "--userconfig",
                "/app/tmp/mcp-packages/7/abc/.npm/npmrc",
                "--globalconfig",
                "/app/tmp/mcp-packages/7/abc/.npm/npmrc-global",
                "--global=false",
                "--ignore-scripts",
                "--no-bin-links",
                "--no-package-lock",
                "--omit=dev",
                "--no-audit",
                "--no-fund",
                "--no-update-notifier",
                "--no-progress",
                "--loglevel=error",
                "--",
                "@shopify/dev-mcp@1.14.4",
            ]
        );
    }

    #[tokio::test]
    async fn installs_the_package_once_and_starts_its_executable_with_the_gateway_environment() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let installs = sandbox.path().join("installs");
        let mut mcp = sandbox.npm_mcp(&[
            ("PATH", &display(&sandbox.path().join("member-bin"))),
            ("LD_PRELOAD", "/member/evil.so"),
            ("DYLD_INSERT_LIBRARIES", "/member/evil.dylib"),
            ("NODE_OPTIONS", "--require=/member/evil.js"),
            ("NODE_PATH", "/member/modules"),
            ("API_KEY", "kept-secret"),
            ("FAKE_INSTALLS", &display(&installs)),
            ("FAKE_VERSION", "1.4.2"),
        ]);
        mcp.set_npm_args_list(&["--stdio".to_owned(), "--verbose".to_owned()]);
        let sandbox_dir = runner.sandbox_root_for(mcp.id);
        // What an earlier run of the package left in its sandbox is its own.
        std::fs::create_dir_all(&sandbox_dir).unwrap();
        std::fs::write(sandbox_dir.join("notes.txt"), "kept").unwrap();
        assert_eq!(runner.installed_npm_package_version(&mcp), None);

        runner.list_tools(&mcp).await.unwrap();
        runner.list_tools(&mcp).await.unwrap();

        // Installed by the first request, found by the second.
        assert_eq!(std::fs::read_to_string(&installs).unwrap(), "installed\n");
        assert_eq!(
            runner.installed_npm_package_version(&mcp).as_deref(),
            Some("1.4.2")
        );
        let install_dir = sandbox.install_dir(&runner, &mcp);
        assert_eq!(
            install_dir.parent().unwrap(),
            sandbox.core.config.data_dir.join("mcp-packages").join("7")
        );
        let entry = install_dir.join("node_modules/@example/fake-mcp/server.js");
        assert!(entry.is_file());
        assert!(sandbox_dir.join("notes.txt").exists());

        // npm ran in the install, with the variables of the MCP and a home of
        // its own inside it, which is gone with its cache.
        let installed = Snapshot::read(&install_dir, "install");
        assert_eq!(
            installed.argv,
            NodeRunner::build_install_args(
                &sandbox.npm_cli,
                &install_dir,
                &PackageSpec::of(&mcp).unwrap()
            )
            .into_iter()
            .map(|argument| argument.into_string().unwrap())
            .collect::<Vec<_>>()
        );
        assert_eq!(installed.argv.last().unwrap(), "@example/fake-mcp@latest");
        assert_eq!(
            installed.cwd.canonicalize().unwrap(),
            install_dir.canonicalize().unwrap()
        );
        let scratch = install_dir.join(".npm");
        assert_eq!(installed.variable("HOME"), Some(display(&scratch).as_str()));
        assert_eq!(
            installed.variable("TMPDIR"),
            Some(display(&scratch).as_str())
        );
        assert_eq!(installed.variable("API_KEY"), Some("kept-secret"));
        assert_eq!(installed.variable("NODE_OPTIONS"), None);
        assert!(!scratch.exists());

        // The package: its executable, then the arguments of the MCP.
        let started = Snapshot::read(&sandbox_dir, "started");
        assert_eq!(
            started.argv,
            [display(&entry).as_str(), "--stdio", "--verbose"]
        );

        // The whole environment of the process: what the gateway sets, the
        // variables of the MCP that are not reserved, and the four names an
        // MCP client lets every stdio server inherit. Nothing else of the
        // environment of this test process, which holds much more.
        let mut expected: BTreeMap<String, String> = inherited_from_the_server();
        expected.extend(
            [
                ("API_KEY", "kept-secret".to_owned()),
                ("FAKE_INSTALLS", display(&installs)),
                ("FAKE_VERSION", "1.4.2".to_owned()),
                ("PATH", sandbox.server_path()),
                ("HOME", display(&sandbox_dir)),
                ("TMPDIR", display(&sandbox_dir)),
                ("NO_COLOR", "1".to_owned()),
            ]
            .map(|(name, value)| (name.to_owned(), value)),
        );
        assert_eq!(started.env, expected);
        assert!(std::env::vars_os().count() > expected.len());

        // It runs in its sandbox, below the data directory of the server.
        assert_eq!(
            sandbox_dir,
            sandbox.core.config.data_dir.join("mcp-sandboxes").join("7")
        );
        assert_eq!(
            started.cwd.canonicalize().unwrap(),
            sandbox_dir.canonicalize().unwrap()
        );
    }

    #[tokio::test]
    async fn installs_once_for_requests_that_arrive_together() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let installs = sandbox.path().join("installs");
        let mcp = sandbox.npm_mcp(&[
            ("FAKE_INSTALLS", &display(&installs)),
            ("FAKE_BEHAVIOUR", "slow-install"),
        ]);

        // Clones share the installs under way.
        let clone = runner.clone();
        let (first, second, third) = tokio::join!(
            runner.list_tools(&mcp),
            clone.list_tools(&mcp),
            runner.call_tool(&mcp, "snapshot", serde_json::Map::new()),
        );

        assert!(first.is_ok() && second.is_ok() && third.is_ok());
        assert_eq!(std::fs::read_to_string(&installs).unwrap(), "installed\n");
    }

    #[tokio::test]
    async fn installs_again_for_another_version_and_on_request() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let installs = sandbox.path().join("installs");
        let count = || std::fs::read_to_string(&installs).unwrap().lines().count();
        let counted = display(&installs);
        let variables = |version: &str| {
            mymcps_core::secrets::merge_environment(
                &sandbox.core.encryption,
                None,
                &environment_inputs(&[("FAKE_INSTALLS", &counted), ("FAKE_VERSION", version)]),
            )
        };
        let mut mcp = sandbox.npm_mcp(&[]);
        mcp.npm_env = variables("1.0.0");

        runner.list_tools(&mcp).await.unwrap();
        let first = sandbox.install_dir(&runner, &mcp);
        assert_eq!(count(), 1);

        // Update MCP: the registry now answers `latest` with another version.
        mcp.npm_env = variables("1.1.0");
        let updated = runner.install_npm_package(&mcp).await.unwrap();
        assert_eq!(count(), 2);
        assert_eq!(updated.version, "1.1.0");
        let second = sandbox.install_dir(&runner, &mcp);
        assert_eq!(updated.directory, second);
        assert_eq!(
            updated.entry,
            second.join("node_modules/@example/fake-mcp/server.js")
        );
        assert_eq!(
            runner.installed_npm_package_version(&mcp).as_deref(),
            Some("1.1.0")
        );
        // A request that started before the update still has its files.
        assert!(first.is_dir() && second.is_dir() && first != second);

        // The next request runs the new install, and installs nothing.
        runner.list_tools(&mcp).await.unwrap();
        assert_eq!(count(), 2);
        let started = Snapshot::read(&runner.sandbox_root_for(mcp.id), "started");
        assert!(
            started.argv[0].starts_with(&display(&second)),
            "{started:?}"
        );

        // A pinned version is another install: what is there is not it.
        mcp.npm_version = Some("1.0.0".to_owned());
        assert_eq!(runner.installed_npm_package_version(&mcp), None);
        mcp.npm_env = variables("1.0.0");
        runner.list_tools(&mcp).await.unwrap();
        assert_eq!(count(), 3);
        assert_eq!(
            Snapshot::read(&sandbox.install_dir(&runner, &mcp), "install")
                .argv
                .last()
                .unwrap(),
            "@example/fake-mcp@1.0.0"
        );
        // Of the three, the first is gone.
        assert!(!first.exists());
        assert!(second.is_dir());
    }

    #[tokio::test]
    async fn starts_npm_and_the_package_through_the_launcher_of_the_sandbox() {
        let sandbox = Sandbox::new().await;
        let launcher = executable(&sandbox.path().join("launcher"), "mymcps", FAKE_LAUNCHER);
        let runner = sandbox.runner_with(sandbox.runtime().sandbox(SandboxMode::Enforced {
            launcher: launcher.clone(),
        }));
        let mcp = sandbox.npm_mcp(&[]);
        let policy = |directory: &Path| -> Policy {
            serde_json::from_str(&std::fs::read_to_string(directory.join("launch.policy")).unwrap())
                .unwrap()
        };

        assert_eq!(
            runner.sandbox_status().await,
            SandboxStatus::Enforced("a pretended sandbox".to_owned())
        );
        runner.list_tools(&mcp).await.unwrap();

        let install_dir = sandbox.install_dir(&runner, &mcp);
        let sandbox_dir = runner.sandbox_root_for(mcp.id);
        // npm reads Node and itself, and writes the install it is making.
        assert_eq!(
            policy(&install_dir),
            Policy::default()
                .read(&sandbox.fake)
                .read(sandbox.path().join("npm"))
                .write(&install_dir)
        );
        // The package reads Node and its install, and writes its sandbox.
        assert_eq!(
            policy(&sandbox_dir),
            Policy::default()
                .read(&sandbox.fake)
                .read(&install_dir)
                .write(&sandbox_dir)
        );
        // Neither is given the data directory, where the database is.
        let data_dir = &sandbox.core.config.data_dir;
        for held in [policy(&install_dir), policy(&sandbox_dir)] {
            for path in held.read.iter().chain(&held.write) {
                assert!(!data_dir.starts_with(path), "{}", path.display());
                assert!(
                    !path.starts_with(sandbox.host().current_dir),
                    "{}",
                    path.display()
                );
            }
        }
        // Behind the launcher, the same processes as without it.
        let started = Snapshot::read(&sandbox_dir, "started");
        assert_eq!(
            started.argv,
            [display(
                &install_dir.join("node_modules/@example/fake-mcp/server.js")
            )]
        );
    }

    #[tokio::test]
    async fn starts_nothing_when_the_launcher_cannot_sandbox() {
        let sandbox = Sandbox::new().await;
        let refusing = executable(
            &sandbox.path().join("launcher"),
            "mymcps",
            "#!/bin/sh\necho 'sandbox: the kernel does not enforce Landlock (it is not built in)' >&2\nexit 126\n",
        );
        let runner = sandbox.runner_with(
            sandbox
                .runtime()
                .sandbox(SandboxMode::Enforced { launcher: refusing }),
        );
        let mcp = sandbox.npm_mcp(&[]);

        assert_eq!(
            runner.sandbox_status().await,
            SandboxStatus::Unavailable(
                "the kernel does not enforce Landlock (it is not built in)".to_owned()
            )
        );
        let failure = runner.list_tools(&mcp).await.unwrap_err();
        assert_eq!(
            failure.to_string(),
            "Failed to install npm package \"@example/fake-mcp\". sandbox: the kernel does not enforce Landlock (it is not built in)"
        );
        // Nothing of npm ran, and nothing is left of the attempt.
        assert_eq!(
            std::fs::read_dir(runner.packages_root_for(mcp.id))
                .unwrap()
                .count(),
            0
        );

        // A launcher that is not there is no sandbox either.
        let gone = sandbox.path().join("gone").join("mymcps");
        let runner = sandbox.runner_with(sandbox.runtime().sandbox(SandboxMode::Enforced {
            launcher: gone.clone(),
        }));
        let SandboxStatus::Unavailable(reason) = runner.sandbox_status().await else {
            panic!("a missing launcher sandboxes nothing");
        };
        assert!(
            reason.starts_with(&format!(
                "the launcher \"{}\" cannot be started: ",
                gone.display()
            )),
            "{reason}"
        );
        assert!(runner.list_tools(&mcp).await.is_err());

        assert_eq!(sandbox.runner().sandbox_status().await, SandboxStatus::Off);
    }

    #[tokio::test]
    async fn keeps_reading_stderr_so_a_chatty_package_cannot_block_itself() {
        let sandbox = Sandbox::new().await;
        // 512 KiB before the first MCP message: more than a pipe and its stream buffer hold.
        let mcp = sandbox.npm_mcp(&[("FAKE_BEHAVIOUR", "noisy")]);

        let tools = sandbox.runner().list_tools(&mcp).await.unwrap();

        assert_eq!(
            tools.iter().map(|tool| tool.name()).collect::<Vec<_>>(),
            ["snapshot", "bare"]
        );
    }

    #[tokio::test]
    async fn reports_what_a_failed_start_wrote_to_stderr_without_its_secrets() {
        let sandbox = Sandbox::new().await;
        // A secret over two lines, like a PEM key, has to be redacted before lines are joined.
        let mcp = sandbox.npm_mcp(&[
            ("FAKE_BEHAVIOUR", "refuse-key"),
            ("API_KEY", "super-secret-api-key\nsecond-line-of-key"),
        ]);

        let failure = sandbox.runner().list_tools(&mcp).await.unwrap_err();

        assert!(failure.is_startup_failure());
        assert_eq!(failure.mcp_code(), None);
        let message = failure.to_string();
        assert_eq!(
            message,
            "Failed to start npm MCP \"@example/fake-mcp\". MCP error -32000: Connection closed. Output: error: cannot start, the key [REDACTED] was refused"
        );
        assert!(!message.contains("super-secret-api-key"));
        assert!(!message.contains("second-line-of-key"));
        assert!(!message.contains("Is Node.js installed?"));
    }

    #[tokio::test]
    async fn does_not_quote_stderr_it_could_not_keep_whole() {
        let sandbox = Sandbox::new().await;
        let mcp = sandbox.npm_mcp(&[("FAKE_BEHAVIOUR", "flood-then-fail")]);

        let failure = sandbox.runner().list_tools(&mcp).await.unwrap_err();

        let message = failure.to_string();
        assert_eq!(
            message,
            "Failed to start npm MCP \"@example/fake-mcp\". MCP error -32000: Connection closed. Output exceeded 32 KiB and is not shown."
        );
        assert!(!message.contains("xxxx"));
        assert!(message.len() < 400);
    }

    #[tokio::test]
    async fn deletes_the_sandbox_and_the_installs_of_an_mcp_on_request() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let mcp = sandbox.npm_mcp(&[]);
        runner.list_tools(&mcp).await.unwrap();
        let sandbox_dir = runner.sandbox_root_for(mcp.id);
        std::fs::create_dir_all(sandbox_dir.join("nested")).unwrap();
        std::fs::write(sandbox_dir.join("nested").join("token.json"), "{}").unwrap();
        let packages_dir = runner.packages_root_for(mcp.id);
        assert!(packages_dir.join("current.json").is_file());
        let neighbour = runner.sandbox_root_for(8);
        std::fs::create_dir_all(&neighbour).unwrap();
        let neighbour_packages = runner.packages_root_for(8);
        std::fs::create_dir_all(&neighbour_packages).unwrap();

        runner.remove_sandbox(mcp.id).await.unwrap();
        assert!(!sandbox_dir.exists());
        assert!(!packages_dir.exists());
        assert_eq!(runner.installed_npm_package_version(&mcp), None);

        // Nothing to delete, and nothing outside the sandboxes for an id that is not one.
        runner.remove_sandbox(mcp.id).await.unwrap();
        for not_an_id in [0, -1, i64::MIN, i64::MAX, (1 << 53)] {
            runner.remove_sandbox(not_an_id).await.unwrap();
        }
        let data_dir = &sandbox.core.config.data_dir;
        assert!(data_dir.join("mcp-sandboxes").is_dir());
        assert!(data_dir.join("mcp-packages").is_dir());
        assert!(neighbour.is_dir() && neighbour_packages.is_dir());
        assert!(sandbox.core.config.database_path().exists());

        // The package is installed again by the next request.
        runner.list_tools(&mcp).await.unwrap();
        assert!(packages_dir.join("current.json").is_file());
    }

    #[tokio::test]
    async fn removes_a_link_left_in_place_of_a_sandbox_without_following_it() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let kept = sandbox.path().join("kept");
        std::fs::create_dir_all(&kept).unwrap();
        std::fs::write(kept.join("file"), "kept").unwrap();
        let sandbox_dir = runner.sandbox_root_for(7);
        std::fs::create_dir_all(sandbox_dir.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&kept, &sandbox_dir).unwrap();

        runner.remove_sandbox(7).await.unwrap();

        assert!(sandbox_dir.symlink_metadata().is_err());
        assert_eq!(std::fs::read_to_string(kept.join("file")).unwrap(), "kept");
    }
}

/// The requests themselves, and the failures before and after a start.
mod requests_of_an_npm_mcp {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::*;

    fn is_running(pid: &str) -> bool {
        std::process::Command::new("/bin/sh")
            .args(["-c", &format!("kill -0 {pid} 2>/dev/null")])
            .status()
            .unwrap()
            .success()
    }

    /// The gateway serves requests on any thread of its runtime.
    #[tokio::test]
    async fn can_be_shared_between_tasks() {
        fn assert_send<T: Send>(value: T) -> T {
            value
        }
        fn assert_shareable<T: Send + Sync + Clone + 'static>(_: &T) {}

        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let mcp = sandbox.npm_mcp(&[]);
        assert_shareable(&runner);

        let (tools, called, installed, removed) = tokio::join!(
            assert_send(runner.list_tools(&mcp)),
            assert_send(runner.call_tool(&mcp, "snapshot", serde_json::Map::new())),
            assert_send(runner.install_npm_package(&mcp)),
            assert_send(runner.remove_sandbox(8)),
        );
        assert_eq!(tools.unwrap().len(), 2);
        assert!(called.is_ok() && installed.is_ok() && removed.is_ok());
        let error: Box<dyn std::error::Error + Send + Sync> = Box::new(NodeError::MissingPackage);
        assert_eq!(error.to_string(), "npm MCP is missing a package name");
    }

    #[tokio::test]
    async fn lists_the_name_description_and_input_schema_of_each_tool() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let mcp = sandbox.npm_mcp(&[]);

        let tools = runner.list_tools(&mcp).await.unwrap();

        assert_eq!(
            serde_json::to_string(&tools).unwrap(),
            json!([
                {
                    "name": "snapshot",
                    "description": "Describes the process it runs in",
                    "inputSchema": { "type": "object" }
                },
                {
                    "name": "bare",
                    "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } }
                }
            ])
            .to_string()
        );
        // The process is ended before the answer is returned.
        let started = Snapshot::read(&runner.sandbox_root_for(mcp.id), "started");
        assert!(!is_running(&started.pid));
    }

    #[tokio::test]
    async fn gives_up_on_a_call_the_package_never_answers_and_ends_the_process() {
        let wait = std::time::Duration::from_millis(300);
        let sandbox = Sandbox::with_config(|config| {
            config.tool_call_timeout = wait;
            config.tool_call_max_duration = std::time::Duration::from_secs(60);
        })
        .await;
        let runner = sandbox.runner();
        let mcp = sandbox.npm_mcp(&[]);

        let error = runner
            .call_tool(&mcp, "hang", serde_json::Map::new())
            .await
            .unwrap_err();

        let timed_out = error.timed_out().expect("a timeout");
        assert_eq!(timed_out.method, "tools/call");
        assert_eq!(timed_out.waited, wait);
        assert_eq!(error.to_string(), "MCP error -32001: Request timed out");
        let started = Snapshot::read(&runner.sandbox_root_for(mcp.id), "started");
        assert!(!is_running(&started.pid));
    }

    #[tokio::test]
    async fn calls_a_tool_and_ends_the_process() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let mut mcp = sandbox.npm_mcp(&[]);
        mcp.npm_version = Some(" 1.2.3 ".to_owned());
        let sandbox_dir = runner.sandbox_root_for(mcp.id);
        let arguments = json!({ "query": "status", "limit": 5 });

        let result = runner
            .call_tool(&mcp, "snapshot", arguments.as_object().unwrap().clone())
            .await
            .unwrap();

        assert_eq!(
            result,
            json!({ "content": [{ "type": "text", "text": "called" }] })
        );
        // The call asks for progress, with its id as the token.
        assert_eq!(
            std::fs::read_to_string(sandbox_dir.join("call.json")).unwrap(),
            "{\"method\":\"tools/call\",\"params\":{\"name\":\"snapshot\",\"arguments\":{\"query\":\"status\",\"limit\":5},\"_meta\":{\"progressToken\":1}},\"jsonrpc\":\"2.0\",\"id\":1}\n"
        );
        let installed = Snapshot::read(&sandbox.install_dir(&runner, &mcp), "install");
        assert_eq!(installed.argv.last().unwrap(), "@example/fake-mcp@1.2.3");
        let started = Snapshot::read(&sandbox_dir, "started");
        assert!(!is_running(&started.pid));

        // No arguments are sent as an empty object, as the gateway always did.
        runner
            .call_tool(&mcp, "snapshot", serde_json::Map::new())
            .await
            .unwrap();
        assert!(
            std::fs::read_to_string(sandbox_dir.join("call.json"))
                .unwrap()
                .contains("\"params\":{\"name\":\"snapshot\",\"arguments\":{},")
        );
    }

    #[tokio::test]
    async fn tells_an_error_of_the_protocol_from_a_tool_that_failed_and_from_a_failed_start() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let mcp = sandbox.npm_mcp(&[]);

        // A tool that reports a failure answered: that is a result.
        let failed = runner
            .call_tool(&mcp, "failing", serde_json::Map::new())
            .await
            .unwrap();
        assert_eq!(failed["isError"], json!(true));

        let refused = runner
            .call_tool(&mcp, "unknown", serde_json::Map::new())
            .await
            .unwrap_err();
        assert_eq!(refused.mcp_code(), Some(-32602));
        assert_eq!(
            refused.mcp_error().map(|error| error.message.as_str()),
            Some("Unknown tool: unknown")
        );
        assert_eq!(
            refused.to_string(),
            "MCP error -32602: Unknown tool: unknown"
        );
        assert!(!refused.is_startup_failure());
        let started = Snapshot::read(&runner.sandbox_root_for(mcp.id), "started");
        assert!(!is_running(&started.pid));

        let unstarted = sandbox.npm_mcp(&[
            ("FAKE_BEHAVIOUR", "refuse-key"),
            ("API_KEY", "refused-secret"),
        ]);
        let failure = runner
            .call_tool(&unstarted, "snapshot", serde_json::Map::new())
            .await
            .unwrap_err();
        assert!(failure.is_startup_failure());
        assert_eq!(failure.mcp_code(), None);
        assert_eq!(
            failure.to_string(),
            "Failed to start npm MCP \"@example/fake-mcp\". MCP error -32000: Connection closed. Output: error: cannot start, the key [REDACTED] was refused"
        );
    }

    #[tokio::test]
    async fn names_node_when_the_binary_cannot_be_started() {
        let sandbox = Sandbox::new().await;
        let gone = sandbox.path().join("gone").join("node");
        let mcp = sandbox.npm_mcp(&[]);

        // The package is installed, and Node.js is then taken away.
        sandbox.runner().list_tools(&mcp).await.unwrap();
        let runner = sandbox.runner_with(sandbox.runtime().binary_path(&gone));
        let failure = runner.list_tools(&mcp).await.unwrap_err();
        assert!(failure.is_startup_failure());
        assert_eq!(
            failure.to_string(),
            format!(
                "Failed to start npm MCP \"@example/fake-mcp\". Is Node.js installed? spawn {} ENOENT",
                gone.display()
            )
        );

        let install = runner.install_npm_package(&mcp).await.unwrap_err();
        assert_eq!(
            install.to_string(),
            format!(
                "Failed to install npm package \"@example/fake-mcp\". spawn {} ENOENT",
                gone.display()
            )
        );
    }

    #[tokio::test]
    async fn fails_before_anything_is_started_when_node_is_not_found() {
        let sandbox = Sandbox::new().await;
        let lookups = Arc::new(AtomicUsize::new(0));
        let counted = lookups.clone();
        let runner = sandbox.runner_with(sandbox.runtime().binary(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            Err(NodeError::BinaryNotFound)
        }));
        let mcp = sandbox.npm_mcp(&[]);

        for error in [
            runner.list_tools(&mcp).await.unwrap_err(),
            runner
                .call_tool(&mcp, "snapshot", serde_json::Map::new())
                .await
                .unwrap_err(),
            runner.install_npm_package(&mcp).await.unwrap_err(),
        ] {
            assert_eq!(error.to_string(), NOT_FOUND);
            assert!(!error.is_startup_failure());
        }
        assert_eq!(lookups.load(Ordering::SeqCst), 3);
        assert!(!runner.packages_root_for(mcp.id).exists());
    }

    #[tokio::test]
    async fn refuses_an_mcp_without_a_package_or_with_one_that_is_not_of_the_registry() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        for package in [None, Some(String::new()), Some("   ".to_owned())] {
            let mut mcp = sandbox.npm_mcp(&[]);
            mcp.npm_package = package;

            for error in [
                runner.list_tools(&mcp).await.unwrap_err(),
                runner.install_npm_package(&mcp).await.unwrap_err(),
            ] {
                assert!(matches!(error, NodeError::MissingPackage));
                assert_eq!(error.to_string(), "npm MCP is missing a package name");
            }
            assert_eq!(runner.installed_npm_package_version(&mcp), None);
        }

        // What npm would fetch from somewhere else, or read as an option.
        for package in [
            "file:/etc",
            "../../outside",
            "git+https://example.test/package.git",
            "--registry=http://example.test",
        ] {
            let mut mcp = sandbox.npm_mcp(&[]);
            mcp.npm_package = Some(package.to_owned());
            for error in [
                runner.list_tools(&mcp).await.unwrap_err(),
                runner.install_npm_package(&mcp).await.unwrap_err(),
            ] {
                assert!(matches!(error, NodeError::InvalidPackage(_)), "{package}");
            }
        }
        let mut mcp = sandbox.npm_mcp(&[]);
        mcp.npm_version = Some("npm:other@1".to_owned());
        assert_eq!(
            runner.list_tools(&mcp).await.unwrap_err().to_string(),
            "\"npm:other@1\" is not a version, a range of versions or a tag of an npm package"
        );

        assert!(!runner.packages_root_for(7).exists());
        assert!(!runner.sandbox_root_for(7).exists());
    }

    #[tokio::test]
    async fn fails_closed_when_the_saved_environment_cannot_be_read() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let sandbox_dir = runner.sandbox_root_for(7);

        let mut corrupted = sandbox.npm_mcp(&[]);
        corrupted.npm_env = Some("{not-json".to_owned());
        let mut undecryptable = sandbox.npm_mcp(&[]);
        undecryptable.npm_env = Some(json!({ "API_KEY": "not-ciphertext" }).to_string());

        for (mcp, message) in [
            (
                &corrupted,
                "Environment variable configuration is corrupted",
            ),
            (
                &undecryptable,
                "Environment variable \"API_KEY\" could not be decrypted",
            ),
        ] {
            for error in [
                runner.build_environment(mcp, &sandbox_dir).unwrap_err(),
                runner.list_tools(mcp).await.unwrap_err(),
                runner.install_npm_package(mcp).await.unwrap_err(),
            ] {
                assert!(matches!(error, NodeError::Environment(_)));
                assert_eq!(error.to_string(), message);
            }
        }
        // Nothing was installed or started, and nothing is left of the attempts.
        assert!(!sandbox_dir.exists());
        assert_eq!(
            std::fs::read_dir(runner.packages_root_for(7))
                .unwrap()
                .count(),
            0
        );
    }

    #[tokio::test]
    async fn reports_the_start_of_what_a_failed_install_wrote() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let mcp = sandbox.npm_mcp(&[
            ("FAKE_BEHAVIOUR", "registry-down"),
            ("API_KEY", "install-secret"),
        ]);

        for failure in [
            runner.install_npm_package(&mcp).await.unwrap_err(),
            // The first request of an MCP is its install.
            runner.list_tools(&mcp).await.unwrap_err(),
        ] {
            assert!(matches!(failure, NodeError::Install { .. }));
            assert!(!failure.is_startup_failure());
            // As written by npm: the caller redacts it, as it does every error it shows.
            assert_eq!(
                failure.to_string(),
                "Failed to install npm package \"@example/fake-mcp\". npm error could not reach the registry of install-secret"
            );
            assert_eq!(
                mymcps_core::redaction::sanitize_mcp_diagnostic(
                    &sandbox.core.encryption,
                    &failure.to_string(),
                    &mcp
                ),
                "Failed to install npm package \"@example/fake-mcp\". npm error could not reach the registry of [REDACTED]"
            );
        }
        // Nothing is left of an install that failed, and nothing is in use.
        assert_eq!(
            std::fs::read_dir(runner.packages_root_for(mcp.id))
                .unwrap()
                .count(),
            0
        );
        assert_eq!(runner.installed_npm_package_version(&mcp), None);
    }

    #[tokio::test]
    async fn refuses_a_package_that_is_not_a_program() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let mcp = sandbox.npm_mcp(&[("FAKE_BEHAVIOUR", "library")]);

        let failure = runner.list_tools(&mcp).await.unwrap_err();

        assert_eq!(
            failure.to_string(),
            "Failed to install npm package \"@example/fake-mcp\". It is not a program: its package.json names no executable (\"bin\") to start."
        );
        assert_eq!(
            std::fs::read_dir(runner.packages_root_for(mcp.id))
                .unwrap()
                .count(),
            0
        );
        assert!(
            !runner
                .sandbox_root_for(mcp.id)
                .join("started.argv")
                .exists()
        );
    }
}

mod client_options {
    use std::time::Duration;

    use mymcps_core::Config;
    use mymcps_mcp::McpError;
    use mymcps_mcp::client::{ClientError, ClientOptions, RequestTimedOut};
    use mymcps_node::client_options;

    use super::*;

    #[test]
    fn gives_a_tool_call_the_configured_wait_with_progress() {
        let mut config = Config::for_tests("tmp");
        config.tool_call_timeout = Duration::from_secs(600);
        config.tool_call_max_duration = Duration::from_secs(7200);

        let options = client_options(&config);

        assert_eq!(options.tool_call_timeout.timeout, Duration::from_secs(600));
        assert!(options.tool_call_timeout.reset_on_progress);
        assert_eq!(
            options.tool_call_timeout.max_total,
            Some(Duration::from_secs(7200))
        );
        // The handshake and the listing keep the client's own wait.
        assert_eq!(
            options.request_timeout,
            ClientOptions::default().request_timeout
        );
    }

    #[test]
    fn tells_a_timeout_from_the_other_failures() {
        let timeout = Duration::from_secs(5);
        let timed_out = NodeError::Client(ClientError::TimedOut(Box::new(RequestTimedOut {
            method: "tools/call".to_owned(),
            waited: timeout,
            error: McpError::new(-32001, "Request timed out"),
        })));
        let timed_out = timed_out.timed_out().expect("a timeout");
        assert_eq!(timed_out.method, "tools/call");
        assert_eq!(timed_out.waited, timeout);

        // A server that answered with -32001 is not a local timeout.
        let answered =
            NodeError::Client(ClientError::Mcp(McpError::new(-32001, "Request timed out")));
        assert!(answered.timed_out().is_none());
        assert!(NodeError::MissingPackage.timed_out().is_none());
    }
}
