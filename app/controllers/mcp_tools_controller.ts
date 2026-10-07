import type { HttpContext } from '@adonisjs/core/http'
import Mcp from '#models/mcp'
import {
  assignToolApprovals,
  defaultToolApproval,
  savedToolApprovals,
  type ToolApprovalMode,
} from '#services/approvals/policy'
import { builtinMcp } from '#services/builtin/registry'
import { sanitizeMcpDiagnostic } from '#services/security_redaction'
import { probeUpstream } from '#services/upstream/manager'
import { updateToolApprovalsValidator } from '#validators/approvals'
import { recordIdParamsValidator } from '#validators/route_params'

const MAX_DESCRIPTION_CHARS = 600

type ListedTool = { name: string; description: string | null }

async function findMcp(params: HttpContext['params']) {
  const [, route] = await recordIdParamsValidator.tryValidate(params)
  return route ? Mcp.find(route.id) : null
}

/**
 * The tools an MCP has. A built-in MCP knows them without connecting, so they
 * can be set up before its account is. Any other MCP is asked for its list.
 */
async function listTools(mcp: Mcp): Promise<{ tools: ListedTool[]; error: string | null }> {
  const definition = builtinMcp(mcp.builtinKey)
  if (definition) {
    return {
      tools: definition.tools.map(({ name, description }) => ({ name, description })),
      error: null,
    }
  }

  try {
    const tools = await probeUpstream(mcp)
    return {
      tools: tools.map(({ name, description }) => ({ name, description: description ?? null })),
      error: null,
    }
  } catch (error) {
    return { tools: [], error: sanitizeMcpDiagnostic(error, mcp) ?? 'Unknown error' }
  }
}

/**
 * Which tools of an MCP run when an agent calls them, and which wait for a
 * person. It works the same for every MCP: the gateway holds the call before
 * it reaches the MCP.
 */
export default class McpToolsController {
  async index({ params, inertia, response, session }: HttpContext) {
    const mcp = await findMcp(params)
    if (!mcp) {
      session.flash('error', 'MCP not found')
      return response.redirect().toRoute('mcps.index')
    }

    const [{ tools, error }, saved] = await Promise.all([listTools(mcp), savedToolApprovals(mcp)])
    // A tool with a saved choice stays listed while its MCP cannot be
    // reached, or after the MCP dropped it, so the choice can still be seen.
    const listed = new Set(tools.map((tool) => tool.name))
    const unlisted = Object.keys(saved ?? {}).filter((name) => !listed.has(name))

    const row = (tool: ListedTool, isListed: boolean) => {
      const defaultMode = defaultToolApproval(mcp, tool.name)
      const mode: ToolApprovalMode = saved
        ? Object.hasOwn(saved, tool.name)
          ? saved[tool.name]
          : defaultMode
        : 'ask'
      return {
        name: tool.name,
        description: tool.description?.slice(0, MAX_DESCRIPTION_CHARS) ?? null,
        mode,
        defaultMode,
        isListed,
      }
    }

    return inertia.render('mcps/tools', {
      mcp: { id: mcp.id, name: mcp.name, slug: mcp.slug, isBuiltin: mcp.transport === 'builtin' },
      tools: [
        ...tools.map((tool) => row(tool, true)),
        ...unlisted.sort().map((name) => row({ name, description: null }, false)),
      ],
      listError: error,
      // What is saved could not be read, so every tool asks until it is saved again.
      savedUnreadable: saved === null,
    })
  }

  async update({ params, request, response, session }: HttpContext) {
    const mcp = await findMcp(params)
    if (!mcp) {
      session.flash('error', 'MCP not found')
      return response.redirect().toRoute('mcps.index')
    }

    const { tools } = await request.validateUsing(updateToolApprovalsValidator)
    assignToolApprovals(mcp, tools)
    await mcp.save()

    session.flash('success', 'Tool approvals saved')
    return response.redirect().toRoute('mcps.tools', { id: mcp.id })
  }
}
