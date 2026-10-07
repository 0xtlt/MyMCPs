import { test } from '@japa/runner'
import { BUILTIN_MCP_KEYS } from '#services/builtin/keys'
import { builtinMcp } from '#services/builtin/registry'

type JsonSchema = Record<string, any>

/** What a tool may say about an argument beyond its type. Whatever it says, its validator enforces. */
const BOUNDS = ['minimum', 'maximum', 'maxLength', 'minItems', 'maxItems', 'enum'] as const

/** Arguments a validator checks although the tool neither advertises nor uses them. */
const UNADVERTISED: Record<string, string[]> = {
  // Strava returns the efforts on a segment on one page.
  'strava list_segment_efforts': ['page'],
}

/** Vine describes a choice among words by the words alone. */
function typeOf(schema: JsonSchema) {
  return schema.type ?? (schema.enum ? 'string' : undefined)
}

const tools = BUILTIN_MCP_KEYS.flatMap((key) =>
  builtinMcp(key)!.tools.map((tool) => ({ id: `${key} ${tool.name}`, tool }))
)

/**
 * The JSON Schema of a tool is written by hand, for its descriptions and
 * defaults. Its validator can describe itself in the same terms: the two must
 * agree, or agents are told one thing and held to another.
 */
test.group('Built-in tools: advertised and enforced arguments', () => {
  for (const { id, tool } of tools) {
    test(`${id} checks the arguments it advertises`, ({ assert }) => {
      const advertised: JsonSchema = tool.inputSchema
      const enforced: JsonSchema = tool.input.toJSONSchema()

      assert.equal(advertised.type, 'object')
      assert.equal(enforced.type, 'object')
      assert.sameMembers(Object.keys(enforced.properties), [
        ...Object.keys(advertised.properties ?? {}),
        ...(UNADVERTISED[id] ?? []),
      ])
      assert.sameMembers(enforced.required, advertised.required ?? [])

      for (const [name, described] of Object.entries<JsonSchema>(advertised.properties ?? {})) {
        const checked: JsonSchema = enforced.properties[name]
        assert.equal(typeOf(checked), described.type, `type of ${name}`)
        for (const bound of BOUNDS) {
          if (bound in described) {
            assert.deepEqual(checked[bound], described[bound], `${bound} of ${name}`)
          }
        }
        if (described.items) {
          assert.equal(typeOf(checked.items), described.items.type, `items of ${name}`)
          assert.deepEqual(checked.items.enum, described.items.enum, `items of ${name}`)
        }
      }
    })
  }

  test('compares every tool of every built-in MCP', ({ assert }) => {
    assert.lengthOf(tools, 58)
    assert.lengthOf(new Set(tools.map(({ id }) => id)), 58)
    for (const id of Object.keys(UNADVERTISED)) {
      assert.include(
        tools.map((tool) => tool.id),
        id
      )
    }
  })
})
