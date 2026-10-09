//! The guarantees of the container image and of the files that deploy it,
//! checked against the Dockerfile, the Compose files and the workflows.

mod common;

use common::{
    has_lines, is_lowercase_hex, jobs_of, read_repository_file, repository_root, without_comments,
    words, workflows,
};

/// A `FROM` line of a Dockerfile.
struct From<'a> {
    flags: Vec<&'a str>,
    image: &'a str,
    stage: Option<&'a str>,
}

fn from_lines(dockerfile: &str) -> Vec<From<'_>> {
    dockerfile
        .lines()
        .filter_map(|line| {
            let mut tokens = line.split_whitespace().peekable();
            if !tokens.next()?.eq_ignore_ascii_case("FROM") {
                return None;
            }
            let mut flags = Vec::new();
            while let Some(flag) = tokens.next_if(|token| token.starts_with("--")) {
                flags.push(flag);
            }
            let image = tokens.next().expect("FROM names an image");
            let stage = match (tokens.next(), tokens.next(), tokens.next()) {
                (None, _, _) => None,
                (Some(keyword), Some(stage), None) if keyword.eq_ignore_ascii_case("AS") => {
                    Some(stage)
                }
                _ => panic!("cannot read `{line}`"),
            };
            Some(From {
                flags,
                image,
                stage,
            })
        })
        .collect()
}

/// Images pulled from a registry, in order. A FROM that continues an earlier
/// stage is not a base image.
fn base_images(dockerfile: &str) -> Vec<&str> {
    let mut stages: Vec<&str> = Vec::new();
    let mut images = Vec::new();

    for from in from_lines(dockerfile) {
        if !stages.contains(&from.image) {
            images.push(from.image);
        }
        stages.extend(from.stage);
    }

    images
}

/// `name:tag@sha256:digest`, spelled out: nothing may come from an ARG.
fn is_pinned(image: &str) -> bool {
    let Some((reference, digest)) = image.split_once("@sha256:") else {
        return false;
    };
    let Some((name, tag)) = reference.split_once(':') else {
        return false;
    };
    let is_name =
        |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"./-".contains(&byte);
    let is_tag = |byte: u8| byte.is_ascii_alphanumeric() || b"._-".contains(&byte);

    !name.is_empty()
        && name.bytes().all(is_name)
        && !tag.is_empty()
        && tag.bytes().all(is_tag)
        && is_lowercase_hex(digest, 64)
}

/// The tag of the base image called `name`.
fn tag_of<'a>(dockerfile: &'a str, name: &str) -> &'a str {
    let prefix = format!("{name}:");
    let image = base_images(dockerfile)
        .into_iter()
        .find(|image| image.starts_with(&prefix))
        .unwrap_or_else(|| panic!("expected a {name} base image"));
    &image[prefix.len()..image.find('@').expect("a digest")]
}

/// The instructions of the last stage, the one the image is made of.
fn runtime_stage(dockerfile: &str) -> &str {
    &dockerfile[dockerfile.rfind("\nFROM ").expect("a runtime stage") + 1..]
}

#[test]
fn every_base_image_is_pinned_by_tag_and_digest_in_its_from_line() {
    let dockerfile = read_repository_file("Dockerfile");
    let images = base_images(&dockerfile);

    assert_eq!(images.len(), 3, "{images:?}");
    for image in images {
        // Dependabot can only update a tag and digest it reads in the FROM
        // line itself, so neither may come from an ARG.
        assert!(is_pinned(image), "{image} is not pinned by tag and digest");
    }

    assert!(is_pinned(
        "denoland/deno:bin-2.9.7@sha256:bc5aa4466e21b6d3021226a85ba2e1911f7c386254d97b9d797903ab74edace2"
    ));
    for image in [
        "debian:trixie-slim",
        "debian@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f",
        "debian:trixie-slim@sha256:a29215f6",
        "debian:$DEBIAN_TAG@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f",
        "$RUNTIME_IMAGE",
    ] {
        assert!(!is_pinned(image), "{image}");
    }
}

#[test]
fn the_server_is_compiled_for_the_debian_release_it_runs_on() {
    let dockerfile = read_repository_file("Dockerfile");
    let stages = from_lines(&dockerfile);

    // The binary is linked against the C library of the build stage and
    // copied into the runtime stage.
    let build = tag_of(&dockerfile, "rust");
    let runtime = tag_of(&dockerfile, "debian");
    let release = runtime
        .strip_suffix("-slim")
        .expect("a debian:<release>-slim runtime image");
    assert!(
        build.ends_with(&format!("-slim-{release}")),
        "rust:{build} is not built on Debian {release}"
    );

    // Only the build stage runs on the architecture of the builder; it
    // cross-compiles for the image's. The runtime stage is the last one.
    for stage in &stages {
        let expected: &[&str] = if stage.stage == Some("build") {
            &["--platform=$BUILDPLATFORM"]
        } else {
            &[]
        };
        assert_eq!(stage.flags, expected, "{}", stage.image);
    }
    assert_eq!(stages.last().unwrap().stage, Some("runtime"));
    assert!(dockerfile.contains("\nARG TARGETARCH\n"));
    assert!(dockerfile.contains("    amd64) target=x86_64-unknown-linux-gnu;"));
    assert!(dockerfile.contains("    arm64) target=aarch64-unknown-linux-gnu;"));
}

#[test]
fn the_build_caches_dependencies_and_always_recompiles_the_workspace() {
    let dockerfile = read_repository_file("Dockerfile");

    for (cache, target) in [
        ("registry", "/usr/local/cargo/registry"),
        ("git", "/usr/local/cargo/git"),
        ("target", "/src/target"),
    ] {
        assert!(
            dockerfile.contains(&format!(
                "--mount=type=cache,id=mymcps-cargo-{cache},target={target},sharing=locked \\\n"
            )),
            "{cache}"
        );
    }

    // Cargo trusts a cached artifact that is newer than its sources, and a
    // build context can hold changed files with an old date: without the
    // touch, such a build ships the binary of the previous one. --locked
    // refuses a Cargo.lock that does not match the manifests.
    assert!(dockerfile.contains(
        "  find crates -type f -exec touch {} + \\\n  && cargo build --release --locked --bin mymcps \\\n"
    ));
    assert!(
        dockerfile.contains("  && cp \"target/$(cat /rust-target)/release/mymcps\" /out/mymcps\n")
    );
}

#[test]
fn the_workflows_install_the_rust_toolchain_the_image_is_compiled_with() {
    let dockerfile = read_repository_file("Dockerfile");
    let toolchain = tag_of(&dockerfile, "rust")
        .split('-')
        .next()
        .unwrap()
        .to_string();
    assert_eq!(
        toolchain.split('.').count(),
        3,
        "expected an exact Rust version in the rust image tag, not {toolchain}"
    );

    let mut compiling = 0;
    for (name, workflow) in workflows() {
        let compiles = workflow.lines().any(|line| {
            ["run", "test", "build", "clippy", "fmt"]
                .iter()
                .any(|command| line.contains(&format!("cargo {command} ")))
        });
        if !compiles {
            continue;
        }
        compiling += 1;

        assert!(
            has_lines(
                &without_comments(&workflow),
                &format!("env:\n  RUST_TOOLCHAIN: {toolchain}\n")
            ),
            "{name} must set RUST_TOOLCHAIN to {toolchain}, the version of the rust image in the Dockerfile"
        );
        // Every job that runs cargo installs that toolchain and makes it the
        // default first.
        for (id, job) in jobs_of(&workflow) {
            let Some(first_cargo) = job.find("cargo ") else {
                continue;
            };
            let before = &job[..first_cargo];
            assert!(
                before.contains("rustup toolchain install \"$RUST_TOOLCHAIN\" ")
                    && before.contains("rustup default \"$RUST_TOOLCHAIN\"\n"),
                "{name}: the {id} job runs cargo before it sets up the toolchain"
            );
        }
    }
    assert_eq!(compiling, 3, "quality, nightly release and stable release");
}

#[test]
fn deno_version_matches_the_pinned_deno_image() {
    let dockerfile = read_repository_file("Dockerfile");

    let image = dockerfile
        .lines()
        .find_map(|line| {
            line.strip_prefix("FROM denoland/deno:bin-")?
                .strip_suffix(" AS deno")
        })
        .expect("a denoland/deno:bin-<version> stage called deno");
    assert!(is_pinned(&format!("denoland/deno:bin-{image}")));
    let version = &image[..image.find('@').unwrap()];
    assert!(
        version.split('.').count() == 3
            && version
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())),
        "{version}"
    );

    let variable = dockerfile
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("DENO_VERSION="))
        .and_then(|value| value.split_whitespace().next())
        .expect("the runtime stage to set DENO_VERSION");

    assert_eq!(
        variable, version,
        "DENO_VERSION must be updated together with the denoland/deno image tag"
    );

    // And the binary is where DENO_PATH says.
    let runtime = runtime_stage(&dockerfile);
    assert!(runtime.contains("\nCOPY --from=deno /deno /usr/local/bin/deno\n"));
    assert!(runtime.contains("  DENO_PATH=/usr/local/bin/deno \\\n"));
}

#[test]
fn dependabot_updates_the_dockerfile_base_images_cargo_and_the_actions() {
    let dependabot = read_repository_file(".github/dependabot.yml");

    for ecosystem in ["docker", "cargo", "github-actions"] {
        assert!(
            has_lines(
                &dependabot,
                &format!("  - package-ecosystem: {ecosystem}\n    directory: /\n")
            ),
            "{ecosystem}"
        );
    }
    assert_eq!(dependabot.matches("package-ecosystem:").count(), 3);
}

#[test]
fn the_build_context_leaves_out_secrets_git_data_and_coding_agent_state() {
    let dockerignore = read_repository_file(".dockerignore");
    let patterns: Vec<&str> = dockerignore
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    let position = |entry: &str| {
        patterns
            .iter()
            .position(|pattern| *pattern == entry)
            .unwrap_or_else(|| panic!("expected .dockerignore to list {entry}"))
    };

    // Everything is left out, except the Cargo workspace.
    assert_eq!(patterns[0], "*");
    let allowed: Vec<&str> = patterns
        .iter()
        .filter_map(|pattern| pattern.strip_prefix('!'))
        .collect();
    assert_eq!(allowed, ["Cargo.toml", "Cargo.lock", ".cargo", "crates"]);

    // The last matching pattern wins, so what must never enter the context
    // comes after the exceptions, for the root and for any directory.
    let last_exception = position("!crates");
    for entry in [".env", ".env.*", ".git", ".claude", ".codex", ".cursor"] {
        assert!(position(entry) > last_exception, "{entry}");
        assert!(
            position(&format!("**/{entry}")) > last_exception,
            "**/{entry}"
        );
    }
    assert!(position("**/target") > last_exception);

    // The Dockerfile copies nothing else from the context.
    let dockerfile = read_repository_file("Dockerfile");
    let mut copied = Vec::new();
    for line in dockerfile.lines() {
        let Some(arguments) = line.strip_prefix("COPY ") else {
            continue;
        };
        if arguments.starts_with("--from=") {
            continue;
        }
        let mut paths: Vec<&str> = arguments.split_whitespace().collect();
        paths.pop().expect("a destination");
        copied.extend(paths);
    }
    assert_eq!(copied, allowed);
}

#[test]
fn the_image_runs_the_server_as_the_user_and_in_the_paths_of_earlier_images() {
    let dockerfile = read_repository_file("Dockerfile");
    let runtime = runtime_stage(&dockerfile);
    let has = |instruction: &str| has_lines(runtime, instruction);

    // uid and gid 1000 own the files of a volume created by an earlier image.
    assert!(runtime.contains("  && groupadd --gid 1000 mymcps \\\n"));
    assert!(runtime.contains("  && useradd --uid 1000 --gid 1000 "));
    assert!(has("USER 1000:1000\n"));

    // The volume, and what the server keeps in it.
    assert!(has("WORKDIR /app\n"));
    assert!(has("VOLUME [\"/app/tmp\"]\n"));
    assert!(runtime.contains("  DATA_DIR=/app/tmp \\\n"));
    assert!(runtime.contains("  DENO_DIR=/app/tmp/deno-cache \\\n"));
    // A new volume starts from this directory: private to the server.
    assert!(runtime.contains(
        "  && install -d -o 1000 -g 1000 -m 0700 /app/tmp /app/tmp/mcp-sandboxes /app/tmp/deno-cache\n"
    ));

    assert!(has("EXPOSE 3333\n"));
    assert!(runtime.contains("  PORT=3333 \\\n"));
    assert!(runtime.contains("ENV APP_ENV=production \\\n"));
    assert!(has(
        "HEALTHCHECK --interval=30s --timeout=5s --start-period=30s --retries=3 \\\n  CMD [\"mymcps\", \"healthcheck\"]\n"
    ));
    assert!(runtime.contains(" ca-certificates dumb-init \\\n"));

    // An init that reaps the Deno children, then the server itself: there is
    // no entrypoint script. The server sets its umask, reads or creates its
    // key and migrates the database (crates/cli); the smoke test of
    // publish-container.yml checks the result in the built image.
    assert!(has("ENTRYPOINT [\"dumb-init\", \"--\"]\n"));
    assert!(has("CMD [\"mymcps\", \"serve\"]\n"));
    assert!(has("COPY --from=build /out/mymcps /usr/local/bin/mymcps\n"));
    assert!(!repository_root().join("docker-entrypoint.sh").exists());
    assert!(!dockerfile.contains("entrypoint.sh"));

    // Nothing of the Node image is left in the instructions.
    for word in words(&without_comments(&dockerfile)) {
        assert!(
            !["node", "pnpm", "npm", "corepack"].contains(&word.to_ascii_lowercase().as_str()),
            "the Dockerfile mentions {word}"
        );
    }
}

#[test]
fn the_deployment_files_keep_the_container_restricted_and_the_checks_of_earlier_images() {
    let compose = read_repository_file("docker-compose.yml");

    assert!(compose.contains("    build: .\n"));
    assert!(compose.contains("    expose:\n      - '3333'\n"));
    assert!(compose.contains("    volumes:\n      - mymcps-data:/app/tmp\n"));
    assert!(compose.contains("    security_opt:\n      - no-new-privileges:true\n"));
    assert!(compose.contains("    cap_drop:\n      - ALL\n"));
    assert!(compose.contains("    restart: unless-stopped\n"));
    assert!(compose.contains("      TRUST_PROXY: ${TRUST_PROXY:-loopback,uniquelocal}\n"));
    assert!(compose.contains("      APP_KEY: ${APP_KEY:-}\n"));
    assert!(compose.contains("      APP_URL: ${APP_URL:-}\n"));
    assert!(compose.contains("      DENO_PATH: /usr/local/bin/deno\n"));

    let local = read_repository_file("docker-compose.local.yml");
    assert!(local.contains("      - '${BIND_ADDRESS:-127.0.0.1}:${PORT:-3333}:3333'\n"));

    let coolify = read_repository_file("coolify.json");
    for setting in [
        "\"location\": \"/docker-compose.yml\"",
        "\"expose\": \"3333\"",
        "\"path\": \"/health\"",
        "\"port\": \"3333\"",
        "\"return_code\": 200",
        "\"key\": \"TRUST_PROXY\"",
        "\"value\": \"loopback,uniquelocal\"",
    ] {
        assert!(coolify.contains(setting), "{setting}");
    }
}

#[test]
fn the_variables_the_server_no_longer_reads_are_gone() {
    let mut files = vec![
        ("Dockerfile".to_string(), read_repository_file("Dockerfile")),
        (
            "docker-compose.yml".to_string(),
            read_repository_file("docker-compose.yml"),
        ),
        (
            "docker-compose.local.yml".to_string(),
            read_repository_file("docker-compose.local.yml"),
        ),
        (
            "coolify.json".to_string(),
            read_repository_file("coolify.json"),
        ),
        (
            ".env.example".to_string(),
            read_repository_file(".env.example"),
        ),
    ];
    files.extend(workflows());

    for (name, text) in files {
        for variable in ["SESSION_DRIVER", "APP_NAME", "VITE_APP_NAME", "NODE_ENV"] {
            assert!(
                !words(&text).any(|word| word == variable),
                "{name} still sets {variable}"
            );
        }
    }
}
