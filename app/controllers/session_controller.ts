import { createHash } from 'node:crypto'
import User from '#models/user'
import { rateLimitClientKey } from '#services/client_ip'
import { SESSION_STAMP_KEY, signIn } from '#services/session_stamp'
import { loginAddressRateLimiter, loginRateLimiter } from '#start/limiter'
import { oauthReturnToValidator } from '#validators/session'
import { loginValidator } from '#validators/user'
import type { HttpContext } from '@adonisjs/core/http'

export default class SessionController {
  async create({ inertia }: HttpContext) {
    /**
     * Guests land here after signing out or losing their session. The pages
     * they saw before stay in the browser history, encrypted: dropping the
     * key makes them unreadable, including a one-time access token.
     */
    inertia.clearHistory()
    return inertia.render('auth/login', {})
  }

  async store(ctx: HttpContext) {
    const { request, response, session } = ctx
    const { email, password } = await request.validateUsing(loginValidator)

    const client = rateLimitClientKey(request.ip())
    // Hashed to keep limiter keys short and free of email addresses.
    const account = createHash('sha256').update(email.toLowerCase()).digest('hex')
    const addressKey = `login-address:${client}`
    const accountKey = `login-account:${client}:${account}`

    /**
     * Attempts are counted before the password is checked, so a burst of
     * parallel guesses cannot all slip in ahead of the first recorded failure.
     */
    await loginAddressRateLimiter.consume(addressKey)
    await loginRateLimiter.consume(accountKey)

    const user = await User.verifyCredentials(email, password)

    /**
     * A sign-in clears the budget of its own account only. The address-wide
     * budget just gets back the attempt this request was charged, so signing
     * in to one account never buys more guesses against another.
     */
    await loginRateLimiter.delete(accountKey)
    await loginAddressRateLimiter.decrement(addressKey)

    await signIn(ctx, user)
    const [, oauthReturnTo] = await oauthReturnToValidator.tryValidate(
      session.pull('oauthReturnTo')
    )
    if (oauthReturnTo) {
      // The path is complete. Forwarding this request's query string would
      // append it after the last authorization parameter.
      return response.redirect().withQs(false).toPath(oauthReturnTo)
    }
    response.redirect().toRoute('home')
  }

  async destroy({ auth, response, session }: HttpContext) {
    /**
     * A cookie-store session cannot be deleted on the server, so signing out
     * retires every session issued to the user so far, a copy of this one
     * included. Their other browsers carry on with their remember-me cookie.
     */
    await auth.user!.invalidateSessions()
    await auth.use('web').logout()
    session.forget(SESSION_STAMP_KEY)
    response.redirect().toRoute('session.create')
  }
}
