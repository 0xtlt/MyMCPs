# MyMCPs

MyMCPs is a self-hosted [Model Context Protocol (MCP)](https://modelcontextprotocol.io/) gateway. Connect your AI client to one endpoint, then manage every upstream MCP, credential, and access token from one dashboard.

## What it does

- Connects HTTP and npm-based MCP servers.
- Includes built-in MCPs for services with no MCP a self-hosted gateway can use: Strava and iCloud Mail.
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

**iCloud Mail** signs in over IMAP and SMTP with an app-specific password, because Apple has no mail API. Choose **iCloud Mail** in the **Add MCP** gallery, create the password at [account.apple.com](https://account.apple.com/account/manage) under **Sign-In and Security → App-Specific Passwords**, and paste it with your iCloud Mail address. Apple cannot limit what that password reaches, so you choose the permissions in the same dialog and MyMCPs enforces them: **Read mail**, **Save drafts**, **Send mail**, and **Organize mail**. Only **Read mail** is allowed to begin with. Attachments are downloaded through temporary signed links, and you can list the aliases and custom domain addresses agents may send from. See [docs/icloud-mail.md](docs/icloud-mail.md) for the full walkthrough, the permissions, the list of tools, and troubleshooting.

## How to deploy to my Coolify

This repository includes a production Docker image, a Compose service, and a `coolify.json` profile.

1. In Coolify, create a project and add a **Public Repository** resource.
2. Paste `https://github.com/0xtlt/MyMCPs` as the repository URL and select the branch you want to deploy.
3. Use **Docker Compose** as the build pack and `/docker-compose.yml` as the Compose file. Coolify may fill these settings from `coolify.json`.
4. Add a domain to the `mymcps` service and set `APP_URL` to the same HTTPS origin, for example `https://mcp.example.com`.
5. Deploy, open the domain, and complete onboarding.

The deployment exposes port `3333`, checks `/health`, and runs database migrations before the app starts. The `mymcps-data` volume persists SQLite, encrypted secrets, the generated app key, and Deno sandbox data under `/app/tmp`.

`APP_KEY` is required, but you do not need to create it in Coolify. On the first start, the container generates a valid key, saves it to `/app/tmp/app.key`, and reuses it on every deploy. Back up the `mymcps-data` volume and do not rotate the key, or existing encrypted MCP credentials will become unreadable.

The Coolify profile sets `TRUST_PROXY=true` so logs use the forwarded client IP instead of the proxy's IP.

## Useful commands

```bash
pnpm run dev        # Start the development server
pnpm test           # Run all tests
pnpm run lint       # Check code style
pnpm run typecheck  # Check TypeScript
pnpm run build      # Create a production build
```

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

The entrypoint loads the persisted application key when needed. Successful resets revoke the account's remember-me tokens. Existing browser sessions can remain active until they expire; MCP access tokens and OAuth connections are unchanged.

## Releases

GitHub Actions publishes releases without an AI or an external release service:

- **Nightly release** runs at 02:42 UTC and publishes a GitHub prerelease only when `main` has moved since the previous nightly. Its semantic prerelease tag includes the UTC date and commit, for example `v0.1.1-nightly.20260808.gabc1234`. It does not change the stable version in `package.json`.
- **Stable release** runs only when a repository maintainer starts it from **Actions → Stable release → Run workflow** and chooses a `patch`, `minor`, or `major` increment. It validates the application, updates `package.json` and `CHANGELOG.md`, commits the release, creates the stable tag, and publishes automatically generated GitHub release notes.

Both workflows use the repository-provided `GITHUB_TOKEN`; no release secret or AI service is required. If a run fails after pushing a release commit or tag, rerun the same stable workflow to resume publication instead of incrementing the version again.

### Release container images

Each published release also produces a public OCI image for `linux/amd64` and `linux/arm64`:

- Stable releases: `ghcr.io/0xtlt/mymcps:stable` and the exact release tag, such as `ghcr.io/0xtlt/mymcps:v0.1.2`.
- Nightly releases: `ghcr.io/0xtlt/mymcps:nightly` and the exact prerelease tag, such as `ghcr.io/0xtlt/mymcps:v0.1.2-nightly.20260809.gabcdef0`.

Pull a channel image and preserve `/app/tmp`, which contains SQLite data, encrypted secrets, the generated app key, and Deno sandbox data:

```bash
docker pull ghcr.io/0xtlt/mymcps:stable
docker run --name mymcps -p 3333:3333 \
  -e APP_URL=http://localhost:3333 \
  -e LOG_LEVEL=info \
  -e SESSION_DRIVER=cookie \
  -v mymcps-data:/app/tmp \
  ghcr.io/0xtlt/mymcps:stable
```

Use `ghcr.io/0xtlt/mymcps:nightly` in the same commands to test the nightly channel. The existing Docker Compose deployment continues to build from source.

After the first image publication, verify the package is anonymously pullable. If GHCR did not inherit the public repository visibility, open the package on GitHub, choose **Package settings → Change package visibility → Public**, and confirm the one-time change. No repository secret is required.

## Stack

MyMCPs uses AdonisJS 7, Inertia, React 19, SQLite, the MCP TypeScript SDK, and Deno. See [SECURITY.md](SECURITY.md) for the deployment security baseline and vulnerability reporting process.

## License

[MIT](LICENSE) © 2026 Thomas Tastet.
