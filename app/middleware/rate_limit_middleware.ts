import type { HttpContext } from '@adonisjs/core/http'
import type { NextFn } from '@adonisjs/core/types/http'

type RateLimitOptions = {
  name: string
  limit: number
  windowMs: number
}

type Bucket = {
  count: number
  resetAt: number
}

const MAX_BUCKETS = 10_000
const PRUNE_INTERVAL_MS = 60_000
const buckets = new Map<string, Bucket>()
const refunds = new WeakMap<HttpContext, () => void>()
let nextPruneAt = 0

function bucketKey(name: string, ip: string | undefined) {
  return `${name}:${ip ?? 'unknown'}`
}

export function refundRateLimit(ctx: HttpContext) {
  const refund = refunds.get(ctx)
  refunds.delete(ctx)
  refund?.()
}

function pruneBuckets(now: number) {
  if (now >= nextPruneAt) {
    for (const [key, bucket] of buckets) {
      if (bucket.resetAt <= now) {
        buckets.delete(key)
      }
    }
    nextPruneAt = now + PRUNE_INTERVAL_MS
  }

  while (buckets.size >= MAX_BUCKETS) {
    const oldestKey = buckets.keys().next().value as string | undefined
    if (!oldestKey) break
    buckets.delete(oldestKey)
  }
}

/**
 * Small fixed-window limiter for expensive anonymous credential endpoints.
 * MyMCPs runs as one Node process, so process-local state protects the deployed
 * instance without adding an external datastore.
 */
export default class RateLimitMiddleware {
  async handle(ctx: HttpContext, next: NextFn, options: RateLimitOptions) {
    const now = Date.now()
    pruneBuckets(now)

    const key = bucketKey(options.name, ctx.request.ip())
    const current = buckets.get(key)
    const bucket =
      current && current.resetAt > now ? current : { count: 0, resetAt: now + options.windowMs }

    bucket.count += 1
    buckets.delete(key)
    buckets.set(key, bucket)

    if (bucket.count > options.limit) {
      const retryAfterSeconds = Math.max(1, Math.ceil((bucket.resetAt - now) / 1_000))
      ctx.response.header('Retry-After', String(retryAfterSeconds))
      return ctx.response.status(429).json({
        error: 'too_many_requests',
        message: 'Too many attempts. Wait before trying again.',
      })
    }

    refunds.set(ctx, () => {
      if (buckets.get(key) !== bucket) return
      bucket.count -= 1
      if (bucket.count === 0) {
        buckets.delete(key)
      }
    })

    try {
      return await next()
    } finally {
      refunds.delete(ctx)
    }
  }
}
