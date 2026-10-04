/**
 * MCP servers that MyMCPs implements itself, for services with no MCP a
 * self-hosted gateway is allowed to use. Import-free so validators can use it.
 */
export const BUILTIN_MCP_KEYS = ['strava'] as const

export type BuiltinMcpKey = (typeof BUILTIN_MCP_KEYS)[number]
