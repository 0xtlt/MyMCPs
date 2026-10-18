# MyMCPs

MyMCPs is a self-hosted [Model Context Protocol (MCP)](https://modelcontextprotocol.io/) gateway. Connect your AI client to one endpoint, then manage every upstream MCP, credential, and access token from one dashboard.

## What it does

- Connects HTTP and npm-based MCP servers.
- Includes built-in MCPs for services with no MCP a self-hosted gateway can use: Strava, iCloud Mail, and Google Ads.
- Holds the tool calls you choose until a person approves them, for any MCP.
- Supports bearer tokens, custom headers, and OAuth.
- Lets MCP clients sign in through OAuth, with manual access tokens as a fallback.
- Exposes every allowed upstream through `GET` and `POST /mcp`.
- Records gateway activity and usage analytics.
- Exports the whole instance to one encrypted backup file, and sets a new instance up from one.

MyMCPs is self-hosted. The first user owns the instance, and nobody else gets in without an invite until that owner opens registration.

## Teams, roles, and registration

Accounts, teams, and permissions work as they do in Coolify, with one difference: who runs the instance is said of the account, not of a team.

- **Teams.** MCPs, access tokens, approvals, call logs, and analytics belong to a team, and only its members see them. An access token reaches the MCPs of its own team and no other. Every account comes with a personal team, anyone can create more from the team switcher at the top of the navigation, and the switcher is how you go from one team to another.
- **Roles.** In each team a person is a **member**, an **admin**, or an **owner**. A member sees the MCPs of the team, makes access tokens of their own, and decides the approvals those tokens ask for. An admin also adds and changes MCPs, sees every access token, the logs and the analytics, and manages members and invites up to their own rank. An owner also deletes the team and decides who else owns it.
- **Super admins.** A super admin runs the instance: they change the instance settings under **Settings → My Instance**, export backups, and manage accounts under **Users**. The first account is one, and makes others from **Users**, which asks for their own password; the instance always keeps one. It gives no access to the teams of other people, and a role in a team gives no right over the instance: an owner of a team runs that team only.
- **Root Team.** The first team of an instance, which the first account owns. It is a team like the others. An instance that upgrades finds everything it had there.
- **Sign-in protection.** Under **Settings → Sign-in protection**, anyone makes their account ask for more than its password: a 6-digit code of an **authenticator app** (1Password, Google Authenticator, Authy and the like), or a **passkey** (a fingerprint, a face, or a security key). A passkey also signs in alone, from the sign-in page, without the password. The first of the two comes with eight **recovery codes**, shown once, each of which signs in once when the app and the passkeys are out of reach. Passkeys belong to the address of the instance: they need `APP_URL` to be the address people open, and stop answering if it changes.
- **Invites.** An admin invites an email address into a team, with a role, from **Team → Create invite**. The link lets that person create their account, or join with the one they have. Invites work whether or not registration is open.
- **Registration.** Off by default. A super admin turns **Allow registration** on under **Settings → My Instance**: anyone who reaches the instance can then create an account on `/signup`. A new account sees nothing of the other teams: it gets a personal team of its own, where it can add MCPs, including npm packages that run in a sandbox on your server. Open registration only on an instance you would let strangers use that way.

## Run locally

You need a stable [Rust](https://rustup.rs/) toolchain with a C compiler (releases are built with Rust 1.99), and [Node.js](https://nodejs.org/) with its npm if you want to run npm-based MCPs.

```bash
cp .env.example .env
cargo run --bin mymcps
```

npm-based MCPs run in a sandbox that needs Linux: see [npm MCPs and their sandbox](#npm-mcps-and-their-sandbox). On macOS or Windows, set `NPM_SANDBOX=off` in `.env` to run them without one while you develop.

The server creates its key and its SQLite database under `tmp/` and applies database migrations when it starts. Open [http://localhost:3333](http://localhost:3333) and create the first account, which owns the instance.

## Connect an AI client

1. Add your upstream servers from **MCPs**.
2. Open **Access tokens**, select **Install MCP**, and copy the OAuth configuration for your client.
3. When the client opens MyMCPs, sign in and approve the connection.
4. Revoke the generated OAuth connection from **Access tokens** when you want to stop it.

OAuth clients connect to `https://your-domain.example/mcp` without a manually copied token. Set
`APP_URL` to the instance's public HTTPS origin so discovery and authorization URLs are correct.

For clients without OAuth support, create a manual token and send it as
`Authorization: Bearer <token>` on every `/mcp` request.

Tools use `{mcp-slug}__{tool-name}` names in eager mode. Super admins can choose the instance default under **Settings → My Instance**; existing instances default to eager mode. Clients can override that default per request with either `eager` or `lazy`:

```text
X-MyMCPs-Tool-Mode: lazy
```

Lazy mode exposes `list_mcps`, `tool_search`, and `call_tool` instead of loading every tool definition at once. The header takes precedence over the instance setting.

### Let an agent manage your MCPs

On the OAuth consent screen, a team admin or owner can tick **Allow this agent to manage your MCPs** (unchecked by default). That connection then also gets tools to list, add, change, delete and test the MCPs of the team, and to get the link that connects an MCP that signs in with OAuth. Secrets are never sent back. Remove the permission from **Access tokens** with **Remove MCP management**, or revoke the connection. See [docs/mcp-management-tools.md](docs/mcp-management-tools.md).

### Upstream OAuth MCPs

For providers that support MCP OAuth discovery and dynamic client registration, choose **OAuth** when adding the server, save it, then select **Connect OAuth**. Set `APP_URL` to the instance's public HTTPS URL so callback URLs are generated correctly.

A provider without dynamic client registration that reads Client ID Metadata Documents (ElevenLabs, for one) is connected the same way: MyMCPs names itself by the document it serves at `APP_URL/oauth/client-metadata.json`, which the provider fetches. That needs `APP_URL` to be an HTTPS address the provider can reach, so it does not work from `localhost`.

The Figma remote MCP (`https://mcp.figma.com/mcp`) only registers client names on its first-party allowlist, and only with a localhost redirect. MyMCPs detects that URL and registers as `Codex` there automatically. **Connect** opens Figma in a new tab; after you approve access, that tab lands on a `http://localhost:…/callback?code=…` address that fails to load. Copy it from the address bar and paste it into **Callback address** in the MCP's edit dialog. Later requests to that host are also sent as Codex. This depends on Figma's allowlist and can stop working if Figma tightens it.

The Strava remote MCP (`https://mcp.strava.com/mcp`) documents Claude Code as its HTTP client and rejects a generic registration. MyMCPs registers it as `Claude Code` and sends that client's User-Agent and MCP initialize identity on later requests. Strava uses the normal MyMCPs OAuth callback, so **Connect** returns to this app instead of the Figma paste step. Other MCP hosts are unchanged.

### Built-in MCPs

Some services have no MCP that a self-hosted gateway is allowed to use. MyMCPs implements those MCPs itself: the tools run inside your instance and reach the service with credentials that you create there and can revoke.

**Strava** signs in through an API application that you register. Choose **Strava** in the **Add MCP** gallery and follow the steps in the dialog: create an application at [strava.com/settings/api](https://www.strava.com/settings/api) with your instance's hostname as its **Authorization Callback Domain**, paste the Client ID and Client Secret, then select **Connect** to approve access. It is read-only unless you check **Allow write access**, which adds tools to create and edit activities. It does not depend on Strava's client allowlist. See [docs/strava.md](docs/strava.md) for the full walkthrough, the list of tools, and troubleshooting.

**iCloud Mail** signs in over IMAP and SMTP with an app-specific password, because Apple has no mail API. Choose **iCloud Mail** in the **Add MCP** gallery, create the password at [account.apple.com](https://account.apple.com/account/manage) under **Sign-In and Security → App-Specific Passwords**, and paste it with your iCloud Mail address. Apple cannot limit what that password reaches, so you choose the permissions in the same dialog and MyMCPs enforces them: **Read mail**, **Save drafts**, **Send mail**, and **Organize mail**. Only **Read mail** is allowed to begin with. Attachments are downloaded, and files to attach are uploaded, through temporary signed links, and you can list the aliases and custom domain addresses agents may send from. See [docs/icloud-mail.md](docs/icloud-mail.md) for the full walkthrough, the permissions, the list of tools, and troubleshooting.

**Google Ads** signs in through an OAuth client from your own Google Cloud project. Choose **Google Ads** in the **Add MCP** gallery and follow the steps in the dialog: enable the Google Ads API in a project, create a **Web application** OAuth client with `APP_URL/mcps/oauth/callback` as its redirect URI, paste the Client ID and Client Secret, then select **Connect**. Google retired developer tokens in September 2026, so there is none to enter: what the API reaches is the access level of the Cloud project. The MCP monitors accounts, campaigns, ads, keywords, and search terms, and with **Allow write access** builds and runs Search and Display campaigns, including image assets uploaded through temporary signed links. You can limit it to the accounts you list. The tools that set a budget, change bidding, or enable a campaign ask for your approval first. See [docs/google-ads.md](docs/google-ads.md) for the full walkthrough, the list of tools, access levels, and troubleshooting.

### Tool approvals

An agent that can call a tool can call it wrongly. For any MCP, built-in or connected, open its **⋯** menu on the **MCPs** page, choose **Tool approvals**, and set the tools that matter to **Asks**. A call to such a tool is not run: the agent gets a link to give you, and the call runs once you have signed in and approved it, when the agent makes it again with the same arguments. Administrators decide any request, and members the ones made with their own access tokens.

The page you approve on is written by MyMCPs from the call itself, never by the agent. For a built-in tool it says what would change, with the current value beside the new one: "Change the daily budget of the campaign "Spring sale" from €2.50 to €250.00". For a tool of a connected MCP it lists the exact arguments beside the description the MCP gives of its tool. An approval covers one call with those exact arguments, is used once, and expires after 24 hours. Open requests are listed under **Approvals**. See [docs/tool-approvals.md](docs/tool-approvals.md).

## How to deploy to my Coolify

This repository includes a production Docker image, a Compose service, and a `coolify.json` profile.

1. In Coolify, create a project and add a **Public Repository** resource.
2. Paste `https://github.com/0xtlt/MyMCPs` as the repository URL and select the branch you want to deploy.
3. Use **Docker Compose** as the build pack and `/docker-compose.yml` as the Compose file. Coolify may fill these settings from `coolify.json`.
4. Add a domain to the `mymcps` service and set `APP_URL` to the same HTTPS origin, for example `https://mcp.example.com`.
5. Deploy, open the domain, and complete onboarding.

The deployment exposes port `3333` and checks `/health`, and the server applies database migrations when it starts. The `mymcps-data` volume persists SQLite, encrypted secrets, the generated app key, and the sandboxes and installed packages of npm MCPs under `/app/tmp`. Files the server creates in the volume are readable only by the user it runs as, uid 1000. Coolify builds the image from source: the first deployment compiles the server and takes longer than the next ones, which reuse the build cache of the Docker host.

You do not need to create `APP_KEY` in Coolify. On the first start, the server generates a valid key, saves it to `/app/tmp/app.key`, and reuses it on every deploy. Back up the `mymcps-data` volume and do not rotate the key, or existing encrypted MCP credentials will become unreadable. To keep a copy of an instance without its volume, or to move it to another server, export an encrypted backup under **Settings** and import it on the setup screen of a new instance, whatever its key: see [docs/backup.md](docs/backup.md). In production the server refuses to start with a key published in this repository: the key the tests use, or the placeholder that images of earlier versions were built with.

The Coolify profile sets `TRUST_PROXY=loopback,uniquelocal`: the app accepts a forwarded client IP only from a proxy on the loopback interface or a private network, such as Coolify's proxy. Rate limits and logs then use the real client address, and a client cannot choose its own. `TRUST_PROXY` accepts `true`, `false`, or a comma-separated list of proxy IPs, CIDR ranges, and the names `loopback`, `linklocal`, and `uniquelocal`. If a CDN sits in front of the proxy, add the CDN's address ranges, or every client appears as a CDN edge address. A deployment created before this default changed keeps the `true` stored in its environment until you edit it.

## Useful commands

```bash
cargo run --bin mymcps                                 # Start the server with the settings in .env
cargo test --workspace                                 # Run all tests
cargo fmt --all --check                                # Check formatting
cargo clippy --workspace --all-targets -- -D warnings  # Lint and type-check
cargo build --release --bin mymcps                     # Create a production build: target/release/mymcps
```

Pull requests and pushes to `main` run the same format check, lint, tests, and production build in the **Quality** workflow.

### Reset a user password

Run this on the server from the application directory, using the instance's usual environment and database:

```sh
mymcps user:reset-password user@example.com
```

Enter and confirm the new password at the hidden prompts. Passwords must contain 8–32 characters and are never passed as command-line arguments. The command works for any account, whatever its teams and roles, without the old password, and exits with a nonzero status if the account does not exist or validation fails.

For Docker Compose (including Coolify), run:

```sh
docker compose exec mymcps mymcps user:reset-password user@example.com
```

The first `mymcps` names the Compose service and the second the server binary, which loads the persisted application key when needed. Successful resets end the account's browser sessions and revoke its remember-me tokens; MCP access tokens and OAuth connections are unchanged.

### Recover an account that lost its sign-in protection

Someone who lost their authenticator app, their passkeys and their recovery codes cannot sign in. A super admin lets them back in from **Users → ⋯ → Remove sign-in protection**: the account then signs in with its password alone, until its owner sets a protection up again.

When nobody can do that, the only super admin being the one locked out, run this on the server:

```sh
mymcps user:reset-protection user@example.com
```

It removes the authenticator app, the passkeys and the recovery codes of the account, and leaves its password as it is.

## Releases

GitHub Actions publishes releases without an AI or an external release service:

- **Nightly release** runs at 02:42 UTC and publishes a GitHub prerelease only when `main` has moved since the previous nightly. Its semantic prerelease tag includes the UTC date and commit, for example `v0.1.1-nightly.20260808.gabc1234`. It does not change the stable version in `Cargo.toml`.
- **Stable release** runs only when a repository maintainer starts it from **Actions → Stable release → Run workflow** and chooses a `patch`, `minor`, or `major` increment. It validates the application, updates the version in `Cargo.toml` and `Cargo.lock` and adds the release to `CHANGELOG.md`, commits the release, creates the stable tag, and publishes automatically generated GitHub release notes.

Both workflows use the repository-provided `GITHUB_TOKEN`; no release secret or AI service is required. Each one compiles the dependencies and runs the format check, lint, tests, the production build, and the dependency audit in a job whose token is read-only; a second job with write access then checks out that exact commit and tags and publishes it. The only code that job compiles is the release tool of this repository (`crates/xtask`), which has no dependencies. If a run fails after pushing a release commit or tag, rerun the same stable workflow to resume publication instead of incrementing the version again.

### Release container images

Each published release also produces a public OCI image for `linux/amd64` and `linux/arm64`:

- Stable releases: `ghcr.io/0xtlt/mymcps:stable` and the exact release tag, such as `ghcr.io/0xtlt/mymcps:v0.1.2`.
- Nightly releases: `ghcr.io/0xtlt/mymcps:nightly` and the exact prerelease tag, such as `ghcr.io/0xtlt/mymcps:v0.1.2-nightly.20260809.gabcdef0`.

Pull a channel image and preserve `/app/tmp`, which contains SQLite data, encrypted secrets, the generated app key, and the sandboxes and installed packages of npm MCPs:

```bash
docker pull ghcr.io/0xtlt/mymcps:stable
docker run --name mymcps -p 127.0.0.1:3333:3333 \
  -e APP_URL=https://mcp.example.com \
  -e TRUST_PROXY=loopback,uniquelocal \
  -e LOG_LEVEL=info \
  -v mymcps-data:/app/tmp \
  ghcr.io/0xtlt/mymcps:stable
```

The port is published on the loopback interface only. Put a TLS-terminating reverse proxy in front of it and set `APP_URL` to the HTTPS origin that proxy serves; the gateway OAuth endpoints stay disabled without an HTTPS `APP_URL`. Complete onboarding before the instance is reachable by anyone else, because the first account created becomes the administrator.

Use `ghcr.io/0xtlt/mymcps:nightly` in the same commands to test the nightly channel. The existing Docker Compose deployment continues to build from source.

After the first image publication, verify the package is anonymously pullable. If GHCR did not inherit the public repository visibility, open the package on GitHub, choose **Package settings → Change package visibility → Public**, and confirm the one-time change. No repository secret is required.

## npm MCPs and their sandbox

An npm MCP is a package of the npm registry that speaks MCP on its standard input and output. MyMCPs installs it with `npm` the first time it is used, in a directory of its own under the data directory, and starts it with [Node.js](https://nodejs.org/) for each request. The image holds both; a server run from source uses the `node` of its `PATH`, or the binary `NODE_BINARY` names, with the npm installed beside it.

A package is code written by someone else, and on an instance with open registration by anyone. The server therefore starts `npm` and the package in a sandbox of the Linux kernel, made of [Landlock](https://docs.kernel.org/userspace-api/landlock.html), seccomp and resource limits:

- A package reads the files it was installed with and the system's, and reads and writes one directory of its own. The database, the key, `.env` and the files of other MCPs are refused, whoever owns them.
- It starts no other process, sends a signal to none but itself, opens no Unix socket, and changes the mode or the owner of no file by its path.
- A process holds at most 2 GiB of memory and 4,096 open files, and cannot take the user the server runs as past 4,096 threads on the host.
- `npm` installs a package under the same sandbox, runs none of its scripts, and can write nothing but the directory it installs into.

Every sandboxed process reads the directories of the system, such as `/usr`. Keep the application and its data directory (`DATA_DIR`) out of them, as the image does with `/app`: a server installed under one starts no npm MCP and says why.

The network stays open: a package reaches whatever the server reaches, the server itself and its private network included. Restrict that at the level of the network when you let strangers add packages.

The sandbox needs no privilege and works in a container without capabilities. It needs Linux 5.13 or later with Landlock enabled, which the kernels of current distributions are (Ubuntu 22.04, Debian 12 and later). The server says what it found when it starts: `npm MCPs run in a sandbox`, with the version of Landlock in use, or `npm MCPs cannot start` and the reason. A server that cannot sandbox a package does not start it. `NPM_SANDBOX=off` starts npm MCPs without a sandbox, with every right of the server: that is for development on another system than Linux, not for an instance other people use.

## Stack

MyMCPs is one Rust binary: an [axum](https://github.com/tokio-rs/axum) server that renders its pages with [maud](https://maud.lambda.xyz/), stores its data in SQLite, speaks MCP with its own implementation of the protocol, and runs npm-based MCPs with [Node.js](https://nodejs.org/) in a sandbox of the kernel. Its pages need no JavaScript build. See [SECURITY.md](SECURITY.md) for the deployment security baseline and vulnerability reporting process.

## License

[MIT](LICENSE) © 2026 Thomas Tastet.
