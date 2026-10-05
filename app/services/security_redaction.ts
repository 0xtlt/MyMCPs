import type Mcp from '#models/mcp'
import McpEnvironmentStore from '#services/mcp_environment_store'
import McpSecretStore from '#services/mcp_secret_store'

const CREDENTIAL_KEYS = new Set([
  'authorization',
  'csrftoken',
  'googleaccessid',
  'jsessionid',
  'passphrase',
  'sessionid',
  'sig',
  'xsrftoken',
])

const CREDENTIAL_KEY_SUFFIXES = [
  'accesskey',
  'accountkey',
  'apikey',
  'credential',
  'password',
  'privatekey',
  'secret',
  'sessionkey',
  'signature',
  'subscriptionkey',
  'token',
]

function isCredentialKey(value: string) {
  let decoded = value
  try {
    decoded = decodeURIComponent(value.replaceAll('+', ' '))
  } catch {
    // Check the raw key when malformed percent escapes cannot be decoded.
  }
  const normalized = decoded.toLowerCase().replaceAll(/[^a-z0-9]/g, '')
  return (
    CREDENTIAL_KEYS.has(normalized) ||
    CREDENTIAL_KEY_SUFFIXES.some((suffix) => normalized.endsWith(suffix))
  )
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
    redacted = redacted.split(secret).join('[REDACTED]')
  }
  return redacted
}

/**
 * Upstreams choose the size of their error text, so pattern redaction only
 * ever scans this many characters of it.
 */
const PATTERN_SCAN_CHARS = 8 * 1024

// The lookbehind covers every key character so that a key is only tried from
// its first character. Retrying from each "." or "-" made long runs quadratic.
const STRUCTURED_KEY_PATTERN = /(?<![A-Za-z0-9_.-])(["']?([A-Za-z0-9_.-]+)["']?\s*[:=]\s*)/g
const STRUCTURED_VALUE_PATTERN = /^(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|[^\s,;&#}]+)/
const QUOTED_VALUE_PATTERN = /^(?:"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*')/

/**
 * Scan assignments without consuming values for noncredential wrapper labels.
 * This allows nested forms such as `Response: {"token":"..."}` and
 * `Cookie: session_token=...` to be inspected at their inner key.
 */
function redactStructuredCredentialAssignments(diagnostic: string, truncated: boolean) {
  let output = ''
  let cursor = 0
  STRUCTURED_KEY_PATTERN.lastIndex = 0

  let match: RegExpExecArray | null
  while ((match = STRUCTURED_KEY_PATTERN.exec(diagnostic))) {
    const key = match[2]
    if (!isCredentialKey(key)) continue

    const valueStart = match.index + match[0].length
    const rest = diagnostic.slice(valueStart)
    let value = rest.match(STRUCTURED_VALUE_PATTERN)?.[0]
    if (!value) continue

    // A quoted value that lost its closing quote to the cut runs to the end.
    if (truncated && /^["']/.test(value) && !QUOTED_VALUE_PATTERN.test(rest)) value = rest

    output += `${diagnostic.slice(cursor, valueStart)}[REDACTED]`
    cursor = valueStart + value.length
    STRUCTURED_KEY_PATTERN.lastIndex = cursor
  }

  return cursor === 0 ? diagnostic : output + diagnostic.slice(cursor)
}

/**
 * `truncated` says the text was cut short, so a credential may be missing the
 * closing delimiter that would otherwise identify it.
 */
function redactPatternCredentials(diagnostic: string, truncated: boolean) {
  let redacted = diagnostic
    .replace(
      /\b(?:Bearer|Basic)\s+(?!error\s*=)[^\s,;"']+/gi,
      (match) => `${match.split(/\s/, 1)[0]} [REDACTED]`
    )
    .replace(/:\/\/[^\s/:@]+:[^\s/@]+@/g, '://[REDACTED]@')
  if (truncated) {
    redacted = redacted.replace(/:\/\/[^\s/:@]+:[^\s/@]*$/, '://[REDACTED]')
  }
  return redactStructuredCredentialAssignments(redacted, truncated)
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
  // Known secrets are replaced over the whole text, in linear time, so that
  // none is left half-cut at the edge of the part the patterns then scan.
  const sanitized = redactExactCredentials(raw, secretVariants)
  const scanLength = Math.max(limit, PATTERN_SCAN_CHARS)
  return redactPatternCredentials(
    sanitized.slice(0, scanLength),
    sanitized.length > scanLength
  ).slice(0, limit)
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
    McpSecretStore.decrypt(mcp.builtinPassword),
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
