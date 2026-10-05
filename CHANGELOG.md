# Changelog

Notable project changes are recorded here in English. Sections are organized by UTC merge date, newest first.

## 2026-10-05

### Changed

- Built-in MCP tools now check all their arguments before anything else: `list_routes` refuses wrong page arguments without calling Strava, and `update_activity` reports a missing `activity_id` before asking for a field to change.

### Fixed

- Report a Strava error response with an unexpected shape as a plain HTTP error instead of failing the tool call.

## 2026-10-04

### Added

- Added built-in MCPs: MCP servers that MyMCPs runs itself for services whose own MCP is closed to self-hosted gateways. They authorize through an API application the admin registers with the service, and their Client Secret and tokens are encrypted like other MCP credentials.
- Added a built-in Strava MCP with 17 read tools for the athlete profile, training totals, zones, activities, activity streams, segments, routes, clubs, and gear. It is read-only by default; **Allow write access** adds 4 tools to create manual activities, edit activity details, update the athlete's weight, and star segments. The **Add MCP** dialog walks through creating the Strava API application, shows the Website and Authorization Callback Domain to enter, and offers a ready-made application icon to upload; [docs/strava.md](docs/strava.md) has the full guide.
- Added a built-in iCloud Mail MCP that reaches the mailbox over IMAP and SMTP with an app-specific password, which is encrypted like other MCP credentials. It has 8 tools to list mailboxes, list, search, and read messages, link to their attachments, save drafts, send plain text mail and replies, mark messages, and move them between mailboxes. The **Add MCP** dialog walks through creating the password at Apple and checks it by signing in when the MCP is saved; [docs/icloud-mail.md](docs/icloud-mail.md) has the full guide. The instance needs outbound access to `imap.mail.me.com:993` and `smtp.mail.me.com:587`.
- Added permissions to built-in MCPs whose provider cannot restrict the sign-in. Apple does not scope an app-specific password, so the admin chooses what agents may do when adding iCloud Mail: **Read mail**, **Save drafts**, **Send mail**, and **Organize mail**, with only reading allowed by default. MyMCPs enforces them for every access token: tools outside the allowed permissions are not listed and are refused when called.
- Added temporary attachment links to the iCloud Mail MCP. `get_message` lists the attachments of a message and `get_attachment_link` returns a signed link to download one from the instance, valid for 15 minutes by default and 60 at most. The link needs no sign-in, cannot be altered, only downloads, and stops working when the MCP is disabled or loses **Read mail**. It requires `APP_URL`, and downloads are limited to 60 per 15 minutes for each client address.
- Added sender addresses to the iCloud Mail MCP. The admin can list the account's aliases, custom domain addresses, and Hide My Email addresses, and agents can then send or draft from them with `from`. Replies are sent from the address the original message was written to.

### Changed

- Updated application and development dependencies and refreshed the lockfile, including AdonisJS core 7.5.2, the MCP SDK 1.32.0, Inertia 3.8.0, Vite 8.3.2, and the Astryx design system 0.6.5.
- Changed the proxy trust default in Docker Compose and Coolify from `TRUST_PROXY=true` to `loopback,uniquelocal`, so a client can no longer choose the address that rate limits and call logs record. `TRUST_PROXY` now accepts a comma-separated list of proxy IPs, CIDR ranges, and the names `loopback`, `linklocal`, and `uniquelocal`. A deployment that already stores `true` keeps it until you edit it; behind a CDN, add the CDN's address ranges.
- All npm MCPs now share one Deno cache that packages can read but not write, so **Update MCP** and scheduled updates change the version that actually runs. Each package is downloaded again once after the upgrade. An MCP's sandbox directory is deleted with the MCP and when its package changes, and a failed start reports Deno's own output.
- The gateway contacts upstream MCPs only to list tools and to call the one in use, instead of connecting to every allowed MCP on each request.
- The server always runs in UTC and ignores `TZ`, so access tokens and authorization codes expire when intended whatever the host's time zone.
- The Logs filter lists the MCPs that exist instead of every name found in the log.
- iCloud Mail `list_messages` and `get_message` cut very long subjects, address lists, and attachment lists, and flag what was cut with `<field>_truncated`.
- Pull requests and pushes to `main` now run the unit, functional, browser, and release-automation tests in the **Quality** workflow, and CodeQL also analyzes the workflows.
- The Docker image pins its Node and Deno base images by digest and Dependabot proposes their updates. The `NODE_VERSION` and `DENO_VERSION` build arguments are gone.
- The dependency audit ignores the `braces` advisory GHSA-vfj7-8cjw-p6xm, which has no fixed release and is only loaded by build tooling.

### Fixed

- Stopped the MCP dialog header from sliding out of view when a validation error was focused in a form taller than the dialog.
- Report a malformed OAuth callback as "Invalid OAuth callback" instead of returning to the app without a message.
- Stopped refreshing an upstream OAuth token on every connection when the provider issues it without `expires_in`.
- Download an attachment whose filename is not well-formed Unicode instead of returning an error.
- Kept `state` intact in gateway OAuth redirects, which had the request's query string appended to them.
- Removed expired rate-limit counters from the database instead of letting them accumulate.
- Kept pending OAuth connections small enough for the session cookie, so starting several of them no longer loses what the session was saving.

### Security

- Updated the MCP SDK to a release that limits request body size and JSON-RPC batch length in its HTTP server transport and follows client redirects only within the endpoint's origin.
- Updated Hono to 4.13.13 and its Node.js adapter to 2.1.3, which fix a `serveStatic` path-decoding bypass of middleware on static paths, and raised the pinned Hono floor to 4.13.11. MyMCPs does not serve files through `serveStatic`.
- Browser sessions are now checked on the server. They end after 2 hours idle or 24 hours, and signing out, changing the password, or running `user:reset-password` revokes the account's sessions, including a copied session cookie. Sessions from before the upgrade are refused once: browsers with a remember-me cookie get a new session automatically, the others sign in again. The upgrade adds a `session_version` column to users.
- Sign-in attempts are counted before the password is checked and limited to 5 per 15 minutes for each account and client address, and 30 failed attempts per 15 minutes for each client address. A successful sign-in to another account no longer resets the count, and IPv6 clients are counted by /64. Current-password confirmations in settings are limited to 5 per 15 minutes.
- Multipart request bodies are ignored. An anonymous upload could previously leave up to 20 MB on disk per request.
- The gateway authorization endpoint records a consent decision only from the CSRF-protected form. A `HEAD` request could previously issue an authorization code without the consent screen. Rejected authorization requests are shown on the instance instead of being redirected, unless the redirect URI is on the user's own device.
- Limited each access token to 600 gateway requests per minute, and dynamically registered OAuth clients to 1000, removing clients unused for 90 days.
- A large error response from an upstream MCP can no longer stall the instance while it is redacted. Upstream responses are limited to 32 MiB, error responses to 64 KiB, and a request stays cancellable after a redirect.
- Refused npm MCP environment variables that configure the Deno sandbox itself, such as `PATH` and loader or Deno runtime variables, which let a member escape the sandbox. Saved ones are ignored when the MCP starts, and Deno is always started from an absolute path.
- A saved bearer token, header value, or environment value is no longer carried over when an MCP is pointed at another origin, transport, or npm package; enter it again.
- Starting OAuth for an MCP with a different provider or a newly registered client drops the saved tokens first, and the start link refuses requests coming from another site. An MCP on a public address can no longer send the instance to OAuth endpoints on loopback, private, or link-local addresses, and its OAuth resource indicator must match the MCP URL.
- iCloud Mail: a crafted message can no longer stall or crash the instance when an agent reads it. HTML messages are converted in a separate short-lived process limited to 5 seconds; when one cannot be converted, `get_message` returns the headers and attachments with a `warning`. A reply is refused when the message being answered would address it to more than 50 recipients.
- Attachment links check their signature before a download is counted, are limited per MCP and client address, and serve 3 downloads at a time per MCP.
- Full page loads no longer go blank when stored text, such as a logged tool name or an MCP description, contains markup, and page data kept in the browser history is encrypted and unreadable after sign-out.
- The production server refuses to start with either `APP_KEY` published in this repository, and the container creates its files in `/app/tmp` with `umask 077`. Run `chmod -R go-rwx /app/tmp` in the container to tighten an existing volume.
- Server errors returned as JSON no longer include internal messages, failed queries no longer quote their values, and call-log slugs and caller addresses are bounded. The gateway and OAuth endpoints answer cross-origin requests without credentials.
- Release workflows install and run dependency code with a read-only token; a separate job that installs nothing tags and publishes the validated commit.

## 2026-10-02

### Changed

- Detect the Figma remote MCP (`https://mcp.figma.com/mcp`) when connecting OAuth. Figma only registers first-party client names with a localhost redirect, so MyMCPs registers as `Codex`, opens Figma in a new tab, and asks the admin to paste the localhost address that tab ends on to finish connecting.
- Register the Strava remote MCP (`https://mcp.strava.com/mcp`) as `Claude Code`. Upstream HTTP calls to Figma and Strava also use those clients' User-Agent and MCP initialize identity. Other MCP hosts still identify as MyMCPs. Strava uses the normal MyMCPs OAuth callback; only Figma still requires the pasted localhost callback.

## 2026-09-24

### Changed

- Released version [0.4.1](https://github.com/0xtlt/MyMCPs/releases/tag/v0.4.1).

### Fixed

- Share one upstream OAuth refresh across concurrent requests to the same MCP and reload the saved credentials in every caller. Parallel tool calls and health checks no longer reuse a rotating refresh token and accidentally revoke the connection.

## 2026-09-22

### Added

- Added `node ace user:reset-password <email>` for server-side account recovery, with hidden password confirmation, existing password validation, and atomic password hashing and remember-me token revocation. Documented local and Docker Compose usage.

### Changed

- Released version [0.4.0](https://github.com/0xtlt/MyMCPs/releases/tag/v0.4.0).

- Updated application and development dependencies, refreshed the lockfile, and upgraded pnpm to 12.5.1, bundled Deno to 2.9.7, and pinned GitHub Actions to current releases. TypeScript remains on 6.0.3 because the Adonis assembler and TypeScript ESLint do not support TypeScript 7 yet.
- Removed pnpm's obsolete module-purge setting for pnpm 12 compatibility and ran the Astryx 0.6 migration checks.

### Fixed

- Preserved accessible names for MCP and access-token edit dialogs after the design-system upgrade.

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
