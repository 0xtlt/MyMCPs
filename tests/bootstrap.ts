import { assert } from '@japa/assert'
import { ApiClient, apiClient } from '@japa/api-client'
import app from '@adonisjs/core/services/app'
import type { Config, PluginFn } from '@japa/runner/types'
import { pluginAdonisJS } from '@japa/plugin-adonisjs'
import { dbAssertions } from '@adonisjs/lucid/plugins/db'
import testUtils from '@adonisjs/core/services/test_utils'
import { browserClient, decoratorsCollection } from '@japa/browser-client'
import { authBrowserClient } from '@adonisjs/auth/plugins/browser_client'
import { authApiClient } from '@adonisjs/auth/plugins/api_client'
import { sessionBrowserClient } from '@adonisjs/session/plugins/browser_client'
import { sessionApiClient } from '@adonisjs/session/plugins/api_client'
import { shieldApiClient } from '@adonisjs/shield/plugins/api_client'
import { inertiaApiClient } from '@adonisjs/inertia/plugins/api_client'
import { closeTestDatabase, prepareTestDatabase } from '#tests/helpers/database'
import { SESSION_STAMP_KEY, createSessionStamp } from '#services/session_stamp'
import type User from '#models/user'

/**
 * This file is imported by the "bin/test.ts" entrypoint file
 */

/**
 * "loginAs" only writes the guard's own session key, while the application
 * also stamps every session it authenticates. Stamp the sessions "loginAs"
 * creates the same way, for both the API and the browser client.
 */
const stampedLoginAs: PluginFn = () => {
  ApiClient.setup(async (request) => {
    if (request.authData) {
      const user = request.authData.args[0] as User
      request.withSession({ [SESSION_STAMP_KEY]: createSessionStamp(user) })
    }
  })

  decoratorsCollection.register({
    context(context) {
      const loginAs = context.loginAs

      context.loginAs = async function (user) {
        await loginAs.call(context, user)
        await context.setSession({ [SESSION_STAMP_KEY]: createSessionStamp(user) })
      }
    },
  })
}

/**
 * Configure Japa plugins in the plugins array.
 * Learn more - https://japa.dev/docs/runner-config#plugins-optional
 */
export const plugins: Config['plugins'] = [
  assert(),
  pluginAdonisJS(app),
  dbAssertions(app),
  apiClient(),
  inertiaApiClient(app),
  authApiClient(app),
  sessionApiClient(app),
  shieldApiClient(),
  browserClient({ runInSuites: ['browser'] }),
  sessionBrowserClient(app),
  authBrowserClient(app),
  stampedLoginAs,
]

/**
 * Configure lifecycle function to run before and after all the
 * tests.
 *
 * The setup functions are executed before all the tests
 * The teardown functions are executed after all the tests
 */
export const runnerHooks: Required<Pick<Config, 'setup' | 'teardown'>> = {
  setup: [prepareTestDatabase],
  teardown: [closeTestDatabase],
}

/**
 * Configure suites by tapping into the test suite instance.
 * Learn more - https://japa.dev/docs/test-suites#lifecycle-hooks
 */
export const configureSuite: Config['configureSuite'] = (suite) => {
  if (['browser', 'functional', 'e2e'].includes(suite.name)) {
    return suite.setup(() => testUtils.httpServer().start())
  }
}
