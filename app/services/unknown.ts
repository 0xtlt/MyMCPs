/**
 * Narrow unknown values at HTTP/session/JSON boundaries without `as` casts.
 */

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

export function asFiniteNumber(value: unknown): number | undefined {
  if (typeof value === 'number' && Number.isFinite(value)) {
    return value
  }
  if (typeof value === 'string' && value.trim() !== '') {
    const parsed = Number(value)
    if (Number.isFinite(parsed)) {
      return parsed
    }
  }
  return undefined
}

/**
 * Strip HTML dumps and cap length before storing or flashing upstream errors.
 */
export function sanitizeErrorMessage(error: unknown, maxLength = 500): string {
  const raw = error instanceof Error ? error.message : String(error)
  const cleaned = raw.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim()
  if (cleaned.length === 0) {
    return 'Unknown error'
  }
  if (cleaned.length <= maxLength) {
    return cleaned
  }
  return `${cleaned.slice(0, maxLength)}…`
}

export function asToolArguments(value: unknown): Record<string, unknown> {
  return isRecord(value) ? value : {}
}

export function asToolInputSchema(value: unknown): Record<string, unknown> {
  return isRecord(value) ? value : { type: 'object' }
}
