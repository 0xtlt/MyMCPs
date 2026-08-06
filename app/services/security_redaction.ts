import type Mcp from '#models/mcp'
import McpEnvironmentStore from '#services/mcp_environment_store'
import McpSecretStore from '#services/mcp_secret_store'
import { isCredentialKey } from '#services/http_url'

function escapedRegex(value: string) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

function percentCaseInsensitiveRegex(value: string) {
  let pattern = ''
  let cursor = 0
  for (const match of value.matchAll(/%([0-9a-f]{2})/gi)) {
    pattern += escapedRegex(value.slice(cursor, match.index))
    pattern += `%${[...match[1]]
      .map((character) =>
        /[a-f]/i.test(character)
          ? `[${character.toLowerCase()}${character.toUpperCase()}]`
          : character
      )
      .join('')}`
    cursor = match.index! + match[0].length
  }
  pattern += escapedRegex(value.slice(cursor))
  return new RegExp(pattern, 'g')
}

function redactExactVariant(diagnostic: string, secret: string) {
  return /%[0-9a-f]{2}/i.test(secret)
    ? diagnostic.replace(percentCaseInsensitiveRegex(secret), '[REDACTED]')
    : diagnostic.split(secret).join('[REDACTED]')
}

function exactSecretVariants(values: Iterable<string>) {
  const variants = new Set<string>()
  for (const value of values) {
    if (!value) continue
    variants.add(value)

    const jsonContent = JSON.stringify(value).slice(1, -1)
    if (jsonContent !== value) variants.add(jsonContent)

    try {
      variants.add(encodeURIComponent(value))
      const formEncoded = new URLSearchParams({ value }).toString().slice('value='.length)
      variants.add(formEncoded)
    } catch {
      // Invalid surrogate pairs cannot be URL-encoded, but their raw value can
      // still be redacted from a JavaScript diagnostic string.
    }
  }
  return [...variants].sort((left, right) => right.length - left.length)
}

function redactExactCredentials(diagnostic: string, variants: string[]) {
  let redacted = diagnostic
  for (const secret of variants) {
    redacted = redactExactVariant(redacted, secret)
  }
  return redacted
}

const STRUCTURED_KEY_PATTERN = /(?<![A-Za-z0-9_])(["']?([A-Za-z0-9_.-]+)["']?\s*[:=]\s*)/g
const STRUCTURED_VALUE_PATTERN = /^(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|[^\s,;&#}]+)/

/**
 * Scan assignments without consuming values for noncredential wrapper labels.
 * This allows nested forms such as `Response: {"token":"..."}` and
 * `Cookie: session_token=...` to be inspected at their inner key.
 */
function redactStructuredCredentialAssignments(diagnostic: string) {
  let output = ''
  let cursor = 0
  STRUCTURED_KEY_PATTERN.lastIndex = 0

  let match: RegExpExecArray | null
  while ((match = STRUCTURED_KEY_PATTERN.exec(diagnostic))) {
    const key = match[2]
    if (!isCredentialKey(key)) continue

    const valueStart = match.index + match[0].length
    const value = diagnostic.slice(valueStart).match(STRUCTURED_VALUE_PATTERN)?.[0]
    if (!value) continue

    output += `${diagnostic.slice(cursor, valueStart)}[REDACTED]`
    cursor = valueStart + value.length
    STRUCTURED_KEY_PATTERN.lastIndex = cursor
  }

  return cursor === 0 ? diagnostic : output + diagnostic.slice(cursor)
}

function redactPatternCredentials(diagnostic: string) {
  const redacted = diagnostic
    .replace(
      /\b(?:Bearer|Basic)\s+[^\s,;"']+/gi,
      (match) => `${match.split(/\s/, 1)[0]} [REDACTED]`
    )
    .replace(/:\/\/[^\s/:@]+:[^\s/@]+@/g, '://[REDACTED]@')
    .replace(/([?&;])([^=&#;\s]+)=([^&#;\s]+)/g, (match, separator, key) =>
      isCredentialKey(key) ? `${separator}${key}=[REDACTED]` : match
    )
  return redactStructuredCredentialAssignments(redacted)
}

function peelJsonEscapeLayer(diagnostic: string) {
  return diagnostic.replaceAll(/\\(?:["'\\/bfnrt]|u[0-9a-fA-F]{4})/g, (escape) => {
    if (escape[1].toLowerCase() === 'u') {
      return String.fromCharCode(Number.parseInt(escape.slice(2), 16))
    }
    return (
      {
        '"': '"',
        "'": "'",
        '\\': '\\',
        '/': '/',
        'b': '\b',
        'f': '\f',
        'n': '\n',
        'r': '\r',
        't': '\t',
      } as Record<string, string>
    )[escape[1]]
  })
}

/**
 * Convert an error into a short diagnostic that is safe to persist or show in the UI.
 * Upstreams control their error text, so treat it as untrusted and redact common
 * credential shapes before it crosses a logging or presentation boundary.
 */
export function sanitizeDiagnostic(
  value: unknown,
  limit = 500,
  sensitiveValues: Iterable<string> = []
): string | null {
  if (!(value instanceof Error) && typeof value !== 'string') {
    return null
  }

  const raw = value instanceof Error ? value.message : value
  const secretVariants = exactSecretVariants(sensitiveValues)
  let sanitized = raw

  for (let depth = 0; depth < 8; depth++) {
    sanitized = redactExactCredentials(sanitized, secretVariants)
    sanitized = redactPatternCredentials(sanitized)
    const next = peelJsonEscapeLayer(sanitized)
    if (next === sanitized) return sanitized.slice(0, limit)
    sanitized = next
  }
  sanitized = redactExactCredentials(sanitized, secretVariants)
  return redactPatternCredentials(sanitized).slice(0, limit)
}

/**
 * Decrypt the credentials associated with one MCP only long enough to redact
 * their exact values from diagnostics. Values are never returned to callers.
 */
export function mcpSensitiveValues(mcp: Mcp) {
  const values = [
    McpSecretStore.decrypt(mcp.authBearer),
    McpSecretStore.decrypt(mcp.authHeaderValue),
    McpSecretStore.decrypt(mcp.oauthClientSecret),
    McpSecretStore.decrypt(mcp.oauthAccessToken),
    McpSecretStore.decrypt(mcp.oauthRefreshToken),
  ].filter((value): value is string => Boolean(value))

  try {
    values.push(...Object.values(McpEnvironmentStore.decrypt(mcp.npmEnv)))
  } catch {
    // A corrupt encrypted environment is already reported by the caller. Do
    // not let diagnostic sanitization replace that error with another one.
  }
  return values
}

export function sanitizeMcpDiagnostic(
  value: unknown,
  mcp: Mcp,
  limit = 500,
  additionalSensitiveValues: Iterable<string> = []
) {
  return sanitizeDiagnostic(value, limit, [
    ...mcpSensitiveValues(mcp),
    ...additionalSensitiveValues,
  ])
}
