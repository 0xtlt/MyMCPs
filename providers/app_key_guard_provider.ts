import type { ApplicationService } from '@adonisjs/core/types'

/**
 * APP_KEY values published in this repository. They are long enough to pass
 * validation, but anyone can read them.
 */
const PUBLISHED_APP_KEYS = new Set([
  // Dockerfile build stage: lets `node ace build` load the environment.
  'build-only-not-for-runtime-use',
  // .env.test
  'base64:btJw8RPglRbr2TIbpi4nR2AMmmpc/gO1Weadd2mSSaI=',
])

export function isPublishedAppKey(appKey: string): boolean {
  return PUBLISHED_APP_KEYS.has(appKey.trim())
}

/**
 * Throws when the key is one of the published values.
 */
export function assertPrivateAppKey(appKey: string): void {
  if (!isPublishedAppKey(appKey)) return

  throw new Error(
    'Refusing to start: APP_KEY is a placeholder published in the MyMCPs repository ' +
      '(the Docker build key or the test key), so MCP credentials and sessions would be ' +
      'protected by a key anyone can read. Set APP_KEY to a private value from ' +
      '`node ace generate:key`, or leave it empty in Docker so the entrypoint generates one.'
  )
}

/**
 * Stops the production HTTP server from booting with a published APP_KEY.
 * Registered for the `web` environment only: ace commands keep working with
 * the placeholder, which the Docker build stage relies on.
 */
export default class AppKeyGuardProvider {
  constructor(protected app: ApplicationService) {}

  async boot() {
    if (!this.app.inProduction) return

    const { default: env } = await import('#start/env')
    assertPrivateAppKey(env.get('APP_KEY').release())
  }
}
