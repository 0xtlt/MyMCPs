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
security and quality checks before merging. Actions should be restricted to
trusted or explicitly approved actions, with read-only default `GITHUB_TOKEN`
permissions.

## Deployment baseline

- Complete first-run onboarding before exposing the instance to untrusted traffic;
  the first account created becomes the administrator.
- Set a unique production `APP_KEY` and keep `.env` outside version control. The
  container can generate and persist one in `/app/tmp` when it is omitted.
- Set `APP_URL` explicitly to the HTTPS URL used by the deployment. In Coolify,
  set it to the same public origin assigned to the service.
- Persist `/app/tmp`; losing it also loses the SQLite database, encrypted MCP
  secrets, generated key, and Deno sandbox cache.
- Put the application behind a TLS-terminating reverse proxy rather than exposing
  the container directly to the public internet.
- Treat configured npm MCP packages as executable third-party code. Pin versions
  and use a separate, restricted deployment boundary for untrusted packages.
- Invite only trusted operators. Members can manage the shared MCP registry and
  gateway access tokens, so membership is not a read-only role.
- Revoke access tokens when a user or upstream integration is no longer trusted.

## Application safeguards

- Repeated failed logins are rate-limited per resolved client IP.
- Authenticated upstream requests and OAuth token requests follow redirects only
  within the same origin.
- MCP and OAuth endpoints must use HTTP(S). Query parameters and embedded URL
  credentials are supported; prefer the encrypted authentication fields when
  the provider allows them.
- Upstream error details are redacted before logging or display, and optional MCP
  argument/response captures are capped at 64 KiB per field.
- The browser UI sends a restrictive Content Security Policy with per-response
  script nonces. Password changes revoke every persistent remember-me token.
