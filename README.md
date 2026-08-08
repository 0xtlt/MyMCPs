# MyMCPs

MyMCPs is a self-hosted [Model Context Protocol (MCP)](https://modelcontextprotocol.io/) gateway. Connect your AI client to one endpoint, then manage every upstream MCP, credential, and access token from one dashboard.

## What it does

- Connects HTTP and npm-based MCP servers.
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

## Stack

MyMCPs uses AdonisJS 7, Inertia, React 19, SQLite, the MCP TypeScript SDK, and Deno. See [SECURITY.md](SECURITY.md) for the deployment security baseline and vulnerability reporting process.

## License

[MIT](LICENSE) © 2026 Thomas Tastet.
