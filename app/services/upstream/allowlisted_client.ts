import { applicationVersion } from '#services/application_version'
import { parseHttpUrl } from '#services/http_url'

/** OAuth dynamic-registration name used for hosts that do not allowlist a client. */
export const DEFAULT_OAUTH_CLIENT_NAME = 'MyMCPs'

/** MCP initialize identity used for hosts that do not allowlist a client. */
export const DEFAULT_MCP_CLIENT_NAME = 'mymcps-gateway'

export type UpstreamMcpClientInfo = {
  name: string
  version: string
  title?: string
}

export type AllowlistedUpstreamClient = {
  /** Compared to the MCP URL hostname. */
  hostname: string
  /** RFC 7591 `client_name`. These hosts allowlist the exact string. */
  oauthClientName: string
  /** MCP `initialize` `clientInfo`, matching that client's handshake. */
  mcpClientInfo: UpstreamMcpClientInfo
  /** `User-Agent` on HTTP requests whose host is `hostname`. */
  userAgent: string
  /**
   * Fixed loopback redirect. Nothing listens on it, so the admin pastes the
   * browser address back into MyMCPs. Omit to use the normal app callback.
   */
  loopbackRedirectUri?: string
}

/**
 * Remote MCP hosts that reject a generic client.
 *
 * OAuth registration is the check these providers document. Figma's register
 * endpoint accepts an exact `client_name` such as `Codex` and rejects others
 * (anthropics/claude-code#74768); the initialize HTTP 403 in that report was
 * the failed registration, not a separate `clientInfo` check. Strava Help
 * documents Claude Code as the HTTP client, and generic dynamic registration
 * is rejected at registration or token issuance rather than by User-Agent
 * (millerchou/strava-mcp-bridge). The runtime fields below still match the
 * clients those hosts allow, so a later header or handshake check sees the
 * same identity as OAuth.
 *
 * Codex (openai/codex `rmcp-client`, workspace package version 0.0.0) sends
 * `clientInfo.name` `codex-mcp-client`, title `Codex`, and
 * `User-Agent: codex-mcp-client/0.0.0`. It only registers that name with a
 * localhost redirect, which is why Figma keeps the pasted loopback callback.
 * Claude Code sends `clientInfo.name` `claude-code`, title `Claude Code`, and
 * `User-Agent: claude-code/<version> (cli)` (observed as 2.1.89). Strava does
 * not document a fixed loopback redirect, so it uses the normal app callback.
 */
const ALLOWLISTED_UPSTREAM_CLIENTS: readonly AllowlistedUpstreamClient[] = [
  {
    hostname: 'mcp.figma.com',
    oauthClientName: 'Codex',
    mcpClientInfo: {
      name: 'codex-mcp-client',
      version: '0.0.0',
      title: 'Codex',
    },
    userAgent: 'codex-mcp-client/0.0.0',
    loopbackRedirectUri: 'http://localhost:45873/callback',
  },
  {
    hostname: 'mcp.strava.com',
    oauthClientName: 'Claude Code',
    mcpClientInfo: {
      name: 'claude-code',
      version: '2.1.89',
      title: 'Claude Code',
    },
    userAgent: 'claude-code/2.1.89 (cli)',
  },
]

export function allowlistedUpstreamClient(httpUrl: string | null | undefined) {
  if (!httpUrl) return null

  try {
    const hostname = parseHttpUrl(httpUrl, 'MCP URL').hostname
    return ALLOWLISTED_UPSTREAM_CLIENTS.find((client) => client.hostname === hostname) ?? null
  } catch {
    return null
  }
}

/** `client_name` for dynamic client registration. */
export function registrationClientName(httpUrl: string | null | undefined) {
  return allowlistedUpstreamClient(httpUrl)?.oauthClientName ?? DEFAULT_OAUTH_CLIENT_NAME
}

export function mcpClientInfoForUrl(httpUrl: string | null | undefined): UpstreamMcpClientInfo {
  return (
    allowlistedUpstreamClient(httpUrl)?.mcpClientInfo ?? {
      name: DEFAULT_MCP_CLIENT_NAME,
      version: applicationVersion,
    }
  )
}

/** Identity headers for an allowlisted URL. Empty for every other host. */
export function upstreamIdentityHeaders(
  httpUrl: string | null | undefined
): Record<string, string> {
  const userAgent = allowlistedUpstreamClient(httpUrl)?.userAgent
  return userAgent ? { 'User-Agent': userAgent } : {}
}
