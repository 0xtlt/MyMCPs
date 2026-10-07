import type Mcp from '#models/mcp'
import { builtinMcp } from '#services/builtin/registry'
import type { UpstreamTool } from '#services/upstream/http_client'
import { savedToolApprovalsValidator } from '#validators/approvals'

/** `auto` runs the call. `ask` holds it until a person approves it in MyMCPs. */
export type ToolApprovalMode = 'auto' | 'ask'

export const APPROVAL_NOTE =
  'Needs approval: the first call is not run and returns a link for a person to approve in MyMCPs. Once they have, call the tool again with the same arguments.'

/**
 * The modes saved for this MCP, by tool name. `null` when what is saved
 * cannot be read, which no tool may take as leave to run.
 */
export async function savedToolApprovals(
  mcp: Mcp
): Promise<Record<string, ToolApprovalMode> | null> {
  if (!mcp.toolApprovals) return {}

  let parsed: unknown
  try {
    parsed = JSON.parse(mcp.toolApprovals)
  } catch {
    return null
  }
  const [unreadable, saved] = await savedToolApprovalsValidator.tryValidate(parsed)
  return unreadable ? null : saved
}

/**
 * What a tool does until the admin chooses: built-in tools that commit money
 * or go live ask, and every other tool runs.
 */
export function defaultToolApproval(mcp: Mcp, toolName: string): ToolApprovalMode {
  const tool = builtinMcp(mcp.builtinKey)?.tools.find((candidate) => candidate.name === toolName)
  return tool?.approval === 'ask' ? 'ask' : 'auto'
}

function modeOf(
  saved: Record<string, ToolApprovalMode> | null,
  mcp: Mcp,
  toolName: string
): ToolApprovalMode {
  if (!saved) return 'ask'
  return Object.hasOwn(saved, toolName) ? saved[toolName] : defaultToolApproval(mcp, toolName)
}

export async function toolApprovalMode(mcp: Mcp, toolName: string): Promise<ToolApprovalMode> {
  return modeOf(await savedToolApprovals(mcp), mcp, toolName)
}

/** The mode of each of these tools, in one read of what is saved. */
export async function toolApprovalModes(mcp: Mcp, toolNames: readonly string[]) {
  const saved = await savedToolApprovals(mcp)
  return new Map(toolNames.map((name) => [name, modeOf(saved, mcp, name)]))
}

/**
 * Keep the choices that differ from the defaults, so that a tool the admin
 * never touched follows its default if a later version changes it.
 */
export function assignToolApprovals(
  mcp: Mcp,
  choices: ReadonlyArray<{ name: string; mode: ToolApprovalMode }>
) {
  const saved = choices.filter(({ name, mode }) => mode !== defaultToolApproval(mcp, name))
  mcp.toolApprovals =
    saved.length > 0
      ? JSON.stringify(Object.fromEntries(saved.map(({ name, mode }) => [name, mode])))
      : null
}

/** Tell agents which tools wait for a person, before they plan around them. */
export async function withApprovalNotes<Listed extends UpstreamTool>(
  mcp: Mcp,
  tools: Listed[]
): Promise<Listed[]> {
  const modes = await toolApprovalModes(
    mcp,
    tools.map((tool) => tool.name)
  )
  return tools.map((tool) =>
    modes.get(tool.name) === 'ask'
      ? {
          ...tool,
          description: [tool.description, APPROVAL_NOTE].filter(Boolean).join('\n\n'),
        }
      : tool
  )
}
