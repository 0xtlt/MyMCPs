# MyMCPs

Self-hosted **MCP gateway**: expose one MCP URL to your AI agents, and route tools/resources to many upstream MCPs behind it.

Agents talk to **one** endpoint. MyMCPs talks to the rest.

## Why

Running many MCP servers means many URLs, credentials, and agent configs. MyMCPs concentrates them into a single gateway you host yourself—on your laptop, a VPS, or your infra.

This is **not** a public SaaS signup product. You install an instance, complete first-run onboarding, and own the deployment.

## Access model

1. **Fresh install** — any URL redirects to `/onboarding` to create the admin (no public marketing home)
2. **Afterwards** — `/` is the signed-in dashboard; guests are sent to `/login`
3. **Teammates** — invite-only (admin creates a link; no email sending required)
4. **Agents** — use an access token as `Authorization: Bearer …` against `POST/GET /mcp`

## Lazy tool discovery

The gateway keeps its existing eager behavior by default: `tools/list` returns
every allowed upstream tool as `{slug}__{toolName}`. Clients with large tool
catalogs can opt into MyMCPs progressive discovery by adding this static request
header to their MCP configuration:

```text
X-MyMCPs-Tool-Mode: lazy
```

Lazy mode shares the access token's allowed MCP catalog during initialization
as a concise `slug: description` list, with instructions to refresh it using
`list_mcps` and discover tools using `tool_search`. It exposes only three stable
tools:

1. `list_mcps` returns the current allowed MCP names, slugs, descriptions, and statuses.
2. `tool_search` searches one selected MCP and returns matching tool definitions.
3. `call_tool` invokes an exact tool using its MCP slug, name, and schema-compliant arguments.

Example workflow:

```json
{ "name": "tool_search", "arguments": { "mcp": "github", "query": "create issue" } }
{ "name": "call_tool", "arguments": { "mcp": "github", "tool": "create_issue", "arguments": { "title": "Example" } } }
```

This `tool_search` is a portable MyMCPs MCP tool. It does not enable or replace
a host's reserved native tool-search feature; Codex, Cursor, and other hosts
control their own model-facing tool deferral independently. Use
`X-MyMCPs-Tool-Mode: eager` to select the original behavior explicitly. Other
header values are rejected with HTTP 400 so configuration mistakes are visible.

## Quick start

Requirements: **Node.js ≥ 24**, **pnpm**. For **npm-transport MCPs**, also install [Deno](https://deno.land/). npm packages run in a Deno subprocess with filesystem access limited to a per-MCP sandbox directory (they cannot read the Adonis SQLite DB, `.env`, or app source). Network and env remain allowed so typical MCP packages can call APIs—treat installed packages as trusted software.

```bash
pnpm install
cp .env.example .env   # if needed; scaffold may already have .env
pnpm run dev           # node ace serve --hmr
```

Optional: set `DENO_PATH` if `deno` is not on your `PATH`.

Open the app URL (default `http://localhost:3333`), complete onboarding, then sign in on later visits.

Migrations run as part of a fresh scaffold; for an existing DB:

```bash
node ace migration:run
```

## Codex worktrees

When creating a worktree in the Codex desktop app, select the **MyMCPs** local
environment. Codex will install the locked pnpm dependencies, create an isolated
`.env` with a generated `APP_KEY`, and migrate a fresh SQLite database. The
environment also adds **Dev** and **Tests** actions to the Codex toolbar.

## One-click OAuth MCPs

For a remote OAuth MCP such as Notion, add an HTTP MCP with this URL:
`https://mcp.notion.com/mcp`. Select **OAuth**, save it, and click **Connect
OAuth**. MyMCPs discovers the MCP's OAuth metadata, registers a client for the
current callback URL, opens the provider's login, and stores the resulting
tokens. You do not need to enter authorize/token endpoints, a client ID, or
register a callback URI manually.

The provider must support MCP OAuth discovery and dynamic client registration.
Providers without those capabilities can use the optional advanced OAuth
overrides in the MCP form. In production, `APP_URL` must be the public HTTPS
origin of the MyMCPs instance so gateway URLs, invite links, and OAuth callbacks
use the same host. MyMCPs does not infer this value from request headers.

## Tests

The project uses native AdonisJS test suites powered by Japa:

```bash
pnpm test
pnpm test unit
pnpm test functional
pnpm test browser
```

Tests use `tmp/test.sqlite3`, which is isolated from the development database at `tmp/db.sqlite3` and reset for each test through a rolled-back database transaction. Browser tests run headless Chromium through Playwright. After installing dependencies on a new machine, install the browser once:

```bash
pnpm exec playwright install chromium
```

Add reusable records through the helpers in `tests/helpers/factories.ts`, and place fast logic tests in `tests/unit`, HTTP tests in `tests/functional`, and end-to-end UI tests in `tests/browser`.

## Coolify

The repository includes a Coolify deployment profile. To deploy a public
repository:

1. Create a new resource from **Public Repository** and paste the GitHub
   repository URL.
2. Use the checked-in `coolify.json` configuration, assign the domain that
   should serve MyMCPs, and set `APP_URL` to that public HTTPS origin (for
   example, `https://mcp.example.com`).
3. Deploy the resource.

The profile selects the Compose build pack, exposes the internal port `3333`,
configures `/health`, and keeps SQLite, generated key material, and Deno
sandboxes in the named `/app/tmp` volume. `APP_URL` is intentionally explicit:
when it is missing, signed-in pages show a warning and public-link or OAuth
controls stay disabled. The container generates an `APP_KEY` on first start
when one is not provided; the key is stored in the persistent volume. Set an
explicit `APP_KEY` in Coolify if you prefer to manage it yourself.

If the Coolify instance does not automatically apply `coolify.json`, select
**Docker Compose**, set the Compose file to `/docker-compose.yml`, expose port
`3333`, and add the same domain manually.

## Docker

The image bundles **Node.js 24**, production deps, and **Deno** (for npm MCP sandboxes). Persist `/app/tmp` so SQLite and sandbox caches survive restarts.

```bash
cp .env.example .env
# APP_KEY is generated and persisted by the container when omitted.
# For a direct production deployment, set APP_URL to the public HTTPS URL.
docker compose \
  -f docker-compose.yml \
  -f docker-compose.local.yml \
  up --build -d
```

The optional local Compose override binds the published port to `127.0.0.1` by
default. If the container must be reachable outside the host, set
`BIND_ADDRESS` only behind a TLS-terminating reverse proxy and use an HTTPS
`APP_URL`. Coolify uses its own proxy and the base `docker-compose.yml` does not
publish a host port.

Or without Compose:

```bash
docker build -t mymcps .
docker run --rm -p 3333:3333 \
  -e APP_KEY=... \
  -e APP_URL=https://mcp.example.com \
  -v mymcps-data:/app/tmp \
  mymcps
```

`DENO_PATH` defaults to `/usr/local/bin/deno` inside the container.

## Product surfaces

- **MCPs** — register HTTP (Streamable HTTP) or npm upstreams with none / bearer / header / OAuth auth
- **Access tokens** — identifiers scoped to all MCPs (auto-includes new ones) or a selected list; optional expiry
- **Gateway** — `/mcp` aggregates namespaced tools (`{slug}__{toolName}`) or provides opt-in lazy discovery for allowed upstreams

## Stack

- [AdonisJS 7](https://adonisjs.com/) — backend, Lucid, session auth
- [Inertia](https://inertiajs.com/) + React 19 — server-driven UI
- [Astryx](https://astryx.atmeta.com/) — design system
- SQLite by default (`tmp/db.sqlite3`)
- [@modelcontextprotocol/sdk](https://github.com/modelcontextprotocol/typescript-sdk) — client/server + Streamable HTTP
- Deno — sandboxed filesystem runner for npm MCPs (network/env still permitted)

## License

See repository license file when present. UNLICENSED until one is added.
