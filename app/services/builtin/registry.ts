import type Mcp from '#models/mcp'
import type { BuiltinMcpDefinition } from '#services/builtin/definition'
import { BUILTIN_MCP_KEYS, type BuiltinMcpKey } from '#services/builtin/keys'
import { stravaMcp } from '#services/builtin/strava/index'

const BUILTIN_MCPS: Record<BuiltinMcpKey, BuiltinMcpDefinition> = {
  strava: stravaMcp,
}

export function builtinMcp(key: string | null | undefined): BuiltinMcpDefinition | null {
  return BUILTIN_MCP_KEYS.includes(key as BuiltinMcpKey) ? BUILTIN_MCPS[key as BuiltinMcpKey] : null
}

export function requireBuiltinMcp(mcp: Mcp): BuiltinMcpDefinition {
  const definition = builtinMcp(mcp.builtinKey)
  if (!definition) {
    throw new Error(`Unknown built-in MCP: ${mcp.builtinKey ?? 'none'}`)
  }
  return definition
}
