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

- Set a unique production `APP_KEY` and keep `.env` outside version control. The
  container can generate and persist one in `/app/tmp` when it is omitted.
- Set `APP_URL` to the HTTPS URL used by the deployment. In Coolify, assign the
  intended domain before deployment so its generated service URL is used.
- Persist `/app/tmp`; losing it also loses the SQLite database, encrypted MCP
  secrets, generated key, and Deno sandbox cache.
- Put the application behind a TLS-terminating reverse proxy rather than exposing
  the container directly to the public internet.
- Treat configured npm MCP packages as executable third-party code. Pin versions
  and use a separate, restricted deployment boundary for untrusted packages.
- Revoke access tokens when a user or upstream integration is no longer trusted.
