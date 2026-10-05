/**
 * Vine schemas for what agents send to the MCP gateway, apart from the
 * JSON-RPC messages themselves, which the MCP SDK validates. A blank value is
 * refused or read as absent by every schema here, so they answer the same
 * with or without the app-wide conversion of blank strings to null.
 */
import vine from '@vinejs/vine'
import type { MessagesProviderContact } from '@vinejs/vine/types'

/**
 * An agent that gets an argument wrong reads one sentence saying what that
 * argument must be, whichever rule refused it.
 */
function argumentMessages(messages: Record<string, string>): MessagesProviderContact {
  return {
    getMessage: (defaultMessage, _rule, field) => messages[field.wildCardPath] ?? defaultMessage,
  }
}

/**
 * Vine reads null as a value left out. In the arguments of a tool call it is
 * a value the agent sent, and it is not a valid one.
 */
const notNull = vine.createRule(
  (value, _options, field) => {
    if (value === null) {
      field.report('The {{ field }} field must not be null', 'notNull', field)
    }
  },
  { implicit: true }
)

const mcpSlugArgument = () => vine.string().trim().minLength(1).maxLength(120)

/**
 * The `X-MyMCPs-Tool-Mode` request header. Case and surrounding whitespace
 * are ignored, and a blank header leaves the choice to the instance.
 */
export const gatewayToolModeValidator = vine.create(
  vine
    .enum(['eager', 'lazy'] as const)
    .parse((value) => (typeof value === 'string' ? value.trim().toLowerCase() || undefined : value))
    .optional()
)

/**
 * A tool name of the eager gateway, `<slug>__<tool>`, split at its first
 * separator. The slug cannot be empty; the tool name is the upstream's to
 * judge.
 */
export const namespacedToolValidator = vine.create(
  vine
    .string()
    .regex(/^(?!__).+?__/s)
    .transform((name) => {
      const separator = name.indexOf('__')
      return { slug: name.slice(0, separator), toolName: name.slice(separator + 2) }
    })
)

/**
 * Arguments of the lazy gateway's `tool_search`.
 */
export const toolSearchValidator = vine.create({
  mcp: mcpSlugArgument(),
  query: vine.string().trim().minLength(1).maxLength(200),
  limit: vine
    .number({ strict: true })
    .parse((value) => (value === undefined ? 10 : value))
    .withoutDecimals()
    .range([1, 20]),
})
toolSearchValidator.messagesProvider = argumentMessages({
  mcp: 'mcp must be a non-empty MCP slug of at most 120 characters',
  query: 'query must be non-empty and at most 200 characters',
  limit: 'limit must be an integer between 1 and 20',
})

/**
 * Arguments of the lazy gateway's `call_tool`. What is passed on to the
 * upstream tool is only required to be an object: its content is the
 * upstream's to judge.
 */
export const callToolValidator = vine.create({
  mcp: mcpSlugArgument(),
  tool: vine.string().trim().minLength(1).maxLength(128),
  arguments: vine.object({}).use(notNull()).optional(),
})
callToolValidator.messagesProvider = argumentMessages({
  mcp: 'mcp must be a non-empty MCP slug of at most 120 characters',
  tool: 'tool must be a non-empty upstream tool name of at most 128 characters',
  arguments: 'arguments must be an object when provided',
})
