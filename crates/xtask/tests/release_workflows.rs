//! The guarantees of the GitHub Actions workflows, checked against the
//! workflow files themselves.

mod common;

use common::{
    count, has_lines, is_lowercase_hex, job, job_ids, jobs_of, read_repository_file, read_workflow,
    repository_root, steps_of, words, workflows,
};

const RELEASE_WORKFLOWS: [(&str, &str); 2] = [
    ("stable-release.yml", "stable"),
    ("nightly-release.yml", "nightly"),
];

const CHECKOUT: &str = "uses: actions/checkout@";

/// The lines of `text` that run a cargo command.
fn cargo_commands(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|line| words(line).any(|word| word == "cargo"))
        .collect()
}

/// True when the command compiles nothing but the repository's own xtask
/// crate, which has no dependencies.
fn builds_only_xtask(command: &str) -> bool {
    command.contains("cargo run --locked -p xtask -- ")
        || command.trim() == "cargo test --locked -p xtask"
        || command.trim() == "run: cargo test --locked -p xtask"
}

#[test]
fn release_workflows_publish_their_exact_successful_release_through_the_reusable_workflow() {
    for (name, channel) in RELEASE_WORKFLOWS {
        let workflow = read_workflow(name);

        assert!(workflow.contains("published: ${{ steps.publish.outputs.published }}"));
        assert!(
            workflow.contains("release_tag: ${{ steps.release.outputs.tag }}")
                || workflow.contains("release_tag: ${{ steps.publish.outputs.tag }}")
        );
        assert!(workflow.contains("if: needs.release.outputs.published == 'true'"));
        assert!(workflow.contains("uses: ./.github/workflows/publish-container.yml"));
        assert!(workflow.contains(&format!("release_channel: {channel}")));
        assert!(workflow.contains("release_tag: ${{ needs.release.outputs.release_tag }}"));
    }
}

#[test]
fn release_workflows_keep_dependency_code_away_from_the_write_token() {
    for (name, _) in RELEASE_WORKFLOWS {
        let workflow = read_workflow(name);
        let jobs = jobs_of(&workflow);

        assert_eq!(job_ids(&jobs), ["validate", "release", "publish-container"]);
        assert!(has_lines(&workflow, "permissions:\n  contents: read\n\n"));
        assert_eq!(count(&workflow, "contents: write"), 1);

        // Everything that compiles or runs dependencies happens with a
        // read-only token and without credentials left in the checkout.
        let validate = job(&jobs, "validate");
        assert!(has_lines(
            validate,
            "    permissions:\n      contents: read\n    outputs:"
        ));
        assert!(validate.contains("ref: main\n"));
        assert_eq!(count(validate, CHECKOUT), 1);
        assert!(validate.contains("persist-credentials: false"));
        assert!(
            validate.contains("sha: ${{ steps.commit.outputs.sha }}")
                || validate.contains("sha: ${{ steps.plan.outputs.sha }}")
        );
        assert!(validate.contains("echo \"sha=$(git rev-parse HEAD)\""));
        for command in [
            "rustup toolchain install \"$RUST_TOOLCHAIN\" --profile minimal --component clippy --component rustfmt\n",
            "rustup default \"$RUST_TOOLCHAIN\"\n",
            "cargo test --locked -p xtask",
            "cargo fmt --all --check",
            "cargo clippy --workspace --all-targets --locked -- -D warnings",
            "cargo test --workspace --locked",
            "cargo build --release --locked --bin mymcps",
            "cargo audit",
        ] {
            assert!(
                validate.contains(command),
                "expected validate to run \"{command}\""
            );
        }
        for forbidden in [
            "git push",
            "git tag --annotate",
            "git commit",
            "gh release create",
            "gh api",
        ] {
            assert!(!validate.contains(forbidden), "validate runs {forbidden}");
        }

        // The job holding the write token only tags and publishes the commit
        // that was validated.
        let release = job(&jobs, "release");
        assert!(has_lines(release, "    needs: validate\n"));
        assert!(has_lines(
            release,
            "    permissions:\n      contents: write\n    outputs:"
        ));
        assert_eq!(count(release, CHECKOUT), 1);
        assert!(release.contains(
            "fetch-depth: 0\n          persist-credentials: true\n          ref: ${{ needs.validate.outputs.sha }}\n"
        ));
        assert!(release.contains("\"$(git rev-parse HEAD)\" != \"$VALIDATED_SHA\""));
        assert!(
            release.contains(
                "git merge-base --is-ancestor \"$VALIDATED_SHA\" refs/remotes/origin/main"
            )
        );
        assert!(release.contains("gh release create \"$TAG\""));
        assert!(release.contains("--verify-tag"));

        // It restores no cache and installs no tool: the checkout is its only
        // action, and the only thing it compiles is xtask.
        assert_eq!(count(release, "uses: "), 1);
        assert!(
            !release
                .lines()
                .any(|line| line.trim_start().starts_with("cache: "))
        );
        for command in cargo_commands(release) {
            assert!(
                builds_only_xtask(command),
                "the release job runs `{}`",
                command.trim()
            );
        }
        // The toolchain itself, without the components the checks need.
        for line in release.lines().filter(|line| line.contains("rustup")) {
            assert!(
                [
                    "rustup toolchain install \"$RUST_TOOLCHAIN\" --profile minimal",
                    "rustup default \"$RUST_TOOLCHAIN\"",
                ]
                .contains(&line.trim()),
                "the release job runs `{}`",
                line.trim()
            );
        }

        let container = job(&jobs, "publish-container");
        assert!(has_lines(container, "    needs: release\n"));
        assert!(has_lines(
            container,
            "    permissions:\n      contents: read\n      packages: write\n"
        ));
    }
}

#[test]
fn release_workflows_never_run_together() {
    for (name, _) in RELEASE_WORKFLOWS {
        assert!(has_lines(
            &read_workflow(name),
            "concurrency:\n  group: release\n  cancel-in-progress: false\n"
        ));
    }
}

#[test]
fn the_nightly_workflow_only_releases_when_the_validation_job_planned_one() {
    let workflow = read_workflow("nightly-release.yml");
    let jobs = jobs_of(&workflow);
    let validate = job(&jobs, "validate");
    let release = job(&jobs, "release");

    assert!(has_lines(
        &workflow,
        "on:\n  schedule:\n    - cron: '42 2 * * *'\n  workflow_dispatch:\n"
    ));
    assert!(validate.contains("publish: ${{ steps.plan.outputs.publish }}"));
    assert_eq!(
        count(validate, "echo \"publish=false\" >> \"$GITHUB_OUTPUT\""),
        2
    );

    // Every step after the plan is skipped when there is nothing to release,
    // and nothing before it compiles dependency code.
    let steps = steps_of(validate);
    let plan = steps
        .iter()
        .position(|step| step.contains("\n        id: plan\n"))
        .expect("a plan step");
    let after_plan = &steps[plan + 1..];
    assert_eq!(after_plan.len(), 4);
    for step in after_plan {
        assert!(
            step.contains("\n        if: steps.plan.outputs.publish == 'true'\n"),
            "{step}"
        );
    }
    for command in steps[..=plan].iter().flat_map(|step| cargo_commands(step)) {
        assert!(builds_only_xtask(command), "{command}");
    }

    assert!(has_lines(
        release,
        "    if: needs.validate.outputs.publish == 'true'\n"
    ));
    assert!(release.contains("TAG: ${{ needs.validate.outputs.tag }}"));
    assert!(release.contains("PREVIOUS_TAG: ${{ needs.validate.outputs.previous_tag }}"));
    assert!(release.contains("git push origin \"refs/tags/$TAG\""));
    assert!(release.contains("--prerelease"));
    assert!(cargo_commands(release).is_empty());
}

#[test]
fn the_nightly_tag_names_the_next_patch_version_the_date_and_the_commit() {
    let workflow = read_workflow("nightly-release.yml");
    let jobs = jobs_of(&workflow);
    let validate = job(&jobs, "validate");

    assert!(validate.contains("short_sha=\"$(git rev-parse --short=7 HEAD)\"\n"));
    assert!(validate.contains("release_date=\"$(date -u +%Y%m%d)\"\n"));
    assert!(validate.contains(
        "nightly_version=\"$(cargo run --locked -p xtask -- nightly-version --date \"$release_date\" --sha \"$short_sha\")\"\n"
    ));
    assert!(validate.contains("tag=\"v$nightly_version\"\n"));

    // The shape the release job and the container workflow accept.
    assert!(
        job(&jobs, "release").contains(
            r"tag_pattern='^v[0-9]+\.[0-9]+\.[0-9]+-nightly\.[0-9]{8}\.g([0-9a-f]{7,40})$'"
        )
    );
    assert!(
        read_workflow("publish-container.yml")
            .contains(r"tag_pattern='^v[0-9]+\.[0-9]+\.[0-9]+-nightly\.[0-9]{8}\.g[0-9a-f]{7}$'")
    );

    // And the shape xtask produces for the version of this workspace.
    let current = xtask::workspace_version(&read_repository_file("Cargo.toml"))
        .unwrap()
        .to_string();
    let nightly = xtask::nightly_version(&current, "20260808", "abc1234").unwrap();
    let (version, build) = nightly.split_once("-nightly.").unwrap();
    assert_eq!(version, xtask::bump_version(&current, "patch").unwrap());
    assert_eq!(build, "20260808.gabc1234");
}

#[test]
fn the_stable_workflow_prepares_pushes_and_publishes_in_the_release_job() {
    let workflow = read_workflow("stable-release.yml");
    let jobs = jobs_of(&workflow);
    let release = job(&jobs, "release");

    // Started by hand only.
    assert!(has_lines(
        &workflow,
        "on:\n  workflow_dispatch:\n    inputs:\n      bump:\n"
    ));
    assert!(!workflow.contains("\n  schedule:") && !workflow.contains("\n  push:"));

    assert!(release.contains("BUMP: ${{ inputs.bump }}"));
    assert!(release.contains("current_version=\"$(cargo run --locked -p xtask -- version)\"\n"));
    assert!(release.contains(
        "cargo run --locked -p xtask -- prepare-release \\\n              --bump \"$BUMP\" \\\n              --date \"$(date -u +%F)\" \\\n              --repository \"$GITHUB_REPOSITORY\"\n"
    ));
    assert!(release.contains("git diff --check\n          cargo test --locked -p xtask\n"));
    // The release commit holds the three files prepare-release writes.
    assert!(release.contains("git add -- Cargo.toml Cargo.lock CHANGELOG.md\n"));
    assert!(release.contains("git push --atomic origin HEAD:refs/heads/main \"refs/tags/$TAG\""));
    assert!(!release.contains("--prerelease"));
}

#[test]
fn the_stable_workflow_resumes_a_release_that_failed_after_its_commit_or_tag() {
    let workflow = read_workflow("stable-release.yml");
    let jobs = jobs_of(&workflow);
    let release = job(&jobs, "release");

    // A rerun recognises the release commit by the message it was given.
    assert!(release.contains("git commit --message \"chore(release): prepare $TAG\"\n"));
    assert!(release.contains(
        "[[ \"$(git log -1 --format=%s)\" == \"chore(release): prepare $current_tag\" ]]; then\n            mode=\"recover-commit\"\n"
    ));
    assert!(release.contains("mode=\"recover-tag\"\n"));

    // Only a new release changes files, and a tag already on the remote is
    // not pushed again.
    assert!(release.contains("if [[ \"$MODE\" == \"prepare\" ]]; then\n            git add -- "));
    assert!(release.contains("if [[ \"$MODE\" == \"recover-tag\" ]]; then\n            echo "));
    assert!(release.contains("if ! git rev-parse --verify --quiet \"refs/tags/$TAG\" >/dev/null; then\n            git tag --annotate \"$TAG\""));
}

#[test]
fn the_release_tool_has_no_dependencies_to_build() {
    let manifest = read_repository_file("crates/xtask/Cargo.toml");
    let mut table = "";
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            table = line;
            assert!(
                ["[package]", "[lints]", "[dependencies]"].contains(&table),
                "crates/xtask/Cargo.toml has a {table} table"
            );
        } else if table == "[dependencies]" {
            assert!(
                line.is_empty() || line.starts_with('#'),
                "xtask depends on `{line}`"
            );
        } else {
            assert!(!line.starts_with("build"), "xtask has a build script");
        }
    }
    assert!(!repository_root().join("crates/xtask/build.rs").exists());

    // What Cargo resolved: a package with dependencies lists them.
    let lockfile = read_repository_file("Cargo.lock");
    let package = lockfile
        .split("[[package]]\n")
        .find(|package| package.starts_with("name = \"xtask\"\n"))
        .expect("xtask in Cargo.lock");
    assert!(!package.contains("dependencies"), "{package}");
    assert!(!package.contains("source"), "{package}");

    // It runs no other program either.
    let sources = repository_root().join("crates/xtask/src");
    let mut checked = 0;
    for entry in std::fs::read_dir(sources).unwrap() {
        let source = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(!source.contains("process::Command") && !source.contains("extern crate"));
        checked += 1;
    }
    assert!(checked > 0);
}

#[test]
fn pull_requests_and_main_run_the_checks_with_a_read_only_token() {
    let workflow = read_workflow("quality.yml");
    let jobs = jobs_of(&workflow);
    let quality = job(&jobs, "quality");
    let tests = job(&jobs, "tests");

    let triggers = workflow
        .find("on:\n  pull_request:\n")
        .expect("a pull_request trigger");
    assert!(triggers == 0 || workflow[..triggers].ends_with('\n'));
    assert!(workflow[triggers..].contains("\n  push:\n    branches:\n      - main\n"));
    assert!(has_lines(&workflow, "permissions:\n  contents: read\n\n"));
    assert!(!workflow.contains(": write"));
    assert_eq!(
        count(&workflow, CHECKOUT),
        count(&workflow, "persist-credentials: false")
    );

    // The names branch protection requires (see SECURITY.md).
    assert!(has_lines(quality, "    name: Lint and typecheck\n"));
    assert!(has_lines(tests, "    name: Tests\n"));
    let security = read_repository_file("SECURITY.md");
    assert!(security.contains("`Lint and typecheck`") && security.contains("`Tests`"));

    assert!(quality.contains("run: cargo fmt --all --check\n"));
    assert!(
        quality.contains("run: cargo clippy --workspace --all-targets --locked -- -D warnings\n")
    );

    assert!(tests.contains("run: cargo test --locked -p xtask\n"));
    // Every crate of the workspace, then the binary the image ships.
    assert!(tests.contains("run: cargo test --workspace --locked\n"));
    assert!(tests.contains("run: cargo build --release --locked --bin mymcps\n"));
}

#[test]
fn the_security_workflow_audits_dependencies_analyses_the_code_and_scans_for_secrets() {
    let workflow = read_workflow("security.yml");
    let jobs = jobs_of(&workflow);

    assert_eq!(
        job_ids(&jobs),
        ["dependency-audit", "codeql", "secret-scan"]
    );
    assert!(has_lines(&workflow, "permissions:\n  contents: read\n\n"));
    assert_eq!(count(&workflow, ": write"), 1);
    assert_eq!(
        count(&workflow, CHECKOUT),
        count(&workflow, "persist-credentials: false")
    );
    for trigger in [
        "  pull_request:\n",
        "  push:\n",
        "  schedule:\n",
        "  workflow_dispatch:\n",
    ] {
        assert!(has_lines(&workflow, trigger), "{trigger}");
    }

    let audit = job(&jobs, "dependency-audit");
    assert!(audit.contains("tool: cargo-audit\n"));
    // Only a binary whose checksum the action knows, never one it compiles.
    assert!(audit.contains("fallback: none\n"));
    assert!(audit.contains("run: cargo audit\n"));

    let codeql = job(&jobs, "codeql");
    assert!(codeql.contains("security-events: write\n"));
    assert!(codeql.contains(
        "        language:\n          - actions\n          - javascript-typescript\n          - rust\n"
    ));
    // There is browser JavaScript for the second language to analyse.
    let scripts = repository_root().join("crates/web/assets/js");
    assert!(
        std::fs::read_dir(scripts).is_ok_and(|mut entries| entries.next().is_some()),
        "drop javascript-typescript from the CodeQL matrix when the interface ships no script"
    );

    assert!(job(&jobs, "secret-scan").contains("uses: gitleaks/gitleaks-action@"));
}

#[test]
fn container_workflow_validates_before_publishing_exact_multi_platform_channel_tags() {
    let workflow = read_workflow("publish-container.yml");

    assert!(workflow.contains("workflow_call:"));
    assert!(workflow.contains("ref: refs/tags/${{ inputs.release_tag }}"));
    assert!(workflow.contains("platforms: linux/amd64,linux/arm64"));
    assert!(workflow.contains("${{ env.IMAGE_NAME }}:${{ inputs.release_channel }}"));
    assert!(workflow.contains("${{ env.IMAGE_NAME }}:${{ inputs.release_tag }}"));
    assert!(
        workflow.contains("org.opencontainers.image.revision=${{ steps.revision.outputs.sha }}")
    );
    assert!(workflow.contains("cache-from: type=gha"));
    assert!(workflow.contains("cache-to: type=gha"));
    let builds = count(&workflow, "uses: docker/build-push-action@");
    assert_eq!(builds, 3);
    assert_eq!(count(&workflow, "provenance: false"), builds);
    assert_eq!(count(&workflow, "sbom: false"), builds);

    // Both architectures are built, loaded and started before anything is
    // pushed.
    let smoke_test = workflow
        .find("      - name: Smoke-test container health\n")
        .expect("a smoke test");
    let publish = workflow.find("push: true").expect("a publishing build");
    assert!(smoke_test < publish);
    assert_eq!(count(&workflow, "push: true"), 1);
    assert_eq!(count(&workflow[..smoke_test], "push: false"), 2);
    for arch in ["amd64", "arm64"] {
        assert!(workflow[..smoke_test].contains(&format!(
            "platforms: linux/{arch}\n          load: true\n          push: false\n"
        )));
        assert!(
            workflow[..smoke_test].contains(&format!("tags: ${{{{ env.SMOKE_IMAGE }}}}-{arch}\n"))
        );
    }

    let smoke_test = &workflow[smoke_test..publish];
    assert!(smoke_test.contains("for arch in amd64 arm64; do\n"));
    assert!(smoke_test.contains("--platform \"linux/$arch\""));
    assert!(smoke_test.contains("\"$SMOKE_IMAGE-$arch\""));
    assert!(smoke_test.contains("--env LOG_LEVEL=info"));
    // As deployed by docker-compose.yml, and as a first start: no key given.
    assert!(smoke_test.contains("--cap-drop ALL"));
    assert!(smoke_test.contains("--security-opt no-new-privileges:true"));
    assert!(!smoke_test.contains("--env APP_KEY"));
    assert!(smoke_test.contains("if [[ \"$status\" != \"healthy\" ]]; then\n"));
    // What an upgrade in place relies on.
    assert!(smoke_test.contains("test \"$(id -u):$(id -g)\" = 1000:1000\n"));
    assert!(smoke_test.contains("test -z \"$(find /app/tmp -perm /077)\"\n"));
    assert!(smoke_test.contains("/app/tmp/app.key)\" = \"600 1000:1000\"\n"));
    assert!(smoke_test.contains("/app/tmp/db.sqlite3)\" = \"600 1000:1000\"\n"));
    assert!(smoke_test.contains("\"$version\" != \"mymcps ${RELEASE_TAG#v}\""));
    assert!(smoke_test.contains("grep -q \"^deno $DENO_VERSION \""));
}

#[test]
fn every_external_action_in_every_workflow_is_pinned_to_a_full_commit_sha() {
    let workflows = workflows();
    assert!(workflows.len() >= 5);

    let mut actions = 0;
    for (name, workflow) in &workflows {
        for line in workflow.lines() {
            let line = line.trim_start();
            let Some(uses) = line
                .strip_prefix("- ")
                .unwrap_or(line)
                .strip_prefix("uses:")
            else {
                continue;
            };
            let action = uses.split_whitespace().next().expect("an action");
            if action.starts_with("./") {
                continue;
            }
            let pinned = action
                .split_once('@')
                .is_some_and(|(action, commit)| !action.is_empty() && is_lowercase_hex(commit, 40));
            assert!(pinned, "{name}: {action} is not pinned to a commit");
            actions += 1;
        }
    }
    assert!(actions > 0);
}

#[test]
fn no_workflow_installs_or_runs_node() {
    for (name, workflow) in workflows() {
        for word in words(&workflow) {
            assert!(
                ![
                    "node",
                    "nodejs",
                    "pnpm",
                    "npm",
                    "npx",
                    "yarn",
                    "corepack",
                    "playwright",
                    "mjs"
                ]
                .contains(&word.to_ascii_lowercase().as_str()),
                "{name} mentions {word}"
            );
        }
        assert!(!workflow.contains("package.json"), "{name}");
    }
}
