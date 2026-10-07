import type Mcp from '#models/mcp'
import type { BuiltinMcpDefinition, BuiltinOauthMcpDefinition } from '#services/builtin/definition'
import { BUILTIN_MCP_KEYS, type BuiltinMcpKey } from '#services/builtin/keys'
import { googleAdsMcp } from '#services/builtin/google_ads/index'
import { icloudMailMcp } from '#services/builtin/icloud_mail/index'
import { stravaMcp } from '#services/builtin/strava/index'

const BUILTIN_MCPS: Record<BuiltinMcpKey, BuiltinMcpDefinition> = {
  'strava': stravaMcp,
  'icloud-mail': icloudMailMcp,
  'google-ads': googleAdsMcp,
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

export function requireBuiltinOauthMcp(mcp: Mcp): BuiltinOauthMcpDefinition {
  const definition = requireBuiltinMcp(mcp)
  if (!definition.oauth) {
    throw new Error(`${definition.name} does not sign in with OAuth`)
  }
  return definition
}
