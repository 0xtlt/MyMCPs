import limiter from '@adonisjs/limiter/services/main'

/**
 * Requests one access token may send to /mcp per minute. Every JSON-RPC
 * message is its own request and agents send them in bursts, so the allowance
 * is generous: it only stops one token from keeping the upstreams busy.
 */
export const GATEWAY_REQUESTS_PER_MINUTE = 600

/**
 * Counted in memory rather than in the database: the check runs on every
 * gateway request, where a write to SQLite would cost more than it protects.
 */
export const gatewayRateLimiter = limiter.use('memory', {
  requests: GATEWAY_REQUESTS_PER_MINUTE,
  duration: '1 minute',
})
