import env from '#start/env'
import { defineConfig, stores } from '@adonisjs/limiter'

const limiterConfig = defineConfig({
  default: env.get('NODE_ENV') === 'test' ? 'memory' : 'database',
  stores: {
    /**
     * SQLite is already the application's durable production store. Keeping
     * rate-limit counters there also makes increments atomic across workers.
     */
    database: stores.database({
      tableName: 'rate_limits',
    }),

    /**
     * Tests use an isolated in-memory store and clear it between cases.
     */
    memory: stores.memory({}),
  },
})

export default limiterConfig

declare module '@adonisjs/limiter/types' {
  export interface LimitersList extends InferLimiters<typeof limiterConfig> {}
}
