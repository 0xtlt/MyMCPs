//! `tests/unit/hardening_upstream_deno.spec.ts`. Its group "npm MCP
//! environment names" is in `src/environment_policy.rs`, and "Deno cache
//! directory" in `src/runtime.rs`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mymcps_deno::{DenoError, DenoRunner, DenoRuntime, locate_deno_binary};

use crate::support::{
    FAKE_DENO, Sandbox, Snapshot, display, executable, inherited_from_the_server,
};

mod deno_binary_resolution {
    use super::*;

    fn directory() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("mymcps-deno-binary-")
            .tempdir()
            .unwrap()
    }

    #[test]
    fn prefers_deno_path_then_the_server_path_then_the_usual_locations() {
        let directory = directory();
        let directory = directory.path();
        let configured = executable(&directory.join("configured"), "deno", FAKE_DENO);
        let on_path = executable(&directory.join("bin"), "deno", FAKE_DENO);
        let known = [executable(&directory.join("known"), "deno", FAKE_DENO)];
        let search_path = format!(
            "{}:{}",
            directory.join("empty").display(),
            directory.join("bin").display()
        );
        let missing = display(&directory.join("missing").join("deno"));
        let empty = display(&directory.join("empty"));

        assert_eq!(
            locate_deno_binary(Some(&display(&configured)), Some(&search_path), &known).unwrap(),
            configured
        );
        // A DENO_PATH that does not exist falls back instead of being spawned.
        assert_eq!(
            locate_deno_binary(Some(&missing), Some(&search_path), &known).unwrap(),
            on_path
        );
        assert_eq!(
            locate_deno_binary(Some(""), Some(&search_path), &known).unwrap(),
            on_path
        );
        assert_eq!(
            locate_deno_binary(Some(""), Some(&empty), &known).unwrap(),
            known[0]
        );
        assert_eq!(locate_deno_binary(None, None, &known).unwrap(), known[0]);
    }

    #[test]
    fn never_answers_with_a_bare_or_relative_command() {
        let directory = directory();
        let directory = directory.path();
        executable(&directory.join("bin"), "deno", FAKE_DENO);
        std::fs::write(directory.join("not-executable"), "").unwrap();
        std::fs::create_dir_all(directory.join("folder").join("deno")).unwrap();

        // Relative PATH entries would be searched from the sandbox working directory.
        let relative_only = ["", ".", "bin", "node_modules/.bin"].join(":");
        let error = locate_deno_binary(Some(""), Some(&relative_only), &[]).unwrap_err();
        assert!(matches!(error, DenoError::BinaryNotFound));
        assert_eq!(
            error.to_string(),
            "Deno was not found. Install Deno, or set DENO_PATH to the absolute path of the deno binary."
        );
        assert!(matches!(
            locate_deno_binary(Some("deno"), Some(""), &[]),
            Err(DenoError::BinaryNotFound)
        ));
        // Neither a plain file without the executable bit nor a directory called deno.
        assert!(matches!(
            locate_deno_binary(
                Some(&display(&directory.join("not-executable"))),
                Some(&display(&directory.join("folder"))),
                &[]
            ),
            Err(DenoError::BinaryNotFound)
        ));
        // Nor a usual location that is not an absolute path.
        assert!(matches!(
            locate_deno_binary(None, None, &[PathBuf::from("bin/deno")]),
            Err(DenoError::BinaryNotFound)
        ));

        let search_path = format!("bin:{}", directory.join("bin").display());
        let found = locate_deno_binary(Some(""), Some(&search_path), &[]).unwrap();
        assert!(found.is_absolute());
        assert_eq!(found, directory.join("bin").join("deno"));
    }

    #[tokio::test]
    async fn looks_deno_up_on_the_server_once_and_again_after_a_failure() {
        let sandbox = Sandbox::new().await;
        let mut host = sandbox.host();
        host.path = Some(display(&sandbox.path().join("late-bin")));
        let known = executable(&sandbox.path().join("known"), "deno", FAKE_DENO);
        let runtime = DenoRuntime::with_host(None, host.clone());

        // A failed lookup is retried.
        assert!(matches!(
            runtime.resolve_binary(),
            Err(DenoError::BinaryNotFound)
        ));
        let installed = executable(&sandbox.path().join("late-bin"), "deno", FAKE_DENO);
        assert_eq!(runtime.resolve_binary().unwrap(), installed);
        // A successful one is kept.
        std::fs::remove_file(&installed).unwrap();
        assert_eq!(runtime.resolve_binary().unwrap(), installed);

        // DENO_PATH first, then the server's PATH, then the usual locations.
        host.known_deno_locations = vec![known.clone()];
        assert_eq!(
            DenoRuntime::with_host(None, host.clone())
                .resolve_binary()
                .unwrap(),
            known
        );
        let on_path = executable(&sandbox.path().join("late-bin"), "deno", FAKE_DENO);
        assert_eq!(
            DenoRuntime::with_host(None, host.clone())
                .resolve_binary()
                .unwrap(),
            on_path
        );
        let configured = executable(&sandbox.path().join("configured"), "deno", FAKE_DENO);
        assert_eq!(
            DenoRuntime::with_host(Some(display(&configured)), host)
                .resolve_binary()
                .unwrap(),
            configured
        );
    }
}

mod deno_sandbox_process {
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
            ("DENO_DIR", "/member/cache"),
            ("DENO_V8_FLAGS", "--allow-natives-syntax"),
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
                ("DENO_DIR", display(&sandbox.deno_dir())),
                ("NO_COLOR", "1".to_owned()),
            ]
            .map(|(name, value)| (name.to_owned(), value))
        );
    }

    #[tokio::test]
    async fn starts_the_resolved_binary_with_the_gateway_environment_and_one_explicit_cache() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let mcp = sandbox.npm_mcp(&[
            ("PATH", &display(&sandbox.path().join("member-bin"))),
            ("LD_PRELOAD", "/member/evil.so"),
            ("DYLD_INSERT_LIBRARIES", "/member/evil.dylib"),
            ("DENO_DIR", &display(&sandbox.path().join("member-cache"))),
            ("DENO_V8_FLAGS", "--allow-natives-syntax"),
            ("NODE_PATH", "/member/modules"),
            ("API_KEY", "kept-secret"),
        ]);
        let sandbox_dir = runner.sandbox_root_for(mcp.id);
        let deno_dir = sandbox.deno_dir();
        // What a version of the package that ran before left where Deno reads
        // its configuration and the registry of npm, next to a file of its own.
        std::fs::create_dir_all(&sandbox_dir).unwrap();
        let planted = [".npmrc", "deno.json", "deno.jsonc", "package.json"];
        for name in planted {
            std::fs::write(sandbox_dir.join(name), "registry=http://evil.example/\n").unwrap();
        }
        std::fs::write(sandbox_dir.join("notes.txt"), "kept").unwrap();

        runner.list_tools(&mcp).await.unwrap();
        let snapshot = Snapshot::read(&sandbox_dir, "started");
        for name in planted {
            assert!(!sandbox_dir.join(name).exists(), "{name}");
        }
        assert!(sandbox_dir.join("notes.txt").exists());

        let listed = [
            "PATH",
            "HOME",
            "TMPDIR",
            "DENO_DIR",
            "NO_COLOR",
            "API_KEY",
            "NPM_CONFIG_REGISTRY",
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
            "DENO_V8_FLAGS",
            "NODE_PATH",
        ]
        .map(|name| (name, snapshot.variable(name)));
        assert_eq!(
            listed,
            [
                ("PATH", Some(sandbox.server_path().as_str())),
                ("HOME", Some(display(&sandbox_dir).as_str())),
                ("TMPDIR", Some(display(&sandbox_dir).as_str())),
                ("DENO_DIR", Some(display(&deno_dir).as_str())),
                ("NO_COLOR", Some("1")),
                ("API_KEY", Some("kept-secret")),
                ("NPM_CONFIG_REGISTRY", None),
                ("LD_PRELOAD", None),
                ("DYLD_INSERT_LIBRARIES", None),
                ("DENO_V8_FLAGS", None),
                ("NODE_PATH", None),
            ]
        );

        // The whole environment of the process: what the gateway sets, the
        // one variable of the MCP that is not reserved, and the four names an
        // MCP client lets every stdio server inherit. Nothing else of the
        // environment of this test process, which holds much more.
        let mut expected: BTreeMap<String, String> = inherited_from_the_server();
        expected.extend(
            [
                ("API_KEY", "kept-secret".to_owned()),
                ("PATH", sandbox.server_path()),
                ("HOME", display(&sandbox_dir)),
                ("TMPDIR", display(&sandbox_dir)),
                ("DENO_DIR", display(&deno_dir)),
                ("NO_COLOR", "1".to_owned()),
            ]
            .map(|(name, value)| (name.to_owned(), value)),
        );
        assert_eq!(snapshot.env, expected);
        assert!(std::env::vars_os().count() > expected.len());

        // The package may read the cache it runs from, and write only its sandbox.
        assert_eq!(
            snapshot.argv,
            [
                "run".to_owned(),
                "--quiet".to_owned(),
                "--node-modules-dir=none".to_owned(),
                "--no-lock".to_owned(),
                format!(
                    "--allow-read={},{}",
                    sandbox_dir.display(),
                    deno_dir.display()
                ),
                format!("--allow-write={}", sandbox_dir.display()),
                format!(
                    "--deny-write={0}/.npmrc,{0}/deno.json,{0}/deno.jsonc,{0}/package.json",
                    sandbox_dir.display()
                ),
                "--allow-net".to_owned(),
                "--allow-env".to_owned(),
                "--allow-sys=homedir".to_owned(),
                "--no-prompt".to_owned(),
                "npm:@example/fake-mcp@latest".to_owned(),
            ]
        );
        assert!(!deno_dir.starts_with(&sandbox_dir));
        // It runs in its sandbox, below the data directory of the server.
        assert_eq!(
            sandbox_dir,
            sandbox.core.config.data_dir.join("mcp-sandboxes").join("7")
        );
        assert_eq!(
            snapshot.cwd.canonicalize().unwrap(),
            sandbox_dir.canonicalize().unwrap()
        );
    }

    #[tokio::test]
    async fn reloads_the_cache_the_mcp_process_reads_with_the_same_environment() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let mcp = sandbox.npm_mcp(&[
            ("NPM_CONFIG_REGISTRY", "https://registry.example/"),
            ("DENO_DIR", &display(&sandbox.path().join("member-cache"))),
        ]);
        let sandbox_dir = runner.sandbox_root_for(mcp.id);

        runner.reload_npm_package_cache(&mcp).await.unwrap();
        let reload = Snapshot::read(&sandbox_dir, "reload");
        runner.list_tools(&mcp).await.unwrap();
        let started = Snapshot::read(&sandbox_dir, "started");

        assert_eq!(
            reload.argv,
            [
                "cache",
                "--reload",
                "--quiet",
                "--node-modules-dir=none",
                "--no-lock",
                "npm:@example/fake-mcp@latest",
            ]
        );
        assert_eq!(
            reload.variable("DENO_DIR"),
            Some(display(&sandbox.deno_dir()).as_str())
        );
        assert_eq!(
            reload.variable("DENO_DIR"),
            Some(display(&runner.resolve_deno_dir()).as_str())
        );
        assert_eq!(
            reload.variable("NPM_CONFIG_REGISTRY"),
            Some("https://registry.example/")
        );
        assert_eq!(reload.env, started.env);
        assert_eq!(reload.cwd, started.cwd);
    }

    #[tokio::test]
    async fn refuses_a_cache_directory_that_exposes_the_app_or_that_a_package_could_write() {
        let sandbox = Sandbox::new().await;
        let mcp = sandbox.npm_mcp(&[]);
        let with_deno_dir = |deno_dir: PathBuf| {
            sandbox.runner_with(|host| host.deno_dir = Some(display(&deno_dir)))
        };
        let app = sandbox.host().current_dir;
        let data_dir = sandbox.core.config.data_dir.clone();
        let refused = |runner: &DenoRunner| {
            let sandbox_dir = runner.sandbox_root_for(mcp.id);
            let from_args = runner.build_args(&mcp, &sandbox_dir).unwrap_err();
            let from_environment = runner.build_environment(&mcp, &sandbox_dir).unwrap_err();
            assert!(matches!(from_args, DenoError::UnsafeCacheDirectory(_)));
            assert_eq!(from_args.to_string(), from_environment.to_string());
            from_args.to_string()
        };

        let message = refused(&with_deno_dir(app.clone()));
        assert_eq!(
            message,
            format!(
                "The Deno cache directory \"{}\" must not contain the application or lie inside an MCP sandbox. Set DENO_DIR to a dedicated directory.",
                app.display()
            )
        );
        // A directory above the application holds it as well.
        assert!(refused(&with_deno_dir(sandbox.path().to_owned())).contains("must not contain"));

        let runner = sandbox.runner();
        let inside = runner.sandbox_root_for(mcp.id).join("cache");
        assert!(refused(&with_deno_dir(inside)).contains("inside an MCP sandbox"));
        let another = runner.sandbox_root_for(8);
        assert!(refused(&with_deno_dir(another)).contains("inside an MCP sandbox"));
        let all_sandboxes = data_dir.join("mcp-sandboxes");
        assert!(refused(&with_deno_dir(all_sandboxes)).contains("inside an MCP sandbox"));

        // The data directory holds the database, wherever DATA_DIR puts it.
        assert!(refused(&with_deno_dir(data_dir.clone())).contains("must not contain"));

        // The Docker layout: next to the sandboxes, inside the app's tmp directory.
        let docker = with_deno_dir(data_dir.join("deno-cache"));
        let sandbox_dir = docker.sandbox_root_for(mcp.id);
        assert!(docker.build_args(&mcp, &sandbox_dir).is_ok());
        assert!(docker.build_environment(&mcp, &sandbox_dir).is_ok());
        // A neighbour whose name only starts like the sandboxes directory.
        let neighbour = with_deno_dir(data_dir.join("mcp-sandboxes-cache"));
        assert!(neighbour.build_args(&mcp, &sandbox_dir).is_ok());
    }

    #[tokio::test]
    async fn starts_nothing_when_the_cache_directory_is_refused() {
        let sandbox = Sandbox::new().await;
        let mcp = sandbox.npm_mcp(&[]);
        let app = sandbox.host().current_dir;
        let runner = sandbox.runner_with(|host| host.deno_dir = Some(display(&app)));
        let sandbox_dir = runner.sandbox_root_for(mcp.id);

        for error in [
            runner.list_tools(&mcp).await.unwrap_err(),
            runner
                .call_tool(&mcp, "snapshot", serde_json::Map::new())
                .await
                .unwrap_err(),
            runner.reload_npm_package_cache(&mcp).await.unwrap_err(),
        ] {
            assert!(
                matches!(error, DenoError::UnsafeCacheDirectory(_)),
                "{error}"
            );
        }
        assert!(!sandbox_dir.join("started.argv").exists());
        assert!(!sandbox_dir.join("reload.argv").exists());
    }

    #[tokio::test]
    async fn removes_the_cache_deno_used_to_keep_inside_the_sandbox() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let mcp = sandbox.npm_mcp(&[]);
        let sandbox_dir = runner.sandbox_root_for(mcp.id);
        let legacy = [
            sandbox_dir.join(".cache").join("deno"),
            sandbox_dir.join("Library").join("Caches").join("deno"),
        ];
        for cache in &legacy {
            std::fs::create_dir_all(cache.join("npm").join("registry.npmjs.org")).unwrap();
        }
        std::fs::write(sandbox_dir.join("state.json"), "{}").unwrap();

        runner.list_tools(&mcp).await.unwrap();

        for cache in &legacy {
            assert!(!cache.exists(), "{}", cache.display());
        }
        // Only the cache goes: what the package itself stored stays.
        assert_eq!(
            std::fs::read_to_string(sandbox_dir.join("state.json")).unwrap(),
            "{}"
        );
        assert!(sandbox_dir.join(".cache").exists());
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
            "Failed to start Deno npm MCP \"@example/fake-mcp\". MCP error -32000: Connection closed. Output: error: cannot start, the key [REDACTED] was refused"
        );
        assert!(!message.contains("super-secret-api-key"));
        assert!(!message.contains("second-line-of-key"));
        assert!(!message.contains("Is Deno installed?"));
    }

    #[tokio::test]
    async fn does_not_quote_stderr_it_could_not_keep_whole() {
        let sandbox = Sandbox::new().await;
        let mcp = sandbox.npm_mcp(&[("FAKE_BEHAVIOUR", "flood-then-fail")]);

        let failure = sandbox.runner().list_tools(&mcp).await.unwrap_err();

        let message = failure.to_string();
        assert_eq!(
            message,
            "Failed to start Deno npm MCP \"@example/fake-mcp\". MCP error -32000: Connection closed. Output exceeded 32 KiB and is not shown."
        );
        assert!(!message.contains("xxxx"));
        assert!(message.len() < 400);
    }

    #[tokio::test]
    async fn deletes_the_sandbox_of_an_mcp_on_request() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let mcp = sandbox.npm_mcp(&[]);
        let sandbox_dir = runner.sandbox_root_for(mcp.id);
        std::fs::create_dir_all(sandbox_dir.join("nested")).unwrap();
        std::fs::write(sandbox_dir.join("nested").join("token.json"), "{}").unwrap();
        let neighbour = runner.sandbox_root_for(8);
        std::fs::create_dir_all(&neighbour).unwrap();

        runner.remove_sandbox(mcp.id).await.unwrap();
        assert!(!sandbox_dir.exists());

        // Nothing to delete, and nothing outside the sandboxes for an id that is not one.
        runner.remove_sandbox(mcp.id).await.unwrap();
        for not_an_id in [0, -1, i64::MIN, i64::MAX, (1 << 53)] {
            runner.remove_sandbox(not_an_id).await.unwrap();
        }
        let root = sandbox.core.config.data_dir.join("mcp-sandboxes");
        assert!(root.is_dir());
        assert!(neighbour.is_dir());
        assert!(sandbox.core.config.database_path().exists());
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

/// What the Node tests left to the functional suites: the requests
/// themselves, and the failures before and after a start.
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

        let (tools, called, reloaded, removed) = tokio::join!(
            assert_send(runner.list_tools(&mcp)),
            assert_send(runner.call_tool(&mcp, "snapshot", serde_json::Map::new())),
            assert_send(runner.reload_npm_package_cache(&mcp)),
            assert_send(runner.remove_sandbox(8)),
        );
        assert_eq!(tools.unwrap().len(), 2);
        assert!(called.is_ok() && reloaded.is_ok() && removed.is_ok());
        let error: Box<dyn std::error::Error + Send + Sync> = Box::new(DenoError::MissingPackage);
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
    async fn calls_a_tool_and_ends_the_process() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let mut mcp = sandbox.npm_mcp(&[]);
        mcp.npm_version = Some(" 1.2.3 ".to_owned());
        mcp.set_npm_args_list(&["--stdio".to_owned(), "--verbose".to_owned()]);
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
        assert_eq!(
            std::fs::read_to_string(sandbox_dir.join("call.json")).unwrap(),
            "{\"method\":\"tools/call\",\"params\":{\"name\":\"snapshot\",\"arguments\":{\"query\":\"status\",\"limit\":5}},\"jsonrpc\":\"2.0\",\"id\":1}\n"
        );
        let started = Snapshot::read(&sandbox_dir, "started");
        assert_eq!(
            started.argv[11..],
            ["npm:@example/fake-mcp@1.2.3", "--stdio", "--verbose"]
        );
        assert!(!is_running(&started.pid));

        // No arguments are sent as an empty object, as the gateway always did.
        runner
            .call_tool(&mcp, "snapshot", serde_json::Map::new())
            .await
            .unwrap();
        assert!(
            std::fs::read_to_string(sandbox_dir.join("call.json"))
                .unwrap()
                .contains("\"params\":{\"name\":\"snapshot\",\"arguments\":{}}")
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
            "Failed to start Deno npm MCP \"@example/fake-mcp\". MCP error -32000: Connection closed. Output: error: cannot start, the key [REDACTED] was refused"
        );
    }

    #[tokio::test]
    async fn asks_whether_deno_is_installed_when_the_binary_cannot_be_started() {
        let sandbox = Sandbox::new().await;
        let gone = sandbox.path().join("gone").join("deno");
        let runner = DenoRunner::with_runtime(
            sandbox.core.core.clone(),
            DenoRuntime::with_host(None, sandbox.host()).binary_path(&gone),
        );
        let mcp = sandbox.npm_mcp(&[]);

        let failure = runner.list_tools(&mcp).await.unwrap_err();

        assert!(failure.is_startup_failure());
        assert_eq!(
            failure.to_string(),
            format!(
                "Failed to start Deno npm MCP \"@example/fake-mcp\". Is Deno installed? spawn {} ENOENT",
                gone.display()
            )
        );

        let reload = runner.reload_npm_package_cache(&mcp).await.unwrap_err();
        assert_eq!(
            reload.to_string(),
            format!(
                "Failed to reload Deno cache for \"@example/fake-mcp\". spawn {} ENOENT",
                gone.display()
            )
        );
    }

    #[tokio::test]
    async fn fails_before_anything_is_started_when_deno_is_not_found() {
        let sandbox = Sandbox::new().await;
        let lookups = Arc::new(AtomicUsize::new(0));
        let counted = lookups.clone();
        let runner = DenoRunner::with_runtime(
            sandbox.core.core.clone(),
            DenoRuntime::with_host(None, sandbox.host()).binary(move || {
                counted.fetch_add(1, Ordering::SeqCst);
                Err(DenoError::BinaryNotFound)
            }),
        );
        let mcp = sandbox.npm_mcp(&[]);

        for error in [
            runner.list_tools(&mcp).await.unwrap_err(),
            runner
                .call_tool(&mcp, "snapshot", serde_json::Map::new())
                .await
                .unwrap_err(),
            runner.reload_npm_package_cache(&mcp).await.unwrap_err(),
        ] {
            assert_eq!(
                error.to_string(),
                "Deno was not found. Install Deno, or set DENO_PATH to the absolute path of the deno binary."
            );
            assert!(!error.is_startup_failure());
        }
        assert_eq!(lookups.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn refuses_an_mcp_without_a_package() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        for package in [None, Some(String::new())] {
            let mut mcp = sandbox.npm_mcp(&[]);
            mcp.npm_package = package;

            for error in [
                runner.list_tools(&mcp).await.unwrap_err(),
                runner.reload_npm_package_cache(&mcp).await.unwrap_err(),
                runner
                    .build_args(&mcp, &runner.sandbox_root_for(mcp.id))
                    .unwrap_err(),
            ] {
                assert!(matches!(error, DenoError::MissingPackage));
                assert_eq!(error.to_string(), "npm MCP is missing a package name");
            }
        }
        // A blank name is no package to reload.
        let mut blank = sandbox.npm_mcp(&[]);
        blank.npm_package = Some("   ".to_owned());
        assert!(matches!(
            runner.reload_npm_package_cache(&blank).await,
            Err(DenoError::MissingPackage)
        ));
        assert!(!runner.sandbox_root_for(7).join("started.argv").exists());
        assert!(!runner.sandbox_root_for(7).join("reload.argv").exists());
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
                runner.reload_npm_package_cache(mcp).await.unwrap_err(),
            ] {
                assert!(matches!(error, DenoError::Environment(_)));
                assert_eq!(error.to_string(), message);
            }
        }
        assert!(!sandbox_dir.join("started.argv").exists());
        assert!(!sandbox_dir.join("reload.argv").exists());
    }

    #[tokio::test]
    async fn reports_the_start_of_what_a_failed_cache_reload_wrote() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let mcp = sandbox.npm_mcp(&[
            ("FAKE_BEHAVIOUR", "registry-down"),
            ("API_KEY", "reload-secret"),
        ]);

        let failure = runner.reload_npm_package_cache(&mcp).await.unwrap_err();

        assert!(matches!(failure, DenoError::CacheReload { .. }));
        // As written by Deno: the caller redacts it, as it does every error it shows.
        assert_eq!(
            failure.to_string(),
            "Failed to reload Deno cache for \"@example/fake-mcp\". error: could not reach the registry of reload-secret"
        );
        assert_eq!(
            mymcps_core::redaction::sanitize_mcp_diagnostic(
                &sandbox.core.encryption,
                &failure.to_string(),
                &mcp
            ),
            "Failed to reload Deno cache for \"@example/fake-mcp\". error: could not reach the registry of [REDACTED]"
        );
    }
}
