//! `tests/unit/deno_cache_version.spec.ts`.

use std::path::{Path, PathBuf};

use mymcps_core::TestCore;
use mymcps_deno::{
    DenoError, DenoRunner, DenoRuntime, HostEnvironment, build_deno_cache_reload_args,
};

fn fake_npm_cache(npm_package: &str, versions: &[&str], latest: Option<&str>) -> tempfile::TempDir {
    let deno_dir = tempfile::Builder::new()
        .prefix("mymcps-deno-cache-")
        .tempdir()
        .unwrap();
    let mut package_dir = deno_dir.path().join("npm").join("registry.npmjs.org");
    package_dir.extend(npm_package.split('/'));
    std::fs::create_dir_all(&package_dir).unwrap();
    for version in versions {
        std::fs::create_dir_all(package_dir.join(version)).unwrap();
    }
    if let Some(latest) = latest {
        std::fs::write(
            package_dir.join("registry.json"),
            serde_json::json!({ "dist-tags": { "latest": latest } }).to_string(),
        )
        .unwrap();
    }
    deno_dir
}

/// A runner whose `DENO_DIR` is `deno_dir`.
async fn runner(deno_dir: &Path) -> (TestCore, DenoRunner) {
    let core = TestCore::new().await;
    let host = HostEnvironment {
        deno_dir: Some(deno_dir.display().to_string()),
        ..HostEnvironment::from_process()
    };
    let runner = DenoRunner::with_runtime(core.core.clone(), DenoRuntime::with_host(None, host));
    (core, runner)
}

mod deno_npm_cache_version {
    use super::*;

    #[tokio::test]
    async fn reads_dist_tags_latest_when_the_mcp_tracks_latest() {
        let deno_dir = fake_npm_cache("@shopify/dev-mcp", &["1.14.4", "1.13.0"], Some("1.14.4"));
        let (_core, runner) = runner(deno_dir.path()).await;

        let cached = |version| runner.cached_npm_package_version("@shopify/dev-mcp", version);
        assert_eq!(cached(Some("latest")).as_deref(), Some("1.14.4"));
        assert_eq!(cached(None).as_deref(), Some("1.14.4"));
        assert_eq!(cached(Some("")).as_deref(), Some("1.14.4"));
    }

    #[tokio::test]
    async fn returns_a_pinned_version_only_when_that_folder_is_cached() {
        let deno_dir = fake_npm_cache("mongodb-mcp-server", &["2.0.0"], Some("2.1.0"));
        let (_core, runner) = runner(deno_dir.path()).await;

        let cached = |version| runner.cached_npm_package_version("mongodb-mcp-server", version);
        assert_eq!(cached(Some("2.0.0")).as_deref(), Some("2.0.0"));
        assert_eq!(cached(Some(" 2.0.0 ")).as_deref(), Some("2.0.0"));
        assert_eq!(cached(Some("9.9.9")), None);
        // The registry names a latest version that was never downloaded.
        assert_eq!(cached(Some("latest")), None);
        assert_eq!(cached(None), None);
    }

    #[tokio::test]
    async fn returns_null_when_the_package_is_not_in_the_deno_cache() {
        let missing = std::env::temp_dir().join("mymcps-missing-deno-cache");
        let (_core, runner) = runner(&missing).await;
        assert_eq!(
            runner.cached_npm_package_version("@example/missing-mcp", Some("latest")),
            None
        );
        assert_eq!(runner.cached_npm_package_version("", Some("latest")), None);
        assert_eq!(runner.cached_npm_package_version("  ", None), None);
    }

    #[tokio::test]
    async fn never_looks_outside_the_registry_directory_of_the_cache() {
        let deno_dir = fake_npm_cache("@shopify/dev-mcp", &["1.14.4"], Some("1.14.4"));
        // What a name or a version that climbs out of the cache would find.
        std::fs::create_dir_all(deno_dir.path().join("outside").join("1.0.0")).unwrap();
        std::fs::create_dir_all(deno_dir.path().join("npm").join("1.0.0")).unwrap();
        let (_core, runner) = runner(deno_dir.path()).await;

        for package in ["../../outside", "..", "@shopify/../..", "@shopify\\dev-mcp"] {
            assert_eq!(
                runner.cached_npm_package_version(package, Some("1.0.0")),
                None,
                "{package}"
            );
        }
        let absolute: PathBuf = deno_dir.path().join("outside");
        assert_eq!(
            runner.cached_npm_package_version(&absolute.display().to_string(), Some("1.0.0")),
            None
        );
        for version in ["../../1.0.0", "..", "1.14.4/..", "..\\1.14.4"] {
            assert_eq!(
                runner.cached_npm_package_version("@shopify/dev-mcp", Some(version)),
                None,
                "{version}"
            );
        }
    }
}

mod deno_npm_cache_reload_args {
    use super::*;

    #[test]
    fn ignores_the_host_package_json_node_modules_mode() {
        assert_eq!(
            build_deno_cache_reload_args("@shopify/dev-mcp").unwrap(),
            [
                "cache",
                "--reload",
                "--quiet",
                "--node-modules-dir=none",
                "--no-lock",
                "npm:@shopify/dev-mcp@latest",
            ]
        );
    }

    #[test]
    fn rejects_an_empty_package_name() {
        let error = build_deno_cache_reload_args("  ").unwrap_err();
        assert!(matches!(error, DenoError::MissingPackage));
        assert_eq!(error.to_string(), "npm MCP is missing a package name");
    }
}
