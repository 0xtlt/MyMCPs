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

MyMCPs is self-hosted and invite-only. The first user becomes the administrator during onboarding.

## Run locally

You need [Node.js 24 or newer](https://nodejs.org/), [pnpm](https://pnpm.io/), and [Deno](https://deno.com/) if you want to run npm-based MCPs.

```bash
pnpm install
cp .env.example .env
node ace generate:key
node ace migration:run
pnpm run dev
```

Open [http://localhost:3333](http://localhost:3333) and create the admin account.

## Connect an AI client

1. Add your upstream servers from **MCPs**.
2. Open **Access tokens**, select **Install MCP**, and copy the OAuth configuration for your client.
3. When the client opens MyMCPs, sign in and approve the connection.
4. Revoke the generated OAuth connection from **Access tokens** when you want to stop it.

OAuth clients connect to `https://your-domain.example/mcp` without a manually copied token. Set
`APP_URL` to the instance's public HTTPS origin so discovery and authorization URLs are correct.

For clients without OAuth support, create a manual token and send it as
`Authorization: Bearer <token>` on every `/mcp` request.

Tools use `{mcp-slug}__{tool-name}` names in eager mode. Administrators can choose the instance default under **Settings → My Instance**; existing instances default to eager mode. Clients can override that default per request with either `eager` or `lazy`:

```text
X-MyMCPs-Tool-Mode: lazy
```

Lazy mode exposes `list_mcps`, `tool_search`, and `call_tool` instead of loading every tool definition at once. The header takes precedence over the instance setting.

### Upstream OAuth MCPs

For providers that support MCP OAuth discovery and dynamic client registration, choose **OAuth** when adding the server, save it, then select **Connect OAuth**. Set `APP_URL` to the instance's public HTTPS URL so callback URLs are generated correctly.

The Figma remote MCP (`https://mcp.figma.com/mcp`) only registers client names on its first-party allowlist, and only with a localhost redirect. MyMCPs detects that URL and registers as `Codex` there automatically. **Connect** opens Figma in a new tab; after you approve access, that tab lands on a `http://localhost:…/callback?code=…` address that fails to load. Copy it from the address bar and paste it into **Callback address** in the MCP's edit dialog. Later requests to that host are also sent as Codex. This depends on Figma's allowlist and can stop working if Figma tightens it.

The Strava remote MCP (`https://mcp.strava.com/mcp`) documents Claude Code as its HTTP client and rejects a generic registration. MyMCPs registers it as `Claude Code` and sends that client's User-Agent and MCP initialize identity on later requests. Strava uses the normal MyMCPs OAuth callback, so **Connect** returns to this app instead of the Figma paste step. Other MCP hosts are unchanged.

### Built-in MCPs

Some services have no MCP that a self-hosted gateway is allowed to use. MyMCPs implements those MCPs itself: the tools run inside your instance and reach the service with credentials that you create there and can revoke.

**Strava** signs in through an API application that you register. Choose **Strava** in the **Add MCP** gallery and follow the steps in the dialog: create an application at [strava.com/settings/api](https://www.strava.com/settings/api) with your instance's hostname as its **Authorization Callback Domain**, paste the Client ID and Client Secret, then select **Connect** to approve access. It is read-only unless you check **Allow write access**, which adds tools to create and edit activities. It does not depend on Strava's client allowlist. See [docs/strava.md](docs/strava.md) for the full walkthrough, the list of tools, and troubleshooting.

**iCloud Mail** signs in over IMAP and SMTP with an app-specific password, because Apple has no mail API. Choose **iCloud Mail** in the **Add MCP** gallery, create the password at [account.apple.com](https://account.apple.com/account/manage) under **Sign-In and Security → App-Specific Passwords**, and paste it with your iCloud Mail address. Apple cannot limit what that password reaches, so you choose the permissions in the same dialog and MyMCPs enforces them: **Read mail**, **Save drafts**, **Send mail**, and **Organize mail**. Only **Read mail** is allowed to begin with. Attachments are downloaded, and files to attach are uploaded, through temporary signed links, and you can list the aliases and custom domain addresses agents may send from. See [docs/icloud-mail.md](docs/icloud-mail.md) for the full walkthrough, the permissions, the list of tools, and troubleshooting.

**Google Ads** signs in through an OAuth client from your own Google Cloud project. Choose **Google Ads** in the **Add MCP** gallery and follow the steps in the dialog: enable the Google Ads API in a project, create a **Web application** OAuth client with `APP_URL/mcps/oauth/callback` as its redirect URI, paste the Client ID and Client Secret, then select **Connect**. Google retired developer tokens in September 2026, so there is none to enter: what the API reaches is the access level of the Cloud project. The MCP monitors accounts, campaigns, ads, keywords, and search terms, and with **Allow write access** builds and runs Search and Display campaigns, including image assets uploaded through temporary signed links. You can limit it to the accounts you list. The tools that set a budget, change bidding, or enable a campaign ask for your approval first. See [docs/google-ads.md](docs/google-ads.md) for the full walkthrough, the list of tools, access levels, and troubleshooting.

### Tool approvals

An agent that can call a tool can call it wrongly. For any MCP, built-in or connected, open **Edit → Tool approvals** and set the tools that matter to **Asks**. A call to such a tool is not run: the agent gets a link to give you, and the call runs once you have signed in and approved it, when the agent makes it again with the same arguments. Administrators decide any request, and members the ones made with their own access tokens.

The page you approve on is written by MyMCPs from the call itself, never by the agent. For a built-in tool it says what would change, with the current value beside the new one: "Change the daily budget of the campaign "Spring sale" from €2.50 to €250.00". For a tool of a connected MCP it lists the exact arguments beside the description the MCP gives of its tool. An approval covers one call with those exact arguments, is used once, and expires after 24 hours. Open requests are listed under **Approvals**. See [docs/tool-approvals.md](docs/tool-approvals.md).

## How to deploy to my Coolify

This repository includes a production Docker image, a Compose service, and a `coolify.json` profile.

1. In Coolify, create a project and add a **Public Repository** resource.
2. Paste `https://github.com/0xtlt/MyMCPs` as the repository URL and select the branch you want to deploy.
3. Use **Docker Compose** as the build pack and `/docker-compose.yml` as the Compose file. Coolify may fill these settings from `coolify.json`.
4. Add a domain to the `mymcps` service and set `APP_URL` to the same HTTPS origin, for example `https://mcp.example.com`.
5. Deploy, open the domain, and complete onboarding.

The deployment exposes port `3333`, checks `/health`, and runs database migrations before the app starts. The `mymcps-data` volume persists SQLite, encrypted secrets, the generated app key, and Deno sandbox data under `/app/tmp`. Files the container creates in the volume are readable only by its `node` user.

`APP_KEY` is required, but you do not need to create it in Coolify. On the first start, the container generates a valid key, saves it to `/app/tmp/app.key`, and reuses it on every deploy. Back up the `mymcps-data` volume and do not rotate the key, or existing encrypted MCP credentials will become unreadable. In production the server refuses to start with either key published in this repository: the Docker build placeholder or the test key in `.env.test`.

The Coolify profile sets `TRUST_PROXY=loopback,uniquelocal`: the app accepts a forwarded client IP only from a proxy on the loopback interface or a private network, such as Coolify's proxy. Rate limits and logs then use the real client address, and a client cannot choose its own. `TRUST_PROXY` accepts `true`, `false`, or a comma-separated list of proxy IPs, CIDR ranges, and the names `loopback`, `linklocal`, and `uniquelocal`. If a CDN sits in front of the proxy, add the CDN's address ranges, or every client appears as a CDN edge address. A deployment created before this default changed keeps the `true` stored in its environment until you edit it.

## Useful commands

```bash
pnpm run dev        # Start the development server
pnpm test           # Run all tests
pnpm run lint       # Check code style
pnpm run typecheck  # Check TypeScript
pnpm run build      # Create a production build
```

Pull requests and pushes to `main` run the same lint, typecheck, and test suites in the **Quality** workflow.

### Reset a user password

Run this on the server from the application directory, using the instance's usual environment and database:

```sh
node ace user:reset-password user@example.com
```

Enter and confirm the new password at the hidden prompts. Passwords must contain 8–32 characters and are never passed as command-line arguments. The command works for administrator and member accounts without the old password, and exits with a nonzero status if the account does not exist or validation fails.

For Docker Compose (including Coolify), run:

```sh
docker compose exec mymcps /app/docker-entrypoint.sh node ace user:reset-password user@example.com
```

The entrypoint loads the persisted application key when needed. Successful resets end the account's browser sessions and revoke its remember-me tokens; MCP access tokens and OAuth connections are unchanged.

## Releases

GitHub Actions publishes releases without an AI or an external release service:

- **Nightly release** runs at 02:42 UTC and publishes a GitHub prerelease only when `main` has moved since the previous nightly. Its semantic prerelease tag includes the UTC date and commit, for example `v0.1.1-nightly.20260808.gabc1234`. It does not change the stable version in `package.json`.
- **Stable release** runs only when a repository maintainer starts it from **Actions → Stable release → Run workflow** and chooses a `patch`, `minor`, or `major` increment. It validates the application, updates `package.json` and `CHANGELOG.md`, commits the release, creates the stable tag, and publishes automatically generated GitHub release notes.

Both workflows use the repository-provided `GITHUB_TOKEN`; no release secret or AI service is required. Each one installs dependencies and runs lint, typecheck, tests, the build, and the audit in a job whose token is read-only; a second job with write access then checks out that exact commit and tags and publishes it without installing anything. If a run fails after pushing a release commit or tag, rerun the same stable workflow to resume publication instead of incrementing the version again.

### Release container images

Each published release also produces a public OCI image for `linux/amd64` and `linux/arm64`:

- Stable releases: `ghcr.io/0xtlt/mymcps:stable` and the exact release tag, such as `ghcr.io/0xtlt/mymcps:v0.1.2`.
- Nightly releases: `ghcr.io/0xtlt/mymcps:nightly` and the exact prerelease tag, such as `ghcr.io/0xtlt/mymcps:v0.1.2-nightly.20260809.gabcdef0`.

Pull a channel image and preserve `/app/tmp`, which contains SQLite data, encrypted secrets, the generated app key, and Deno sandbox data:

```bash
docker pull ghcr.io/0xtlt/mymcps:stable
docker run --name mymcps -p 127.0.0.1:3333:3333 \
  -e APP_URL=https://mcp.example.com \
  -e TRUST_PROXY=loopback,uniquelocal \
  -e LOG_LEVEL=info \
  -e SESSION_DRIVER=cookie \
  -v mymcps-data:/app/tmp \
  ghcr.io/0xtlt/mymcps:stable
```

The port is published on the loopback interface only. Put a TLS-terminating reverse proxy in front of it and set `APP_URL` to the HTTPS origin that proxy serves; the gateway OAuth endpoints stay disabled without an HTTPS `APP_URL`. Complete onboarding before the instance is reachable by anyone else, because the first account created becomes the administrator.

Use `ghcr.io/0xtlt/mymcps:nightly` in the same commands to test the nightly channel. The existing Docker Compose deployment continues to build from source.

After the first image publication, verify the package is anonymously pullable. If GHCR did not inherit the public repository visibility, open the package on GitHub, choose **Package settings → Change package visibility → Public**, and confirm the one-time change. No repository secret is required.

## Stack

MyMCPs uses AdonisJS 7, Inertia, React 19, SQLite, the MCP TypeScript SDK, and Deno. See [SECURITY.md](SECURITY.md) for the deployment security baseline and vulnerability reporting process.

## License

[MIT](LICENSE) © 2026 Thomas Tastet.
