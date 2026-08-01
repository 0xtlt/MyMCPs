/*
|--------------------------------------------------------------------------
| Routes file
|--------------------------------------------------------------------------
|
| The routes file is used for defining the HTTP routes.
|
*/

import { middleware } from '#start/kernel'
import { controllers } from '#generated/controllers'
import router from '@adonisjs/core/services/router'

/**
 * First-run only. Once an admin exists, these redirect home.
 */
router
  .group(() => {
    router.get('onboarding', [controllers.Onboarding, 'show']).as('onboarding.show')
    router.post('onboarding', [controllers.Onboarding, 'store']).as('onboarding.store')
  })
  .use(middleware.setupComplete())

/**
 * Agent-facing MCP gateway — bearer access tokens, no session auth.
 */
router
  .group(() => {
    router.post('mcp', [controllers.Gateway, 'handle']).as('gateway.handle')
    router.get('mcp', [controllers.Gateway, 'handle']).as('gateway.handleGet')
  })
  .use(middleware.mcpBearer())

/**
 * App routes — require setup to be finished.
 * Unauthenticated visitors never see a public marketing home.
 */
router
  .group(() => {
    router
      .group(() => {
        router.get('login', [controllers.Session, 'create']).as('session.create')
        router.post('login', [controllers.Session, 'store']).as('session.store')
      })
      .use(middleware.guest())

    router
      .group(() => {
        router.get('invite/:token', [controllers.Invites, 'show']).as('invites.show')
        router.post('invite/:token', [controllers.Invites, 'accept']).as('invites.accept')
      })
      .use(middleware.guest())

    router
      .group(() => {
        router.on('/').renderInertia('home', {}).as('home')
        router.post('logout', [controllers.Session, 'destroy']).as('session.destroy')

        router
          .group(() => {
            router.get('invites', [controllers.Invites, 'index']).as('invites.index')
            router.post('invites', [controllers.Invites, 'store']).as('invites.store')
            router.delete('invites/:id', [controllers.Invites, 'destroy']).as('invites.destroy')
            router
              .delete('members/:id', [controllers.Invites, 'destroyMember'])
              .as('members.destroy')
          })
          .use(middleware.admin())

        router.get('mcps', [controllers.Mcps, 'index']).as('mcps.index')
        router.post('mcps', [controllers.Mcps, 'store']).as('mcps.store')
        router
          .get('mcps/oauth/callback', [controllers.Mcps, 'oauthCallback'])
          .as('mcps.oauthCallback')
        router.get('mcps/:id', [controllers.Mcps, 'show']).as('mcps.show')
        router.put('mcps/:id', [controllers.Mcps, 'update']).as('mcps.update')
        router.delete('mcps/:id', [controllers.Mcps, 'destroy']).as('mcps.destroy')
        router.post('mcps/:id/probe', [controllers.Mcps, 'probe']).as('mcps.probe')
        router.post('mcps/:id/oauth/start', [controllers.Mcps, 'oauthStart']).as('mcps.oauthStart')

        router.get('tokens', [controllers.AccessTokens, 'index']).as('tokens.index')
        router.post('tokens', [controllers.AccessTokens, 'store']).as('tokens.store')
        router.post('tokens/:id/revoke', [controllers.AccessTokens, 'revoke']).as('tokens.revoke')
      })
      .use(middleware.auth())
  })
  .use(middleware.needsSetup())
