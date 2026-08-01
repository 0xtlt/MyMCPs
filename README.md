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

## Quick start

Requirements: **Node.js ≥ 24**, **pnpm**.

```bash
pnpm install
cp .env.example .env   # if needed; scaffold may already have .env
pnpm run dev           # node ace serve --hmr
```

Open the app URL (default `http://localhost:3333`), complete onboarding, then sign in on later visits.

Migrations run as part of a fresh scaffold; for an existing DB:

```bash
node ace migration:run
```

## Stack

- [AdonisJS 7](https://adonisjs.com/) — backend, Lucid, session auth
- [Inertia](https://inertiajs.com/) + React 19 — server-driven UI
- [Astryx](https://astryx.atmeta.com/) — design system
- SQLite by default (`tmp/db.sqlite3`)

## Status

Early stage. The admin/onboarding/invite shell is in place. The MCP proxy surface (aggregating upstream MCPs behind one agent URL) is the next product focus.

## License

See repository license file when present. UNLICENSED until one is added.
