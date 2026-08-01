# AGENTS.md

Project-specific guidance for AI coding agents.

<!-- ASTRYX:START -->
Astryx v0.2.0 · 154 components
CLI: run every command as `pnpm exec astryx <cmd>` (shown below as `astryx ...`).

SETUP (once, in your app entry e.g. main.tsx) — without these, components render unstyled:
  import "@astryxdesign/core/reset.css";
  import "@astryxdesign/core/astryx.css";

WORKFLOW — discover, don't guess. Before writing UI:
1. `astryx build "<idea>"` — START HERE: returns a kit (closest [page] + [block]s + [component]s). No args = full playbook.
2. `astryx template <name> [--skeleton]` — scaffold the [page]/[block]s it named, or study their layout. Templates are reference code.
3. `astryx component <Name>` — props + examples for every component you use.

RULES:
- No <div> — components do all layout/spacing. Full page → AppShell; sidebar nav → SideNav.
- Frame first: pick the shell (AppShell / Layout+LayoutPanel) and budget regions in px BEFORE writing content (`astryx docs layout`).
- Dense data = rows (Table, List/Item) edge-to-edge — never Card-wrapped list items. Card = dashboard widgets, galleries, settings groups only.
- Status → StatusDot/Token; Badge only for counts and enumerated states, never decoration.
- Custom styling: component props first; else style/className with tokens — var(--color-*|--spacing-*|--radius-*). No raw hex/px. (No StyleX/Tailwind compiler here — don't use xstyle/utility classes.)
- Tokens for every value (`astryx docs tokens`). Brand/accent via `astryx theme` — never override --color-* in :root.
- SELF-CHECK before you finish: re-read the file and replace any raw <div>/<span> layout, imported .css/@apply, or hardcoded value (#hex, 16px) with the component or a token (var(--color-*|--spacing-*|…)). If unsure a component/prop exists, run `astryx component <Name>` / `astryx search "<thing>"`; don't hand-roll CSS.

MORE CLI:
  search "<query>"   find any component / hook / doc / template / block
  component --list   154 components by category
  template --list    page + block recipes
  docs <topic>       color, elevation, icons, illustrations, internationalization, layout, migration, motion, principles, shape, spacing, styling, theme, tokens, typography
  swizzle <Name>     eject component source for deep customization
  upgrade --apply    run after any @astryxdesign/core bump
<!-- ASTRYX:END -->

## Cursor Cloud specific instructions

Single AdonisJS 7 + Inertia/React app (MyMCPs). No Docker, Redis, or external DB — SQLite via `better-sqlite3` at `tmp/db.sqlite3`.

### Runtime
- **Node.js ≥ 24** and **pnpm** are required (`package.json` `engines`). Use nvm Node 24; put it first on `PATH` (the VM may also ship an older `/exec-daemon/node`).
- Standard commands: see `README.md` and `package.json` scripts (`pnpm run dev`, `lint`, `typecheck`, `test`, `build`).
- Dev server: `pnpm run dev` → `node ace serve --hmr` on **PORT 3333**. For cloud VMs bind with `HOST=0.0.0.0` in `.env` while keeping `APP_URL=http://localhost:3333` for redirects/cookies.
- One-time (or empty DB): copy `.env.example` → `.env`, `node ace generate:key`, then `node ace migration:run`. Do not commit `.env`.

### pnpm gotchas
- `pnpm-workspace.yaml` must approve builds for `@astryxdesign/cli`, `@astryxdesign/core`, `@swc/core`, `better-sqlite3`, and `esbuild`. Placeholder `allowBuilds` values cause `ERR_PNPM_IGNORED_BUILDS` and make every `pnpm run *` fail before the script runs.
- Fresh lockfile packages can trip pnpm’s `minimumReleaseAge` check; the update script passes `--config.minimumReleaseAge=0`. If a manual `pnpm install` fails the same way, use that flag.

### Lint / typecheck / test today
- `pnpm test` boots Japa but has **no specs yet** (`NO TESTS EXECUTED`).
- `pnpm run lint` and `pnpm run typecheck` currently report **pre-existing** Prettier/Inertia/Astryx typing issues; they are not environment setup failures. `pnpm run build` still succeeds.
