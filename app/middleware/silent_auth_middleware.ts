import type { HttpContext } from '@adonisjs/core/http'
import type { NextFn } from '@adonisjs/core/types/http'
import {
  forgetSessionLogin,
  isSessionStampExpired,
  readSessionStamp,
  stampSession,
  touchSessionStamp,
} from '#services/session_stamp'

/**
 * Silent auth middleware can be used as a global middleware to silent check
 * if the user is logged-in or not.
 *
 * The request continues as usual, even when the user is not logged-in.
 *
 * A session only counts as logged-in while its stamp is valid. Otherwise its
 * login is dropped, which leaves the remember-me cookie, revocable on the
 * server, as the only way to carry on without signing in again.
 */
export default class SilentAuthMiddleware {
  async handle(ctx: HttpContext, next: NextFn) {
    const { auth, session } = ctx
    const guard = auth.use('web')
    const loginKey = guard.sessionKeyName

    /**
     * Sessions from before stamps existed, idle sessions, and sessions past
     * their absolute lifetime.
     */
    const hasLogin = session.has(loginKey)
    let stamp = hasLogin ? await readSessionStamp(session) : null
    if (hasLogin && (!stamp || isSessionStampExpired(stamp, session))) {
      forgetSessionLogin(session, loginKey)
      stamp = null
    }

    await auth.check()

    /**
     * The user signed out or changed their password after this session was
     * stamped. The guard caches its verdict for the request, so clear that
     * to have it try the remember-me cookie instead.
     */
    if (stamp && guard.isAuthenticated && stamp.version !== guard.user!.sessionVersion) {
      forgetSessionLogin(session, loginKey)
      stamp = null
      guard.authenticationAttempted = false
      await auth.check()
    }

    if (guard.viaRemember) {
      stampSession(session, guard.user!)
    } else if (guard.isAuthenticated && stamp) {
      touchSessionStamp(session, stamp)
    }

    return next()
  }
}
