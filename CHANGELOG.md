# Changelog

Notable project changes are recorded here in English. Sections are organized by UTC merge date, newest first.

## 2026-08-28

### Added

- Added per-access-token debug sessions with start, pause, continue, and stop controls, an interactive call timeline, sanitized payload inspection, and input/output size metadata.

### Changed

- Replaced the compressed debug inspector panel with a focused, responsive call-details dialog that keeps the timeline visible at full width.

## 2026-08-18

### Removed

- Dropped Figma, Vercel, Canva, and Slack from the MCP template gallery because they only accept first-party clients such as Claude or Codex, not a custom MyMCPs OAuth configuration.

### Changed

- Upgraded the Astryx design system to 0.4.3, including info-banner painting under the neutral theme, tokenized focus rings, and NumberInput/Selector behavior.

## 2026-08-15

### Changed

- Released version [0.3.0](https://github.com/0xtlt/MyMCPs/releases/tag/v0.3.0).

## 2026-08-14

### Changed

- Upgraded the Inertia adapter to version 5 with Inertia v3, `@adonisjs/vite` 6, and Vite 8.
- Delivered success and error toasts through Inertia's flash bag instead of shared page props.

## 2026-08-13

### Added

- Added an Update MCP action for Deno npm MCPs that already track `latest`, which reloads the Deno package cache and retests the connection without changing pinned versions.
- Added instance settings to enable scheduled auto-updates of latest-tracking Deno npm MCPs, with a 5-field UTC cron expression that defaults to every day at 02:00.
- Added the Deno-cached npm package version in small type on the MCP list and edit form.

### Changed

- Released version [0.2.0](https://github.com/0xtlt/MyMCPs/releases/tag/v0.2.0).

- Hid the MCP edit form behind a centered spinner while Update MCP is running.
- Moved the Deno cached-version hint below the Version field so Extra args stays aligned.

### Fixed

- Rendered flash toasts into the open modal so they stay readable above the dialog backdrop.
- Reloaded Deno npm MCP caches with `--node-modules-dir=none` so Update MCP works inside this Node app, where Deno otherwise treats `package.json` as manual `node_modules` and refuses specifiers such as `@shopify/dev-mcp`.
- Allowed Deno npm MCP sandboxes to read the Deno package cache, so Node packages can load their own packaged files after a cache reload.

## 2026-08-12

### Changed

- Released version [0.1.3](https://github.com/0xtlt/MyMCPs/releases/tag/v0.1.3).

### Fixed

- Prevented stable release preparation from adding a blank line at the end of the changelog and failing generated-file validation.
- Hid the inactive access-token bulk selector when no rows can be deleted and restored spacing between the gateway details and token table.

### Added

- Access tokens page shows when each token was last used.

## 2026-08-12

### Added

- Added confirmed cleanup for selected or all expired and revoked access tokens, with a compact grouped mobile layout, neutral triggers, and a destructive final confirmation while preserving active tokens and historical activity labels.

## 2026-08-11

### Fixed

- Fixed OAuth discovery for MCP servers that publish a verified same-origin, path-based issuer while relying on legacy root metadata discovery.

## 2026-08-09

### Changed

- Released version [0.1.2](https://github.com/0xtlt/MyMCPs/releases/tag/v0.1.2).

### Added

- Added public `linux/amd64` and `linux/arm64` GHCR images for every successful stable and nightly release, with immutable release tags, channel tags, OCI metadata, pre-publish health validation, and cached builds.

## 2026-08-08

### Added

- Added a searchable, category-filtered gallery with 20 official MCP setups, branded cards, bottom-left-aligned actions, and prefilled HTTP or npm configuration.

- Added nightly prereleases that run only for new commits and a manually triggered stable release workflow that validates the application, increments its semantic version across package metadata and MCP handshakes, updates the changelog, tags the commit, and publishes generated GitHub release notes without an AI service.

### Changed

- Released version [0.1.1](https://github.com/0xtlt/MyMCPs/releases/tag/v0.1.1).

- Extended the typecheck command to validate repository `.mjs` release scripts with TypeScript's strict JavaScript checking.

### Fixed

- Fixed nightly and stable release validation failing when the settings browser test matched both a visible notification and its accessibility live region.

### Security

- Pinned the transitive Nano ID dependency to a patched release that prevents zero-length custom generators from looping indefinitely.

## 2026-08-07

### Added

- Added OAuth login for MCP clients, including discovery, dynamic client registration, PKCE authorization, consent, refresh-token rotation, connection revocation, and OAuth-managed access tokens ([#53](https://github.com/0xtlt/MyMCPs/pull/53)).
- Added an admin setting for the instance-wide default MCP tool discovery mode, with per-request header overrides.

### Changed

- Set the application version to 0.1.0 for the first tagged release.
- Made OAuth the recommended MCP installation method while retaining manual bearer-token configurations ([#53](https://github.com/0xtlt/MyMCPs/pull/53)).

### Fixed

- Allowed Deno npm MCP sandboxes to call `os.homedir()`, so Node-oriented packages such as `@shopify/dev-mcp` can start under the existing filesystem jail.
- Restored Cursor OAuth connections by accepting its exact native-app callback while continuing to reject unapproved custom URI schemes ([#56](https://github.com/0xtlt/MyMCPs/pull/56)).
- Standardized user-visible dates to day-first format while preserving local-time display and stored ISO values ([#54](https://github.com/0xtlt/MyMCPs/pull/54)).

### Security

- Hardened credential handling across MCP and OAuth redirects, callbacks, CORS, diagnostics, and logs; added safe outbound fetch behavior, login rate limiting, nonce-based CSP, persistent remember-me token revocation after password changes, and call-capture size limits ([#51](https://github.com/0xtlt/MyMCPs/pull/51)).
- Enforced HTTPS outside loopback development, constrained OAuth gateway boundaries, and added refresh-token reuse detection and grant-family revocation ([#53](https://github.com/0xtlt/MyMCPs/pull/53)).

## 2026-08-06

### Added

- Added custom, shareable analytics time ranges with exact timezone-aware intervals, start and end times, and a 365-day limit ([#50](https://github.com/0xtlt/MyMCPs/pull/50)).

### Fixed

- Hardened custom-range validation and fallbacks, preserved exact intervals across timezones and daylight-saving transitions, and restored spacing below log navigation ([#50](https://github.com/0xtlt/MyMCPs/pull/50)).

## 2026-08-05

### Changed

- Made the authenticated application responsive across mobile navigation, dashboard metrics, analytics, MCPs, access tokens, invitations, logs, settings, forms, and dialogs while preserving desktop tables ([#49](https://github.com/0xtlt/MyMCPs/pull/49)).

### Fixed

- Stacked email actions correctly on narrow screens and prevented horizontal overflow at supported mobile widths ([#49](https://github.com/0xtlt/MyMCPs/pull/49)).
