# Security policy

## Reporting a vulnerability

Please report suspected vulnerabilities privately through GitHub's security advisory
reporting flow for this repository. Do not open a public issue or include working
credentials, access tokens, or exploit details in a public discussion.

If private reporting is unavailable, contact the repository owner privately and
include the affected version, reproduction steps, and impact.

## Required GitHub settings

Repository administrators should enable the dependency graph, Dependabot alerts,
secret scanning with push protection, and branch protection requiring the
security checks and the Quality workflow's `Lint and typecheck` and `Tests`
checks before merging. Actions should be restricted to
trusted or explicitly approved actions, with read-only default `GITHUB_TOKEN`
permissions.

## Deployment baseline

- Complete first-run onboarding before exposing the instance to untrusted traffic;
  the first account created becomes the administrator.
- Set a unique production `APP_KEY` and keep `.env` outside version control. The
  container can generate and persist one in `/app/tmp` when it is omitted. The
  server refuses to start in production with the Docker build placeholder or the
  test key committed in `.env.test`.
- Set `APP_URL` explicitly to the HTTPS URL used by the deployment. In Coolify,
  set it to the same public origin assigned to the service.
- Set `TRUST_PROXY` to the proxies in front of the instance. The Compose and
  Coolify default, `loopback,uniquelocal`, trusts proxies on the loopback
  interface and on private networks. Use `true` only when the proxy overwrites
  `X-Forwarded-For`; otherwise clients choose the address that rate limits and
  call logs record.
- Persist `/app/tmp`; losing it also loses the SQLite database, encrypted MCP
  secrets, generated key, and Deno sandbox cache. The container creates its files
  there with `umask 077`; a volume created by an earlier version keeps its
  existing modes until you run `chmod -R go-rwx /app/tmp` in the container.
- Put the application behind a TLS-terminating reverse proxy rather than exposing
  the container directly to the public internet.
- Treat configured npm MCP packages as executable third-party code. Pin versions
  and use a separate, restricted deployment boundary for untrusted packages.
- Invite only trusted operators. Members can manage the shared MCP registry and
  gateway access tokens, so membership is not a read-only role.
- Revoke access tokens when a user or upstream integration is no longer trusted.

## Application safeguards

- Sign-in attempts are counted before the password is checked and limited per
  account and client address, and per client address across accounts. IPv6
  clients are counted by /64. Current-password confirmations in settings are
  limited per user.
- Browser sessions are checked on the server on every request. They end after
  2 hours idle or 24 hours, and signing out, changing the password, or running
  `user:reset-password` revokes the account's sessions, including a copied
  session cookie. Password changes and resets also revoke every remember-me
  token.
- The gateway authorization endpoint records a decision only from the
  CSRF-protected consent form, and sends a rejected request back only to a
  redirect URI on the user's own device.
- Each access token is limited to 600 gateway requests per minute. Dynamically
  registered OAuth clients are capped at 1000, and clients unused for 90 days
  are removed.
- Multipart request bodies are ignored; the instance never writes uploads to
  disk.
- Authenticated upstream requests and OAuth token requests follow redirects only
  within the same origin. Upstream responses are limited to 32 MiB, and error
  responses to 64 KiB.
- npm MCP packages run in a Deno sandbox that can read and write only its own
  directory. Environment variables that would configure the sandbox itself, such
  as `PATH` and loader or Deno runtime variables, are refused. All npm MCPs share
  one Deno cache that packages can read but not write.
- A saved bearer token, header value, or environment value is not carried over
  when an MCP is pointed at another origin, transport, or npm package.
- An MCP on a public address cannot send the instance to OAuth endpoints on
  loopback, private, or link-local addresses, and its OAuth resource indicator
  must match the MCP URL. MCP URLs entered by an operator may still point at
  private addresses. Starting OAuth with a different provider or client drops the
  saved tokens first.
- MCP and OAuth endpoints must use HTTP(S). Query parameters and embedded URL
  credentials are supported; prefer the encrypted authentication fields when
  the provider allows them.
- Upstream error details are redacted before logging or display, and optional MCP
  argument/response captures are capped at 64 KiB per field.
- The browser UI sends a restrictive Content Security Policy with per-response
  script nonces. Page data embedded in a full page load is escaped so that
  stored text, such as a logged tool name or an MCP description, cannot break
  out of its script element, and page data kept in the browser history is
  encrypted and unreadable after sign-out.
- Mail written by strangers is converted to text in a separate short-lived
  process, and attachment links check their signature before a download is
  counted against the per-MCP limits.

## Build and release safeguards

- The container image builds from base images pinned by digest: the Node runtime
  and the Deno binary that sandboxes npm MCP packages. Dependabot proposes
  updates for them, for npm packages, and for GitHub Actions.
- Release workflows install and run dependency code with a read-only token. Only
  a separate job that installs nothing can push the tag and create the release,
  and it publishes the exact commit that was validated.
- Pull requests and pushes to `main` run lint, typecheck, and the unit,
  functional, and browser suites.
