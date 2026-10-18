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
  the first account created is a super admin, and so runs the instance.
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
  secrets, generated key, and the installed packages of npm MCPs. The server creates its files
  there with `umask 077`; a volume created by an earlier version keeps its
  existing modes until you run `chmod -R go-rwx /app/tmp` in the container.
- Put the application behind a TLS-terminating reverse proxy rather than exposing
  the container directly to the public internet.
- Treat configured npm MCP packages as executable third-party code, and pin
  their versions. The server holds each one in a sandbox of the kernel (see
  below), which does not restrict the network: a package reaches whatever the
  server reaches, the server's own port and its private network included.
  Where strangers can add packages, restrict the outbound network of the
  deployment and keep nothing unauthenticated on its network.
- Check the log of the first start after an upgrade for `npm MCPs run in a
  sandbox`. The sandbox needs Linux 5.13 or later with Landlock enabled on
  the host; a server that cannot enforce it starts no npm MCP and says why.
  Do not set `NPM_SANDBOX=off` on an instance other people use: it starts
  packages with every right of the server.
- Give a Tailscale proxy a tagged, reusable auth key, and limit in the policy of
  your tailnet what that tag reaches: an MCP that uses a proxy is trusted like
  one at a private address. See [docs/proxies.md](docs/proxies.md).
- Keep registration closed unless strangers may use the instance: it is off by
  default, and a super admin turns it on under
  **Settings → My Instance**. An account that registers gets a team of its own,
  where it can add MCPs, npm packages included: those run in a sandbox on
  your server.
- Invite only trusted people as owners or admins of a team: they manage its MCP
  registry, with the credentials it holds, and every access token of the team.
  A member reads the registry and makes access tokens of their own, which call
  the tools of every enabled MCP of the team.
- Make operators only super admins: they change the instance settings, export
  backups, which hold every credential of every team, delete accounts, and
  decide who else is a super admin. A role in a team, the Root Team included,
  gives none of that.
- Turn on sign-in protection for every super admin, and for the owners and
  admins of teams: **Settings → Sign-in protection**. Keep the recovery codes
  away from the device that holds the app or the passkey. A super admin can
  remove the protection of an account under **Users**; do it only once you are
  sure who asks.
- Revoke access tokens when a user or upstream integration is no longer trusted.

## Application safeguards

- Sign-in attempts are counted before the password is checked and limited per
  account and client address, and per client address across accounts. IPv6
  clients are counted by /64. Current-password confirmations in settings are
  limited per user.
- An account with an authenticator app or a passkey is not signed in by its
  password alone: the password opens a second step, answered within ten
  minutes with a code of the app, a passkey, or a recovery code. Codes are
  limited per account and work once. The secret of the app is stored encrypted
  with `APP_KEY`, and recovery codes as hashes. A passkey is asked to verify
  the person, is only accepted for the origin of `APP_URL`, and answers a
  challenge that is taken once. Turning a protection on or off, making new
  recovery codes, and adding or removing a passkey ask for the account
  password again.
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
- npm MCP packages are run by Node.js in a sandbox the Linux kernel enforces,
  which no setting of a package and no flaw of the runtime lifts. With
  Landlock, a package reads the files it was installed with and the system's,
  and reads and writes one directory of its own: the database, the key,
  `.env` and the files of other MCPs are refused. With seccomp, it starts no
  other process, signals none but itself, opens no Unix socket, changes the
  mode or the owner of no file by its path, and is refused tracing, mounts,
  namespaces and the other calls that administer a machine. A process holds
  at most 2 GiB of memory. The sandbox needs no privilege, and a server that
  cannot enforce it starts no package.
- `npm` installs a package inside the same sandbox, where it writes nothing
  but the directory it installs into, and runs none of the scripts of the
  package or of what it depends on. A package cannot write its own code, and
  each MCP has an install of its own.
- Only a name of the npm registry is installed, at a version, a range or a
  tag: not a path, a URL, a git repository or an alias. Environment variables
  that would configure Node.js or the loader instead of the package, such as
  `PATH`, `NODE_OPTIONS`, `SSLKEYLOGFILE` and `LD_PRELOAD`, are refused.
- A Tailscale proxy is one userspace node per proxy, with `ShieldsUp`, so
  nothing on the tailnet can connect into the instance. Its private key is in
  the data directory, readable only by the user the server runs as. The
  requests of an MCP reach it through a proxy inside the server process that
  listens on the loopback interface and asks for a random secret held in
  memory only: it is in no file, no environment variable and no log, so a
  sandboxed npm MCP cannot use the tailnet. Only HTTP MCPs use proxies, and a
  request fails when its proxy is not connected instead of going out directly.
  Through a proxy, a destination that is not on the tailnet is reached only if
  its address is public: a loopback, private, or link-local one is refused.
- An MCP that uses a proxy is treated like an MCP at a private address, since
  an operator chose that network for it: the OAuth endpoints it announces are
  accepted whatever they resolve to. Only owners and admins of a team add
  proxies and choose them.
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
  toolchain that compiles the server, the Debian runtime, the Node.js that
  runs npm MCP packages, and the `tailscaled` of proxies. The build refuses a `Cargo.lock` that does
  not match the manifests. Dependabot proposes updates for the images, for
  Cargo dependencies, and for GitHub Actions.
- The image runs the server as an unprivileged user, uid 1000, under an init
  that reaps the processes of npm MCPs. Before an image is published, a
  container of each architecture is started without capabilities and checked:
  it must answer `/health`, run as uid 1000, leave nothing in its data
  directory that another user can read, report that the sandbox of npm
  MCPs is enforced, and, on amd64, hold the `tailscaled` of the pinned version.
- Release workflows compile and run dependency code with a read-only token.
  Only a separate job can push the tag and create the release, and it publishes
  the exact commit that was validated. That job restores no cache, and the only
  code it compiles is the release tool of this repository, which has no
  dependencies.
- Pull requests and pushes to `main` run the format check, Clippy with warnings
  denied, the tests of every crate, and a release build. The security checks
  audit `Cargo.lock` against the RustSec advisory database, analyse the code
  with CodeQL, and scan the history for secrets.
