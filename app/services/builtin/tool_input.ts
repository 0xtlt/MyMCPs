import type { VineValidator } from '@vinejs/vine'
import type { Infer, SchemaTypes } from '@vinejs/vine/types'
import { BuiltinToolError, type BuiltinTool } from '#services/builtin/definition'

/**
 * Check what an agent passed to a tool. It is told about one argument at a
 * time: the first that is wrong, in the order the schema lists them.
 * `context` is the validation's metadata, for the rules that depend on the
 * account the tool runs for.
 */
export async function toolInput<Schema extends SchemaTypes>(
  validator: VineValidator<Schema, any>,
  args: unknown,
  context?: Record<string, any>
): Promise<Infer<Schema>> {
  const [error, input] = await validator.tryValidate(args, { meta: context })
  if (error) {
    throw new BuiltinToolError(error.messages[0].message)
  }
  return input
}

type ToolDefinition<Context, Schema extends SchemaTypes> = Omit<
  BuiltinTool<Context>,
  'input' | 'run'
> & {
  input: VineValidator<Schema, any>
  /** `input` is the arguments once they passed the validator. */
  run: (input: Infer<Schema>, context: Context) => Promise<unknown>
}

/** Define a tool whose `run` only ever sees arguments that passed its validator. */
export function builtinTool<Context extends Record<string, any>, Schema extends SchemaTypes>(
  tool: ToolDefinition<Context, Schema>
): BuiltinTool<Context> {
  return {
    ...tool,
    run: async (args, context) => tool.run(await toolInput(tool.input, args, context), context),
  }
}
