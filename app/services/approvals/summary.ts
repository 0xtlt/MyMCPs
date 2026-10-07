import type Mcp from '#models/mcp'
import type { ApprovalDetail, ApprovalSummary } from '#services/builtin/definition'

/** A summary as it is kept with its request. */
export type SavedApprovalSummary = ApprovalSummary & {
  /**
   * Whether MyMCPs knows what the tool does and read the call itself. When it
   * does not, the summary is the arguments as they are and nothing more.
   */
  interpreted: boolean
  /** What the MCP says its tool does. Written by the MCP, never by the agent. */
  toolDescription: string | null
}

const MAX_DETAILS = 60
const MAX_VALUE_CHARS = 1000
const MAX_DESCRIPTION_CHARS = 2000

function shown(value: string) {
  return value.length > MAX_VALUE_CHARS
    ? `${value.slice(0, MAX_VALUE_CHARS)}… (${value.length - MAX_VALUE_CHARS} more characters)`
    : value
}

function leafValue(value: unknown) {
  if (typeof value === 'string') return value === '' ? '(empty text)' : shown(value)
  if (Array.isArray(value)) return '(empty list)'
  if (value && typeof value === 'object') return '(empty object)'
  return String(value)
}

/**
 * Every value of the arguments on a row of its own, named by where it is:
 * `budget.amount`, `keywords[0].text`. No sentence is made out of them, so
 * the rows say what the agent sent and nothing it would like them to say.
 */
export function argumentDetails(args: Record<string, unknown> | undefined) {
  const details: ApprovalDetail[] = []
  let total = 0

  const visit = (value: unknown, path: string) => {
    const children =
      Array.isArray(value) && value.length > 0
        ? value.map((item, index) => [item, `${path}[${index}]`] as const)
        : value && typeof value === 'object' && Object.keys(value).length > 0
          ? Object.entries(value).map(
              ([key, item]) => [item, path ? `${path}.${key}` : key] as const
            )
          : null
    if (children) {
      for (const [item, childPath] of children) visit(item, childPath)
      return
    }

    total += 1
    if (details.length < MAX_DETAILS) {
      details.push({ label: path, value: leafValue(value) })
    }
  }

  for (const [key, value] of Object.entries(args ?? {})) visit(value, key)
  return { details, hidden: total - details.length }
}

/**
 * The summary of a call to a tool MyMCPs cannot read: one of a connected MCP,
 * or a built-in one that has nothing to add to its arguments.
 */
export function argumentsSummary(
  mcp: Mcp,
  toolName: string,
  args: Record<string, unknown> | undefined,
  toolDescription: string | undefined
): SavedApprovalSummary {
  const { details, hidden } = argumentDetails(args)
  return {
    interpreted: false,
    title: `Run the tool "${toolName}" of ${mcp.name}`,
    details,
    warnings:
      hidden > 0
        ? [
            `Only the first ${MAX_DETAILS} values are listed, and ${hidden} more are not. Read the exact arguments before you decide.`,
          ]
        : undefined,
    toolDescription: toolDescription?.trim().slice(0, MAX_DESCRIPTION_CHARS) || null,
  }
}
