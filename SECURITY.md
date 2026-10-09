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
  server generates and persists one in its data directory (`/app/tmp` in the
  container) when it is omitted. It refuses to start in production with a key
  published in this repository: the key the tests use, or the placeholder that
  images of earlier versions were built with.
- Set `APP_URL` explicitly to the HTTPS URL used by the deployment. In Coolify,
  set it to the same public origin assigned to the service.
- Set `TRUST_PROXY` to the proxies in front of the instance. The Compose and
  Coolify default, `loopback,uniquelocal`, trusts proxies on the loopback
  interface and on private networks. Use `true` only when the proxy overwrites
  `X-Forwarded-For`; otherwise clients choose the address that rate limits and
  call logs record.
- Persist `/app/tmp`; losing it also loses the SQLite database, encrypted MCP
  secrets, generated key, and Deno sandbox cache. The server creates its files
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
  directory, less the files Deno itself reads there when it starts (`.npmrc`,
  `deno.json`, `deno.jsonc`, `package.json`). Environment variables that would
  configure the sandbox itself, such as `PATH`, `SSLKEYLOGFILE` and loader or
  Deno runtime variables, are refused. All npm MCPs share one Deno cache that
  packages can read but not write.
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
- The only route that takes a file is the temporary signed upload link a
  built-in MCP hands to an agent. The signature is checked before the body is
  read, a link takes one file, and the file is deleted an hour after its
  upload. Each MCP is limited in how many files and bytes wait for it. A mail
  attachment is always bytes sent to such a link: no tool can name a path on
  the instance or a URL to attach.

## Build and release safeguards

- The container image builds from base images pinned by digest: the Rust
  toolchain that compiles the server, the Debian runtime, and the Deno binary
  that sandboxes npm MCP packages. The build refuses a `Cargo.lock` that does
  not match the manifests. Dependabot proposes updates for the images, for
  Cargo dependencies, and for GitHub Actions.
- The image runs the server as an unprivileged user, uid 1000, under an init
  that reaps the Deno child processes. Before an image is published, a
  container of each architecture is started without capabilities and checked:
  it must answer `/health`, run as uid 1000, and leave nothing in its data
  directory that another user can read.
- Release workflows compile and run dependency code with a read-only token.
  Only a separate job can push the tag and create the release, and it publishes
  the exact commit that was validated. That job restores no cache, and the only
  code it compiles is the release tool of this repository, which has no
  dependencies.
- Pull requests and pushes to `main` run the format check, Clippy with warnings
  denied, the tests of every crate, and a release build. The security checks
  audit `Cargo.lock` against the RustSec advisory database, analyse the code
  with CodeQL, and scan the history for secrets.
