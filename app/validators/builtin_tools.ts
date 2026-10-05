/**
 * Vine rules shared by the tools of built-in MCPs, for the arguments agents
 * call them with.
 */
import { BaseLiteralType, SimpleMessagesProvider, symbols, Vine } from '@vinejs/vine'
import type { FieldContext, Validation } from '@vinejs/vine/types'
import { DateTime } from 'luxon'

/**
 * Says what is wrong in one sentence the agent can act on, naming the
 * argument. An item of a list is named after its list: `uids`, not `2`.
 */
class ArgumentMessages extends SimpleMessagesProvider {
  getMessage(message: string, rule: string, field: FieldContext, args?: Record<string, any>) {
    const [argument] = field.wildCardPath.split('.')
    const choices = args?.choices
    return super.getMessage(
      message,
      rule,
      { ...field, name: argument || 'arguments' },
      Array.isArray(choices) ? { ...args, choices: choices.join(', ') } : args
    )
  }
}

/**
 * Arguments are JSON written by an agent, not the fields of a form, so they
 * get a Vine of their own: the one the pages use turns an empty string into
 * null, and here an empty description is how a description gets cleared.
 */
export const toolVine = new Vine()

/** The rules of Vine's own types that the schemas use. The others below bring their sentence. */
toolVine.messagesProvider = new ArgumentMessages({
  required: '{{ field }} is required',
  object: '{{ field }} must be an object',
  boolean: '{{ field }} must be true or false',
  enum: '{{ field }} must be one of: {{ choices }}',
})

/**
 * An argument checked by the rules below. Vine's string and number types
 * cannot tell the agent what was expected: their type check knows neither the
 * bounds nor the format to name. Each rule also says how its argument reads
 * in a JSON Schema, to compare with the one the tool advertises.
 */
export class VineArgument<Output> extends BaseLiteralType<unknown, Output, Output> {
  [symbols.SUBTYPE] = 'argument'

  constructor(...validations: Validation<any>[]) {
    super({}, validations)
  }

  clone() {
    const cloned = new VineArgument<Output>(...this.cloneValidations())
    cloned.options = this.cloneOptions()
    return cloned as this
  }
}

export function isBlank(value: unknown) {
  return value === undefined || value === null || value === ''
}

/** An empty string is one of the ways agents leave an argument out. */
export function blankAsMissing(value: unknown) {
  return value === '' ? undefined : value
}

type IntegerRange = { min: number; max?: number }

const integerRule = toolVine.createRule<IntegerRange>(
  (value, range, field) => {
    // Agents often quote large identifiers.
    const parsed = typeof value === 'string' && /^-?\d+$/.test(value.trim()) ? Number(value) : value
    if (
      typeof parsed !== 'number' ||
      !Number.isSafeInteger(parsed) ||
      parsed < range.min ||
      parsed > (range.max ?? Number.MAX_SAFE_INTEGER)
    ) {
      field.report(
        range.max === undefined
          ? '{{ field }} must be an integer of at least {{ min }}'
          : '{{ field }} must be an integer between {{ min }} and {{ max }}',
        'integer',
        field,
        range
      )
      return
    }
    field.mutate(parsed, field)
  },
  {
    toJSONSchema: (schema, { min, max }) => {
      Object.assign(schema, { type: 'integer', minimum: min })
      if (max !== undefined) schema.maximum = max
    },
  }
)

/** A whole number, also when it is quoted. */
export function integer(range: IntegerRange) {
  return new VineArgument<number>(integerRule(range)).parse(blankAsMissing)
}

type NumberRange = { min: number; max: number }

const numberRule = toolVine.createRule<NumberRange>(
  (value, range, field) => {
    const parsed = typeof value === 'string' && value.trim() !== '' ? Number(value) : value
    if (
      typeof parsed !== 'number' ||
      !Number.isFinite(parsed) ||
      parsed < range.min ||
      parsed > range.max
    ) {
      field.report(
        '{{ field }} must be a number between {{ min }} and {{ max }}',
        'number',
        field,
        range
      )
      return
    }
    field.mutate(parsed, field)
  },
  {
    toJSONSchema: (schema, { min, max }) => {
      Object.assign(schema, { type: 'number', minimum: min, maximum: max })
    },
  }
)

/** A number, also when it is quoted. */
export function number(range: NumberRange) {
  return new VineArgument<number>(numberRule(range)).parse(blankAsMissing)
}

/** JSON booleans, and the same two words quoted. Vine's own conversion would also take 1 and "on". */
export function boolean() {
  return toolVine
    .boolean({ strict: true })
    .parse((value) => (value === 'true' ? true : value === 'false' ? false : blankAsMissing(value)))
}

export function choice<const Values extends readonly string[]>(values: Values) {
  return toolVine.enum(values).parse(blankAsMissing)
}

const textRule = toolVine.createRule<{ max: number }>(
  (value, { max }, field) => {
    if (typeof value !== 'string' || value.length > max) {
      field.report('{{ field }} must be text of at most {{ max }} characters', 'text', field, {
        max,
      })
    }
  },
  {
    toJSONSchema: (schema, { max }) => {
      Object.assign(schema, { type: 'string', maxLength: max })
    },
  }
)

/** Text as written: an empty one is a value. */
export function text(max: number) {
  return new VineArgument<string>(textRule({ max }))
}

/** Text without the spaces around it. Left out when nothing remains. */
export function trimmedText(max: number) {
  // Text that is too long is left for the rule to refuse.
  return text(max).parse((value) =>
    typeof value === 'string' && value.length <= max ? value.trim() || undefined : value
  )
}

const singleLineRule = toolVine.createRule((value, _options, field) => {
  if (/\p{Cc}/u.test(value as string)) {
    field.report('{{ field }} must be a single line of text', 'line', field)
  }
})

/** One line of text: it ends up in a mail header or an IMAP command. */
export function line(max: number) {
  return trimmedText(max).use(singleLineRule())
}

const jsonSchemaString = {
  toJSONSchema: (schema: { type?: unknown }) => {
    schema.type = 'string'
  },
}

const patternRule = toolVine.createRule<{ expression: RegExp; hint: string }>(
  (value, { expression, hint }, field) => {
    const written = typeof value === 'number' ? String(value) : value
    if (typeof written !== 'string' || !expression.test(written.trim())) {
      field.report('{{ field }} must be {{ hint }}', 'pattern', field, { hint })
      return
    }
    field.mutate(written.trim(), field)
  },
  jsonSchemaString
)

/** Identifiers end up in request paths, so only an exact pattern match is accepted. */
export function pattern(expression: RegExp, hint: string) {
  return new VineArgument<string>(patternRule({ expression, hint })).parse(blankAsMissing)
}

const isoDateRule = toolVine.createRule((value, _options, field) => {
  const parsed = typeof value === 'string' ? DateTime.fromISO(value.trim(), { zone: 'utc' }) : null
  if (!parsed?.isValid) {
    field.report(
      '{{ field }} must be an ISO 8601 date or datetime, such as 2026-01-31 or 2026-01-31T18:00:00Z',
      'isoDate',
      field
    )
    return
  }
  field.mutate(parsed, field)
}, jsonSchemaString)

/** An ISO 8601 date or datetime. Values without an offset are read as UTC. */
export function isoDate() {
  return new VineArgument<DateTime<true>>(isoDateRule()).parse(blankAsMissing)
}

const localTimestampRule = toolVine.createRule((value, _options, field) => {
  const parsed =
    typeof value === 'string' ? DateTime.fromISO(value.trim(), { setZone: true }) : null
  if (!parsed?.isValid) {
    field.report(
      '{{ field }} must be an ISO 8601 local date and time, such as 2026-01-31T18:00:00',
      'localTimestamp',
      field
    )
    return
  }
  field.mutate(parsed.toFormat("yyyy-MM-dd'T'HH:mm:ss'Z'"), field)
}, jsonSchemaString)

/**
 * A wall-clock time in the user's own timezone, which some APIs take as an
 * ISO 8601 string ending in `Z`. The clock reading is kept as written: an
 * offset in the input is not converted to UTC.
 */
export function localTimestamp() {
  return new VineArgument<string>(localTimestampRule()).parse(blankAsMissing)
}

/**
 * How many items a list may have. `sentence` is what the agent reads when it
 * has fewer or more, since each list names its items its own way.
 */
export const listLength = toolVine.createRule<{ min?: number; max: number; sentence: string }>(
  (value, { min = 0, max, sentence }, field) => {
    const { length } = value as unknown[]
    if (length < min || length > max) {
      field.report(sentence, 'listLength', field)
    }
  },
  {
    toJSONSchema: (schema, { min, max }) => {
      if (min !== undefined) schema.minItems = min
      schema.maxItems = max
    },
  }
)

/** For the tools that take no arguments: whatever they are passed is ignored. */
export const noArgumentsValidator = toolVine.create({})
