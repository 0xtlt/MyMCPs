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

## Docker

The image bundles **Node.js 24**, production deps, and **Deno** (for npm MCP sandboxes). Persist `/app/tmp` so SQLite and sandbox caches survive restarts.

```bash
cp .env.example .env
# set APP_KEY (e.g. node ace generate:key) and APP_URL to your public URL
docker compose up --build -d
```

Compose binds the published port to `127.0.0.1` by default. If the container
must be reachable outside the host, set `BIND_ADDRESS` only behind a
TLS-terminating reverse proxy and use an HTTPS `APP_URL`.

Or without Compose:

```bash
docker build -t mymcps .
docker run --rm -p 3333:3333 \
  -e APP_KEY=... \
  -e APP_URL=http://localhost:3333 \
  -v mymcps-data:/app/tmp \
  mymcps
```

`DENO_PATH` defaults to `/usr/local/bin/deno` inside the container.

## Product surfaces

- **MCPs** — register HTTP (Streamable HTTP) or npm upstreams with none / bearer / header / OAuth auth
- **Access tokens** — identifiers scoped to all MCPs (auto-includes new ones) or a selected list; optional expiry
- **Gateway** — `/mcp` aggregates namespaced tools (`{slug}__{toolName}`) for allowed upstreams

## Stack

- [AdonisJS 7](https://adonisjs.com/) — backend, Lucid, session auth
- [Inertia](https://inertiajs.com/) + React 19 — server-driven UI
- [Astryx](https://astryx.atmeta.com/) — design system
- SQLite by default (`tmp/db.sqlite3`)
- [@modelcontextprotocol/sdk](https://github.com/modelcontextprotocol/typescript-sdk) — client/server + Streamable HTTP
- Deno — sandboxed filesystem runner for npm MCPs (network/env still permitted)

## License

See repository license file when present. UNLICENSED until one is added.
