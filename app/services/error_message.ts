/**
 * Strip HTML dumps and cap length before storing or flashing upstream errors.
 * Domain concern — not validation; Vine is for shaping known input schemas.
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
