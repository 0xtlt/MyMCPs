# Security policy

## Reporting a vulnerability

Please report suspected vulnerabilities privately through GitHub's security advisory
reporting flow for this repository. Do not open a public issue or include working
credentials, access tokens, or exploit details in a public discussion.

If private reporting is unavailable, contact the repository owner privately and
include the affected version, reproduction steps, and impact.

## Deployment baseline

- Set a unique production `APP_KEY` and keep `.env` outside version control.
- Set `APP_URL` to the HTTPS URL used by the deployment.
- Put the application behind a TLS-terminating reverse proxy rather than exposing
  the container directly to the public internet.
- Treat configured npm MCP packages as executable third-party code. Pin versions
  and use a separate, restricted deployment boundary for untrusted packages.
- Revoke access tokens when a user or upstream integration is no longer trusted.
