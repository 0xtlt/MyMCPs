import { test } from '@japa/runner'
import vine from '@vinejs/vine'
import type { SchemaTypes } from '@vinejs/vine/types'
import { DateTime } from 'luxon'
import { BuiltinToolError } from '#services/builtin/definition'
import { builtinTool, toolInput } from '#services/builtin/tool_input'
import {
  blankAsMissing,
  boolean,
  choice,
  integer,
  isoDate,
  line,
  listLength,
  localTimestamp,
  noArgumentsValidator,
  number,
  pattern,
  text,
  toolVine,
  trimmedText,
} from '#validators/builtin_tools'

const REFUSED = Symbol('refused')
const MISSING = Symbol('missing')

/** What a tool gets for the argument `x`, or the sentence the agent reads instead. */
function argument(schema: SchemaTypes) {
  const validator = toolVine.create({ x: schema })
  return async (value: unknown) => {
    try {
      const input: Record<string, unknown> = await toolInput(validator, { x: value })
      return 'x' in input ? input.x : MISSING
    } catch (error) {
      if (!(error instanceof BuiltinToolError)) throw error
      return [REFUSED, error.message]
    }
  }
}

test.group('Vine rules of built-in tools: numbers and choices', () => {
  test('takes a whole number quoted or not, and nothing that only reads like one', async ({
    assert,
  }) => {
    const perPage = argument(integer({ min: 1, max: 100 }).optional())
    const sentence = 'x must be an integer between 1 and 100'

    for (const [value, expected] of [
      [5, 5],
      ['5', 5],
      [' 7 ', 7],
      ['007', 7],
      [100, 100],
      [1.0, 1],
    ] as const) {
      assert.strictEqual(await perPage(value), expected, JSON.stringify(value))
    }
    for (const value of [undefined, null, '']) {
      assert.strictEqual(await perPage(value), MISSING, JSON.stringify(value))
    }
    // Vine's own number type would read every one of these as a number.
    for (const value of [0, 101, 1.5, '1.0', '1e1', '0x10', '+5', ' ', true, [5], {}, '５']) {
      assert.deepEqual(await perPage(value), [REFUSED, sentence], JSON.stringify(value))
    }
  })

  test('names the bounds of a whole number, and stays within the safe integers', async ({
    assert,
  }) => {
    const id = argument(integer({ min: 1 }))

    assert.strictEqual(await id('15000000001'), 15000000001)
    assert.strictEqual(await id(Number.MAX_SAFE_INTEGER), Number.MAX_SAFE_INTEGER)
    for (const value of [0, -1, 'abc', '12/../../athlete', 2 ** 53, '9007199254740993']) {
      assert.deepEqual(
        await id(value),
        [REFUSED, 'x must be an integer of at least 1'],
        JSON.stringify(value)
      )
    }
    assert.deepEqual(await id(undefined), [REFUSED, 'x is required'])
    assert.deepEqual(await id(''), [REFUSED, 'x is required'])

    const category = argument(integer({ min: 0, max: 5 }))
    assert.deepEqual(await category(6), [REFUSED, 'x must be an integer between 0 and 5'])
  })

  test('takes a number quoted or not, within its bounds', async ({ assert }) => {
    const latitude = argument(number({ min: -90, max: 90 }).optional())
    const sentence = 'x must be a number between -90 and 90'

    for (const [value, expected] of [
      [45.5, 45.5],
      ['45.5', 45.5],
      [' -90 ', -90],
      ['1e1', 10],
      [0, 0],
    ] as const) {
      assert.strictEqual(await latitude(value), expected, JSON.stringify(value))
    }
    assert.strictEqual(await latitude(''), MISSING)
    for (const value of [120, -90.01, ' ', 'north', 'Infinity', true, false, [45], {}]) {
      assert.deepEqual(await latitude(value), [REFUSED, sentence], JSON.stringify(value))
    }

    const distance = argument(number({ min: 0, max: 10_000_000 }))
    assert.deepEqual(await distance(-5), [REFUSED, 'x must be a number between 0 and 10000000'])
    assert.deepEqual(await distance(null), [REFUSED, 'x is required'])
  })

  test('takes true and false, quoted or not, and no other way to say them', async ({ assert }) => {
    const flag = argument(boolean().optional())

    assert.strictEqual(await flag(true), true)
    assert.strictEqual(await flag('true'), true)
    assert.strictEqual(await flag(false), false)
    assert.strictEqual(await flag('false'), false)
    assert.strictEqual(await flag(''), MISSING)
    assert.strictEqual(await flag(null), MISSING)
    // Vine's own boolean type would take all of these.
    for (const value of [1, 0, '1', '0', 'on', 'off', 'TRUE', ' true ']) {
      assert.deepEqual(await flag(value), [REFUSED, 'x must be true or false'], String(value))
    }
  })

  test('lists the choices of an argument that has a few', async ({ assert }) => {
    const activityType = argument(choice(['riding', 'running']).optional())

    assert.strictEqual(await activityType('running'), 'running')
    assert.strictEqual(await activityType(''), MISSING)
    for (const value of ['Riding', ' riding', 'walking', 1, ['riding']]) {
      assert.deepEqual(
        await activityType(value),
        [REFUSED, 'x must be one of: riding, running'],
        JSON.stringify(value)
      )
    }
  })
})

test.group('Vine rules of built-in tools: text', () => {
  test('keeps text as written, an empty one included', async ({ assert }) => {
    const description = argument(text(10).optional())

    assert.strictEqual(await description(''), '')
    assert.strictEqual(await description('  two  '), '  two  ')
    assert.strictEqual(await description('a\nb'), 'a\nb')
    assert.strictEqual(await description('x'.repeat(10)), 'x'.repeat(10))
    assert.strictEqual(await description(null), MISSING)
    for (const value of ['x'.repeat(11), 5, true, ['a'], {}]) {
      assert.deepEqual(
        await description(value),
        [REFUSED, 'x must be text of at most 10 characters'],
        JSON.stringify(value)
      )
    }

    const body = argument(text(10).parse(blankAsMissing))
    assert.deepEqual(await body(''), [REFUSED, 'x is required'])
    assert.strictEqual(await body(' '), ' ')
  })

  test('trims text, and counts what is left empty as left out', async ({ assert }) => {
    const name = argument(trimmedText(10))

    assert.strictEqual(await name(' Evening '), 'Evening')
    assert.strictEqual(await name('a\nb'), 'a\nb')
    assert.deepEqual(await name('   '), [REFUSED, 'x is required'])
    assert.deepEqual(await name(''), [REFUSED, 'x is required'])
    // The limit is on what was written, spaces included.
    assert.strictEqual(await name(` ${'x'.repeat(9)}`), 'x'.repeat(9))
    assert.deepEqual(await name(` ${'x'.repeat(10)}`), [
      REFUSED,
      'x must be text of at most 10 characters',
    ])
    assert.deepEqual(await name(5), [REFUSED, 'x must be text of at most 10 characters'])
  })

  test('refuses control characters in a single line of text', async ({ assert }) => {
    const mailbox = argument(line(10).optional())

    assert.strictEqual(await mailbox(' Archive '), 'Archive')
    assert.strictEqual(await mailbox('Boîte'), 'Boîte')
    assert.strictEqual(await mailbox(' \n '), MISSING)
    for (const value of ['a\nb', 'a\rb', 'a\tb', 'a\u0000b', 'a\u007fb', 'a\u0085b']) {
      assert.deepEqual(
        await mailbox(value),
        [REFUSED, 'x must be a single line of text'],
        JSON.stringify(value)
      )
    }
    assert.deepEqual(await mailbox('x'.repeat(11)), [
      REFUSED,
      'x must be text of at most 10 characters',
    ])
    assert.deepEqual(await argument(line(10))('  '), [REFUSED, 'x is required'])
  })

  test('takes an identifier only when it matches its pattern in full', async ({ assert }) => {
    const gear = argument(pattern(/^[bg]\d{1,20}$/, 'a gear identifier such as b1234567'))
    const sentence = 'x must be a gear identifier such as b1234567'

    assert.strictEqual(await gear('b1234567'), 'b1234567')
    assert.strictEqual(await gear(' g1 '), 'g1')
    for (const value of ['../athlete', 'b12?x=1', 'B12', 'b', ' ', 12, true, ['b1']]) {
      assert.deepEqual(await gear(value), [REFUSED, sentence], JSON.stringify(value))
    }
    assert.deepEqual(await gear(''), [REFUSED, 'x is required'])

    // An identifier made of digits may come as a number.
    const part = argument(pattern(/^\d{1,3}(\.\d{1,3}){0,9}$/, 'a part'))
    assert.strictEqual(await part(2), '2')
    assert.strictEqual(await part(1.2), '1.2')
    assert.deepEqual(await part(1000), [REFUSED, 'x must be a part'])
  })

  test('reads ISO 8601 dates as UTC unless they say otherwise', async ({ assert }) => {
    const after = argument(isoDate().optional())
    const iso = async (value: unknown) => {
      const date = await after(value)
      return DateTime.isDateTime(date) ? date.toISO() : date
    }

    assert.equal(await iso('2026-09-01'), '2026-09-01T00:00:00.000Z')
    assert.equal(await iso(' 2026-10-01T12:00:00+02:00 '), '2026-10-01T10:00:00.000Z')
    assert.strictEqual(await iso(''), MISSING)
    for (const value of ['last week', '2026-13-01', '2026-02-30', ' ', 1767225600, true]) {
      assert.deepEqual(
        await iso(value),
        [
          REFUSED,
          'x must be an ISO 8601 date or datetime, such as 2026-01-31 or 2026-01-31T18:00:00Z',
        ],
        JSON.stringify(value)
      )
    }
  })

  test('keeps the clock reading of a local time, whatever offset it names', async ({ assert }) => {
    const start = argument(localTimestamp())

    assert.strictEqual(await start('2026-10-03T19:30:00+02:00'), '2026-10-03T19:30:00Z')
    assert.strictEqual(await start('2026-10-03T08:00:00'), '2026-10-03T08:00:00Z')
    assert.strictEqual(await start('2026-10-03'), '2026-10-03T00:00:00Z')
    assert.deepEqual(await start('tomorrow'), [
      REFUSED,
      'x must be an ISO 8601 local date and time, such as 2026-01-31T18:00:00',
    ])
    assert.deepEqual(await start(undefined), [REFUSED, 'x is required'])
  })
})

test.group('Vine rules of built-in tools: what the agent reads', () => {
  const validator = toolVine.create({
    first: integer({ min: 1 }),
    second: boolean().optional(),
    ids: toolVine
      .array(integer({ min: 1, max: 9 }))
      .use(listLength({ min: 1, max: 3, sentence: '{{ field }} must be a list of 1 to 3 ids' }))
      .optional(),
  })
  /** The sentence the agent reads, or `null` when the arguments are fine. */
  const refusal = (args: unknown) =>
    toolInput(validator, args).then(
      () => null,
      (error: unknown) => {
        if (!(error instanceof BuiltinToolError)) throw error
        return error.message
      }
    )

  test('reports one argument at a time, in the order the schema lists them', async ({ assert }) => {
    assert.equal(await refusal({ second: 'maybe', ids: 'x' }), 'first is required')
    assert.equal(
      await refusal({ first: 0, second: 'maybe' }),
      'first must be an integer of at least 1'
    )
    assert.equal(
      await refusal({ first: 1, second: 'maybe', ids: [] }),
      'second must be true or false'
    )
    assert.isNull(await refusal({ first: 1 }))
  })

  test('names an item of a list after the list', async ({ assert }) => {
    assert.equal(await refusal({ first: 1, ids: [] }), 'ids must be a list of 1 to 3 ids')
    assert.equal(await refusal({ first: 1, ids: [1, 2, 3, 4] }), 'ids must be a list of 1 to 3 ids')
    assert.equal(
      await refusal({ first: 1, ids: [1, 'x'] }),
      'ids must be an integer between 1 and 9'
    )
    assert.equal(await refusal({ first: 1, ids: [1, null] }), 'ids is required')
    assert.deepEqual(await toolInput(validator, { first: 1, ids: ['2', 3] }), {
      first: 1,
      ids: [2, 3],
    })
  })

  test('drops the arguments a tool does not know, and refuses what is not a set of them', async ({
    assert,
  }) => {
    assert.deepEqual(await toolInput(validator, { first: '1', extra: true, constructor: 1 }), {
      first: 1,
    })
    assert.deepEqual(await toolInput(noArgumentsValidator, { anything: 1 }), {})
    assert.equal(await refusal(null), 'arguments is required')
    assert.equal(await refusal([1]), 'arguments must be an object')
    assert.equal(await refusal('first'), 'arguments must be an object')
  })

  test('leaves an empty string alone, unlike the Vine of the pages', async ({ assert }) => {
    // start/validator.ts turns empty strings into null for HTML forms.
    assert.isTrue(vine.convertEmptyStringsToNull)
    assert.isFalse(toolVine.convertEmptyStringsToNull)

    const form = vine.create({ description: vine.string().optional() })
    const tool = toolVine.create({ description: text(100).optional() })
    assert.deepEqual(await form.validate({ description: '' }), {})
    assert.deepEqual(await toolInput(tool, { description: '' }), { description: '' })
  })

  test('says how each argument reads in a JSON Schema', ({ assert }) => {
    const described = toolVine.create({
      id: integer({ min: 1 }),
      per_page: integer({ min: 1, max: 100 }).optional(),
      weight: number({ min: 20, max: 400 }),
      starred: boolean().optional(),
      kind: choice(['riding', 'running']).optional(),
      name: trimmedText(255),
      mailbox: line(255).optional(),
      gear_id: pattern(/^b$/, 'b').optional(),
      after: isoDate().optional(),
      start: localTimestamp(),
      ids: toolVine
        .array(integer({ min: 1 }))
        .use(listLength({ min: 1, max: 3, sentence: '' }))
        .optional(),
    })

    assert.deepEqual(described.toJSONSchema(), {
      type: 'object',
      properties: {
        id: { type: 'integer', minimum: 1 },
        per_page: { type: 'integer', minimum: 1, maximum: 100 },
        weight: { type: 'number', minimum: 20, maximum: 400 },
        starred: { type: 'boolean' },
        kind: { enum: ['riding', 'running'] },
        name: { type: 'string', maxLength: 255 },
        mailbox: { type: 'string', maxLength: 255 },
        gear_id: { type: 'string' },
        after: { type: 'string' },
        start: { type: 'string' },
        ids: {
          type: 'array',
          items: { type: 'integer', minimum: 1 },
          minItems: 1,
          maxItems: 3,
        },
      },
      required: ['id', 'weight', 'name', 'start'],
      additionalProperties: false,
    })
  })
})

test.group('Vine rules of built-in tools: tools', () => {
  const tool = builtinTool({
    name: 'greet',
    description: 'Greets.',
    inputSchema: { type: 'object', properties: { name: { type: 'string' } } },
    input: toolVine.withMetaData<{ greeting: string }>().create({
      name: trimmedText(20),
      times: integer({ min: 1, max: 3 }).optional(),
    }),
    run: async ({ name, times = 1 }, context: { greeting: string }) =>
      Array.from({ length: times }, () => `${context.greeting} ${name}`),
  })

  test('runs a tool with its arguments as the validator returns them', async ({ assert }) => {
    assert.deepEqual(await tool.run({ name: '  Ada ', times: '2', other: 1 }, { greeting: 'Hi' }), [
      'Hi Ada',
      'Hi Ada',
    ])
    assert.deepEqual(tool.input.toJSONSchema().required, ['name'])
  })

  test('refuses wrong arguments with a BuiltinToolError before the tool runs', async ({
    assert,
  }) => {
    let runs = 0
    const counted = builtinTool({
      ...tool,
      input: toolVine.create({ name: trimmedText(20) }),
      run: async () => {
        runs += 1
      },
    })

    await assert.rejects(() => counted.run({}, { greeting: 'Hi' }), 'name is required')
    await assert.rejects(
      () => counted.run({ name: 'x'.repeat(21) }, { greeting: 'Hi' }),
      'name must be text of at most 20 characters'
    )
    const error = await counted.run({ name: 5 }, { greeting: 'Hi' }).catch((reason) => reason)
    assert.instanceOf(error, BuiltinToolError)
    assert.equal(runs, 0)
  })

  test('passes the context of the call to the rules that depend on it', async ({ assert }) => {
    const knownRule = toolVine.createRule((value, _options, field) => {
      if (!field.meta.known.includes(value)) {
        field.report('{{ field }} must be someone we know', 'known', field)
      }
    })
    const validator = toolVine
      .withMetaData<{ known: string[] }>()
      .create({ name: trimmedText(20).use(knownRule()) })

    assert.deepEqual(await toolInput(validator, { name: ' Ada ' }, { known: ['Ada'] }), {
      name: 'Ada',
    })
    await assert.rejects(
      () => toolInput(validator, { name: 'Bob' }, { known: ['Ada'] }),
      'name must be someone we know'
    )
  })
})
