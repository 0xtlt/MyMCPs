import { test } from '@japa/runner'
import {
  parseCallToolInput,
  parseGatewayToolMode,
  parseToolSearchInput,
} from '#services/gateway_lazy_tools'
import { parseNamespacedTool } from '#services/upstream/manager'
import {
  callToolValidator,
  gatewayToolModeValidator,
  namespacedToolValidator,
  toolSearchValidator,
} from '#validators/gateway'

const MCP_MESSAGE = 'mcp must be a non-empty MCP slug of at most 120 characters'
const QUERY_MESSAGE = 'query must be non-empty and at most 200 characters'
const LIMIT_MESSAGE = 'limit must be an integer between 1 and 20'
const TOOL_MESSAGE = 'tool must be a non-empty upstream tool name of at most 128 characters'
const ARGUMENTS_MESSAGE = 'arguments must be an object when provided'

/** Values that stand where a string is expected without being one. */
const notStrings = [undefined, null, 0, 1, true, ['issues'], [], {}, { slug: 'issues' }]

test.group('vine: gateway tool mode header', () => {
  test('reads eager and lazy whatever their case and surrounding whitespace', async ({
    assert,
  }) => {
    for (const [header, mode] of [
      ['eager', 'eager'],
      ['lazy', 'lazy'],
      ['LAZY', 'lazy'],
      ['Eager', 'eager'],
      [' LaZy ', 'lazy'],
      ['\tlazy\n', 'lazy'],
    ]) {
      const [error, value] = await gatewayToolModeValidator.tryValidate(header)
      assert.isNull(error)
      assert.equal(value, mode)
      assert.equal(await parseGatewayToolMode(header, 'eager'), mode)
    }
  })

  test('leaves the mode to the instance when the header is absent or blank', async ({ assert }) => {
    for (const header of [undefined, '', ' ', ' \t ']) {
      const [error, value] = await gatewayToolModeValidator.tryValidate(header)
      assert.isNull(error)
      assert.isUndefined(value)
      assert.equal(await parseGatewayToolMode(header), 'eager')
      assert.equal(await parseGatewayToolMode(header, 'lazy'), 'lazy')
    }
  })

  test('refuses a header that names no mode the gateway has', async ({ assert }) => {
    for (const header of [
      'sometimes',
      'lazyy',
      'laz',
      'lazy, eager',
      'eager lazy',
      '"lazy"',
      '0',
    ]) {
      const [error] = await gatewayToolModeValidator.tryValidate(header)
      assert.isNotNull(error)
      assert.isNull(await parseGatewayToolMode(header, 'lazy'))
    }
    for (const header of [0, 1, true, ['lazy'], {}]) {
      const [error] = await gatewayToolModeValidator.tryValidate(header)
      assert.isNotNull(error)
    }
  })
})

test.group('vine: gateway namespaced tool name', () => {
  test('splits a name at its first separator', async ({ assert }) => {
    for (const [name, slug, toolName] of [
      ['weather__get_forecast', 'weather', 'get_forecast'],
      ['weather__get__forecast', 'weather', 'get__forecast'],
      ['weather___get_forecast', 'weather', '_get_forecast'],
      ['_weather__get_forecast', '_weather', 'get_forecast'],
      ['weather__', 'weather', ''],
      [' __tool', ' ', 'tool'],
      ['Not A Slug__tool', 'Not A Slug', 'tool'],
      ['multi\nline__tool\nname', 'multi\nline', 'tool\nname'],
      [`${'x'.repeat(4000)}__tool`, 'x'.repeat(4000), 'tool'],
    ]) {
      assert.deepEqual(await parseNamespacedTool(name), { slug, toolName })
      assert.deepEqual(await namespacedToolValidator.validate(name), { slug, toolName })
    }
  })

  test('refuses a name without a separator or without a slug before it', async ({ assert }) => {
    for (const name of [
      '',
      ' ',
      '_',
      '__',
      '___',
      '__tool',
      '___tool',
      '__weather__tool',
      'weather',
      'weather_tool',
      'weather_',
    ]) {
      assert.isNull(await parseNamespacedTool(name), JSON.stringify(name))
    }
    for (const name of notStrings) {
      const [error] = await namespacedToolValidator.tryValidate(name)
      assert.isNotNull(error)
    }
  })
})

test.group('vine: lazy gateway tool_search arguments', () => {
  test('trims the slug and the query and defaults the limit to 10', async ({ assert }) => {
    assert.deepEqual(await parseToolSearchInput({ mcp: ' issues ', query: ' create issue ' }), {
      mcp: 'issues',
      query: 'create issue',
      limit: 10,
    })
    assert.deepEqual(
      await parseToolSearchInput({ mcp: 'issues', query: 'q', limit: 20, extra: true }),
      { mcp: 'issues', query: 'q', limit: 20 }
    )
    assert.deepEqual(
      await toolSearchValidator.validate({
        mcp: 'x'.repeat(120),
        query: 'q'.repeat(200),
        limit: 1,
      }),
      { mcp: 'x'.repeat(120), query: 'q'.repeat(200), limit: 1 }
    )
    // The bounds apply to what is left after trimming.
    assert.isObject(
      await parseToolSearchInput({ mcp: ` ${'x'.repeat(120)} `, query: ` ${'q'.repeat(200)} ` })
    )
  })

  test('tells the agent what the slug must be', async ({ assert }) => {
    assert.equal(await parseToolSearchInput(undefined), MCP_MESSAGE)
    assert.equal(await parseToolSearchInput({}), MCP_MESSAGE)
    for (const mcp of ['', ' ', ' \n\t', 'x'.repeat(121), ` ${'x'.repeat(121)} `, ...notStrings]) {
      assert.equal(await parseToolSearchInput({ mcp, query: 'issue' }), MCP_MESSAGE)
    }
  })

  test('tells the agent what the query must be', async ({ assert }) => {
    assert.equal(await parseToolSearchInput({ mcp: 'issues' }), QUERY_MESSAGE)
    for (const query of ['', ' ', 'q'.repeat(201), ` ${'q'.repeat(201)} `, ...notStrings]) {
      assert.equal(await parseToolSearchInput({ mcp: 'issues', query }), QUERY_MESSAGE)
    }
  })

  test('takes an integer from 1 to 20 as limit, and nothing that only looks like one', async ({
    assert,
  }) => {
    for (const limit of [1, 2, 10, 20, 5.0]) {
      assert.deepEqual(await parseToolSearchInput({ mcp: 'issues', query: 'q', limit }), {
        mcp: 'issues',
        query: 'q',
        limit,
      })
    }
    for (const limit of [
      0,
      -1,
      21,
      1.5,
      1e21,
      Number.NaN,
      Number.POSITIVE_INFINITY,
      '5',
      '',
      ' ',
      null,
      true,
      false,
      [5],
      [],
      {},
    ]) {
      assert.equal(
        await parseToolSearchInput({ mcp: 'issues', query: 'q', limit }),
        LIMIT_MESSAGE,
        JSON.stringify(limit)
      )
    }
  })

  test('reports the first wrong argument only', async ({ assert }) => {
    assert.equal(await parseToolSearchInput({ mcp: '', query: '', limit: 0 }), MCP_MESSAGE)
    assert.equal(await parseToolSearchInput({ mcp: 'issues', query: '', limit: 0 }), QUERY_MESSAGE)
    assert.equal(await parseToolSearchInput({ mcp: 'issues', query: 'q', limit: 0 }), LIMIT_MESSAGE)
  })
})

test.group('vine: lazy gateway call_tool arguments', () => {
  test('trims the slug and the tool name and makes the arguments optional', async ({ assert }) => {
    assert.deepEqual(await parseCallToolInput({ mcp: ' issues ', tool: ' create_issue ' }), {
      mcp: 'issues',
      tool: 'create_issue',
      arguments: undefined,
    })
    assert.isObject(await parseCallToolInput({ mcp: 'issues', tool: 't'.repeat(128) }))
    assert.isObject(await parseCallToolInput({ mcp: 'issues', tool: ` ${'t'.repeat(128)} ` }))
  })

  test('hands the upstream tool the very object the agent sent', async ({ assert }) => {
    const sent = { 'title': '', 'nested': { list: [1, null, { deep: true }] }, '': 'empty key' }
    const input = await parseCallToolInput({ mcp: 'issues', tool: 'create_issue', arguments: sent })

    assert.isObject(input)
    assert.strictEqual((input as { arguments: unknown }).arguments, sent)
    assert.deepEqual(sent, {
      'title': '',
      'nested': { list: [1, null, { deep: true }] },
      '': 'empty key',
    })

    const [error] = await callToolValidator.tryValidate({
      mcp: 'issues',
      tool: 'create_issue',
      arguments: {},
    })
    assert.isNull(error)
  })

  test('tells the agent which of the slug, the tool and the arguments is wrong', async ({
    assert,
  }) => {
    assert.equal(await parseCallToolInput(undefined), MCP_MESSAGE)
    for (const mcp of ['', ' ', 'x'.repeat(121), ...notStrings]) {
      assert.equal(await parseCallToolInput({ mcp, tool: 'create_issue' }), MCP_MESSAGE)
    }

    assert.equal(await parseCallToolInput({ mcp: 'issues' }), TOOL_MESSAGE)
    for (const tool of ['', ' ', 't'.repeat(129), ` ${'t'.repeat(129)} `, ...notStrings]) {
      assert.equal(await parseCallToolInput({ mcp: 'issues', tool }), TOOL_MESSAGE)
    }

    // Null is an argument the agent sent, unlike an argument left out.
    for (const sent of [null, [], [{}], 'text', '', ' ', 0, 1, true, false]) {
      assert.equal(
        await parseCallToolInput({ mcp: 'issues', tool: 'create_issue', arguments: sent }),
        ARGUMENTS_MESSAGE,
        JSON.stringify(sent)
      )
    }

    assert.equal(await parseCallToolInput({ mcp: '', tool: '', arguments: [] }), MCP_MESSAGE)
    assert.equal(await parseCallToolInput({ mcp: 'issues', tool: '', arguments: [] }), TOOL_MESSAGE)
  })
})
