/// MCP servers that MyMCPs implements itself, for services with no MCP a
/// self-hosted gateway is allowed to use.
pub const BUILTIN_MCP_KEYS: &[&str] = &["strava", "icloud-mail", "google-ads"];

pub fn is_builtin_mcp_key(key: &str) -> bool {
    BUILTIN_MCP_KEYS.contains(&key)
}
