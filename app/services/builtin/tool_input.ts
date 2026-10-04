import { DateTime } from 'luxon'
import { BuiltinToolError } from '#services/builtin/definition'

type Args = Record<string, unknown>

function isMissing(value: unknown) {
  return value === undefined || value === null || value === ''
}

export function required<T>(value: T | undefined, name: string): T {
  if (value === undefined) {
    throw new BuiltinToolError(`${name} is required`)
  }
  return value
}

/** Accepts digit strings too: agents often quote large identifiers. */
export function integerInput(args: Args, name: string, range: { min?: number; max?: number } = {}) {
  const raw = args[name]
  if (isMissing(raw)) return undefined

  const value = typeof raw === 'string' && /^-?\d+$/.test(raw.trim()) ? Number(raw) : raw
  const min = range.min ?? Number.MIN_SAFE_INTEGER
  const max = range.max ?? Number.MAX_SAFE_INTEGER
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < min || value > max) {
    const bounds =
      range.min !== undefined && range.max !== undefined
        ? ` between ${range.min} and ${range.max}`
        : range.min !== undefined
          ? ` of at least ${range.min}`
          : ''
    throw new BuiltinToolError(`${name} must be an integer${bounds}`)
  }
  return value
}

export function numberInput(args: Args, name: string, range: { min: number; max: number }) {
  const raw = args[name]
  if (isMissing(raw)) return undefined

  const value = typeof raw === 'string' && raw.trim() !== '' ? Number(raw) : raw
  if (
    typeof value !== 'number' ||
    !Number.isFinite(value) ||
    value < range.min ||
    value > range.max
  ) {
    throw new BuiltinToolError(`${name} must be a number between ${range.min} and ${range.max}`)
  }
  return value
}

export function booleanInput(args: Args, name: string) {
  const raw = args[name]
  if (isMissing(raw)) return undefined
  if (raw === true || raw === 'true') return true
  if (raw === false || raw === 'false') return false
  throw new BuiltinToolError(`${name} must be true or false`)
}

export function enumInput<const T extends string>(args: Args, name: string, values: readonly T[]) {
  const raw = args[name]
  if (isMissing(raw)) return undefined
  if (typeof raw !== 'string' || !values.includes(raw as T)) {
    throw new BuiltinToolError(`${name} must be one of: ${values.join(', ')}`)
  }
  return raw as T
}

/** Identifiers end up in request paths, so only an exact pattern match is accepted. */
export function patternInput(args: Args, name: string, pattern: RegExp, hint: string) {
  const raw = args[name]
  if (isMissing(raw)) return undefined

  const value = typeof raw === 'number' ? String(raw) : raw
  if (typeof value !== 'string' || !pattern.test(value.trim())) {
    throw new BuiltinToolError(`${name} must be ${hint}`)
  }
  return value.trim()
}

/** An ISO 8601 date or datetime. Values without an offset are read as UTC. */
export function isoDateInput(args: Args, name: string) {
  const raw = args[name]
  if (isMissing(raw)) return undefined

  const parsed = typeof raw === 'string' ? DateTime.fromISO(raw.trim(), { zone: 'utc' }) : null
  if (!parsed?.isValid) {
    throw new BuiltinToolError(
      `${name} must be an ISO 8601 date or datetime, such as 2026-01-31 or 2026-01-31T18:00:00Z`
    )
  }
  return parsed
}
