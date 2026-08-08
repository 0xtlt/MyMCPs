# Changelog

Notable project changes are recorded here in English. Sections are organized by UTC merge date, newest first.

## 2026-08-08

### Added

- Added nightly prereleases that run only for new commits and a manually triggered stable release workflow that validates the application, increments its semantic version, updates the changelog, tags the commit, and publishes generated GitHub release notes without an AI service.

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
