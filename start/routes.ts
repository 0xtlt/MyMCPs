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
          })
          .use(middleware.admin())
      })
      .use(middleware.auth())
  })
  .use(middleware.needsSetup())
