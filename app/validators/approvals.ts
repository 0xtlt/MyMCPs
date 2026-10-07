/**
 * Vine schemas for tool approvals: what the pages submit, what a link names,
 * and what the app reads back from its own database.
 */
import vine from '@vinejs/vine'
import { toolVine } from '#validators/builtin_tools'

/** Whether a tool runs when an agent calls it, or waits for a person first. */
export const TOOL_APPROVAL_MODES = ['auto', 'ask'] as const

/**
 * The modes saved for an MCP, by tool name. Tool names belong to the MCP and
 * may be anything, so nothing here trims or rewrites them.
 */
export const savedToolApprovalsValidator = toolVine.create(
  toolVine.record(toolVine.enum(TOOL_APPROVAL_MODES))
)

/** The tools page sends every tool it shows with the mode chosen for it. */
export const updateToolApprovalsValidator = toolVine.create({
  tools: toolVine
    .array(
      toolVine.object({
        name: toolVine.string().minLength(1).maxLength(254),
        mode: toolVine.enum(TOOL_APPROVAL_MODES),
      })
    )
    .maxLength(2000),
})

/** `:id` of an approval link: 24 random bytes in base64url. */
export const approvalParamsValidator = vine.create({
  id: vine.string().regex(/^[A-Za-z0-9_-]{32}$/),
})

export const approvalDecisionValidator = vine.create({
  decision: vine.enum(['approve', 'deny'] as const),
})

/**
 * What MyMCPs read in a call when it was made, as stored with the request.
 * Values are kept as written: an empty one is a value.
 */
export const savedApprovalSummaryValidator = toolVine.create({
  interpreted: toolVine.boolean({ strict: true }),
  title: toolVine.string(),
  details: toolVine.array(
    toolVine.object({
      label: toolVine.string(),
      value: toolVine.string(),
      before: toolVine.string().optional(),
    })
  ),
  warnings: toolVine.array(toolVine.string()).optional(),
  toolDescription: toolVine.string().nullable(),
})
