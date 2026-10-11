use std::time::Duration;

use mymcps_core::limiter::Limiter;

/// Requests one access token may send to /mcp per minute. Every JSON-RPC
/// message is its own request and agents send them in bursts, so the allowance
/// is generous: it only stops one token from keeping the upstreams busy.
pub const GATEWAY_REQUESTS_PER_MINUTE: u32 = 600;

/// Counted in memory rather than in the database: the check runs on every
/// gateway request, where a write to SQLite would cost more than it protects.
pub fn gateway_rate_limiter() -> Limiter {
    Limiter::memory(GATEWAY_REQUESTS_PER_MINUTE, Duration::from_secs(60))
}
