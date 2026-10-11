//! The cases of `tests/unit/mcp_environment.spec.ts` about the Deno runner.
//! The others belong to the form of an MCP (validator, controller,
//! transformer) and to `mymcps_core::secrets`.

use std::path::Path;

use mymcps_deno::DenoError;

use crate::support::{Sandbox, display, environment_inputs};

mod npm_mcp_environment_variables {
    use mymcps_core::secrets::merge_environment;

    use super::*;

    /// The half of this case about the runner. That saving an HTTP MCP clears
    /// its variables is checked where `assignMcpFromPayload` is ported.
    #[tokio::test]
    async fn clears_variables_for_http_and_protects_the_deno_runtime_environment() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let mut mcp = sandbox.npm_mcp(&[]);
        mcp.npm_package = Some("@example/environment-mcp".to_owned());
        mcp.npm_env = merge_environment(
            &sandbox.core.encryption,
            None,
            &environment_inputs(&[("API_KEY", "secret"), ("HOME", "/unsafe")]),
        );

        assert_eq!(
            runner
                .build_environment(&mcp, Path::new("/safe/sandbox"))
                .unwrap(),
            [
                ("API_KEY", "secret".to_owned()),
                ("PATH", sandbox.server_path()),
                ("HOME", "/safe/sandbox".to_owned()),
                ("TMPDIR", "/safe/sandbox".to_owned()),
                ("DENO_DIR", display(&runner.resolve_deno_dir())),
                ("NO_COLOR", "1".to_owned()),
            ]
            .map(|(name, value)| (name.to_owned(), value))
        );
    }

    #[tokio::test]
    async fn hands_the_package_an_empty_path_when_the_server_has_none() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner_with(|host| host.path = None);
        let environment = runner
            .build_environment(&sandbox.npm_mcp(&[]), Path::new("/safe/sandbox"))
            .unwrap();
        assert_eq!(environment[0], ("PATH".to_owned(), String::new()));
    }

    #[tokio::test]
    async fn grants_deno_sandbox_permissions_including_os_homedir_for_node_npm_mcps() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let mut mcp = sandbox.npm_mcp(&[]);
        mcp.npm_package = Some("@shopify/dev-mcp".to_owned());
        mcp.npm_version = Some("1.14.4".to_owned());
        mcp.npm_args = Some(serde_json::json!(["--flag"]).to_string());

        assert_eq!(
            runner.build_args(&mcp, Path::new("/safe/sandbox")).unwrap(),
            [
                "run".to_owned(),
                "--quiet".to_owned(),
                "--node-modules-dir=none".to_owned(),
                "--no-lock".to_owned(),
                format!(
                    "--allow-read=/safe/sandbox,{}",
                    runner.resolve_deno_dir().display()
                ),
                "--allow-write=/safe/sandbox".to_owned(),
                "--deny-write=/safe/sandbox/.npmrc,/safe/sandbox/deno.json,/safe/sandbox/deno.jsonc,/safe/sandbox/package.json".to_owned(),
                "--allow-net".to_owned(),
                "--allow-env".to_owned(),
                "--allow-sys=homedir".to_owned(),
                "--no-prompt".to_owned(),
                "npm:@shopify/dev-mcp@1.14.4".to_owned(),
                "--flag".to_owned(),
            ]
        );
    }

    #[tokio::test]
    async fn runs_the_latest_version_of_an_mcp_that_names_none() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let mut mcp = sandbox.npm_mcp(&[]);
        for version in [None, Some(""), Some("  ")] {
            mcp.npm_version = version.map(str::to_owned);
            let args = runner.build_args(&mcp, Path::new("/safe/sandbox")).unwrap();
            assert_eq!(args.last().unwrap(), "npm:@example/fake-mcp@latest");
            assert_eq!(args.len(), 12);
        }
    }

    /// `McpEnvironmentStore.decrypt` itself is tested in `mymcps_core::secrets`.
    #[tokio::test]
    async fn fails_closed_when_stored_json_or_ciphertext_is_corrupted() {
        let sandbox = Sandbox::new().await;
        let runner = sandbox.runner();
        let mut mcp = sandbox.npm_mcp(&[]);

        mcp.npm_env = Some("{not-json".to_owned());
        let error = runner
            .build_environment(&mcp, Path::new("/safe/sandbox"))
            .unwrap_err();
        assert!(matches!(error, DenoError::Environment(_)));
        assert_eq!(
            error.to_string(),
            "Environment variable configuration is corrupted"
        );

        mcp.npm_env = Some(serde_json::json!({ "API_KEY": "not-ciphertext" }).to_string());
        assert_eq!(
            runner
                .build_environment(&mcp, Path::new("/safe/sandbox"))
                .unwrap_err()
                .to_string(),
            "Environment variable \"API_KEY\" could not be decrypted"
        );
    }
}
