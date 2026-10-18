# AGENTS.md

Project-specific guidance for AI coding agents.

## Changelog

- Keep the root `CHANGELOG.md` up to date in every pull request that changes user-visible behavior, security, or operations, including dependency and developer-workflow changes that affect users or operators.
- Before final validation, add concise English entries under the current UTC date (`## YYYY-MM-DD`), grouped as `Added`, `Changed`, `Fixed`, `Security`, or `Removed`.
- Consolidate related commits into outcome-focused entries. Do not list commit hashes or merge commits, and omit tests and internal refactors unless they affect users or operators.

## Pages and design

The server renders its own pages: `maud` templates in `crates/web/src/views`, one stylesheet (`crates/web/assets/css/app.css`) and one script (`crates/web/assets/js/app.js`), both written by hand and embedded in the binary. There is no JavaScript build and no component library.

- Read [docs/design-system.md](docs/design-system.md) before you write or change a page: it lists every component with its markup and class names, the page shell, the `data-` attributes the script understands, and the protocol of the forms sent without a page load. The design it implements is the Figma file "MyMCP".
- Compose a page from the components that exist. Add CSS only when no component fits, at the end of `app.css`, with the `--mm-*` tokens: no raw colour and no value the tokens already name.
- The Content-Security-Policy is strict: no inline `style` attribute, no `<style>`, no inline script, no asset from another origin. Sizes that depend on data use the classes and `data-` attributes the design system lists (`data-pct`, `.col-<px>`), and charts are SVG drawn by the server.
- The server does the work and the script only presents it: every list, filter and page is a URL the server renders, and a form sent in the background also answers a plain post with a redirect. Dialogs are the exception: a `<dialog>` is opened by the script, so the forms that live in one (create, edit, install) need JavaScript, while the actions of a row (revoke, delete, test, enable) do not. So are passkeys, which a browser only makes and uses through its script. Keep it that way: a new page must not need the script for anything but its dialogs and its passkeys.
- Validate input with a `mymcps-vine` validator, as the rest of the code does, not with hand-written checks.

## Cursor Cloud specific instructions

Single Rust binary, `mymcps` (Cargo workspace in `crates/`, entry point `crates/cli`): an HTTP server that renders its own pages. No Redis or external DB — SQLite at `tmp/db.sqlite3`. Production packaging: `Dockerfile` (+ `docker-compose.yml`) compiles the binary and bundles Node.js with npm for npm MCPs; persist `/app/tmp`.

### Runtime
- **Rust** through rustup, and a C compiler: SQLite and the TLS library are compiled from C. The project toolchain is the version in the `rust:` image tag of the `Dockerfile`, which the workflows install as `RUST_TOOLCHAIN` and `mise.toml` pins with the Node.js of the image and cargo-audit: `mise install` sets a checkout up. There is no pnpm.
- Standard commands: the tasks of `mise.toml`, which run what CI runs (`mise run dev`, `mise run lint`, `mise run test`, `mise run audit`, `mise run build`, `mise run ci`; `mise tasks` lists them). Use them rather than the cargo command behind them; see `README.md`.
- Dev server: `cargo run --bin mymcps` reads `.env` and listens on **PORT 3333**. For cloud VMs bind with `HOST=0.0.0.0` in `.env` while keeping `APP_URL=http://localhost:3333` for redirects/cookies.
- One-time: copy `.env.example` → `.env`. The server generates its key (`tmp/app.key`) when `APP_KEY` is empty and migrates the database every time it starts; `mymcps generate:key` and `mymcps migration:run` do each step on its own. Do not commit `.env`.
- Node.js with its npm is only needed to run npm MCPs: `node` on `PATH`, or `NODE_BINARY`. They run in a kernel sandbox (`crates/sandbox`: Landlock, seccomp, resource limits) that needs Linux; elsewhere they do not start unless `NPM_SANDBOX=off` is in `.env`. The tests of what the sandbox refuses (`crates/sandbox/tests`, `crates/cli/tests/npm_sandbox.rs`) only run on Linux: on another system, run them in a Linux container before changing `crates/sandbox` or how `crates/node` starts a process.

### Cargo gotchas
- CI passes `--locked` to every Cargo command, and so does the image build: commit `Cargo.lock` with any dependency change.
- `crates/xtask` is the release tooling, run as `cargo xtask <command>` (`version`, `nightly-version`, `prepare-release`). It must keep having no dependencies: the stable release compiles it in the job that holds the write token.
- A Rust bump changes the `rust:` tag in the `Dockerfile`, `RUST_TOOLCHAIN` in the workflows, and `rust` in `mise.toml` together; a Node.js bump changes the `node` tag, `NODE_VERSION`, and `node` in `mise.toml` together. `cargo test -p xtask` fails until they agree.

### Lint / test today
- `cargo test --workspace` runs the tests of every crate. Those of `crates/xtask` read the workflows, the `Dockerfile`, `.dockerignore`, the Compose files, and `mise.toml`, and fail when one of the guarantees of the release process or of the image is lost: run `cargo test -p xtask` after changing any of these files.
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` are expected to pass; a failure is a regression, not an environment issue.
