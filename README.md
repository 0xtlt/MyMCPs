# MyMCPs

MyMCPs is a self-hosted [Model Context Protocol (MCP)](https://modelcontextprotocol.io/) gateway. Connect your AI client to one endpoint, then manage every upstream MCP, credential, and access token from one dashboard.

## What it does

- Connects HTTP and npm-based MCP servers.
- Supports bearer tokens, custom headers, and OAuth.
- Issues access tokens for all MCPs or a selected set.
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
2. Create a token from **Access tokens**.
3. Point your AI client to `https://your-domain.example/mcp`.
4. Send the token as `Authorization: Bearer <token>`.

Tools use `{mcp-slug}__{tool-name}` names by default. For clients with large tool catalogs, add the following request header to enable progressive discovery:

```text
X-MyMCPs-Tool-Mode: lazy
```

Lazy mode exposes `list_mcps`, `tool_search`, and `call_tool` instead of loading every tool definition at once.

### OAuth MCPs

For providers that support MCP OAuth discovery and dynamic client registration, choose **OAuth** when adding the server, save it, then select **Connect OAuth**. Set `APP_URL` to the instance's public HTTPS URL so callback URLs are generated correctly.

## How to deploy to my Coolify

This repository includes a production Docker image, a Compose service, and a `coolify.json` profile.

1. In Coolify, create a project and add a **Public Repository** resource.
2. Use [0xtlt/MyMCPs](https://github.com/0xtlt/MyMCPs) as the repository and select the branch you want to deploy.
3. Use **Docker Compose** as the build pack and `/docker-compose.yml` as the Compose file. Coolify may fill these settings from `coolify.json`.
4. Add a domain to the `mymcps` service and set `APP_URL` to the same HTTPS origin, for example `https://mcp.example.com`.
5. Deploy, open the domain, and complete onboarding.

The deployment exposes port `3333` and checks `/health`. The `mymcps-data` volume persists SQLite, encrypted secrets, the generated app key, and Deno sandbox data under `/app/tmp`.

`APP_KEY` is generated on the first start if you leave it empty. You can instead provide your own stable AdonisJS application key. Back up the persistent volume: losing it also loses the database and generated key.

If requests are logged with the proxy's IP, set `TRUST_PROXY` to the Coolify proxy's IP or CIDR.

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

UNLICENSED.
