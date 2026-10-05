import string from '@adonisjs/core/helpers/string'
import type { HttpContext } from '@adonisjs/core/http'
import type { Session } from '@adonisjs/session'
import type { Infer } from '@vinejs/vine/types'
import type User from '#models/user'
import { sessionStampValidator } from '#validators/session'

/**
 * The cookie store keeps a session entirely in the browser, where the server
 * can neither expire nor delete it: a copied session cookie would stay valid
 * forever. Every authenticated session therefore carries a stamp, checked on
 * each request by the silent auth middleware. It records the user's session
 * version at sign-in, which the user bumps to retire their sessions, and the
 * times that bound how long the session lives.
 */
export const SESSION_STAMP_KEY = 'auth_stamp'

/**
 * However active, a session has to be re-established this long after sign-in.
 */
const ABSOLUTE_LIFETIME = string.milliseconds.parse('24 hours')

export type SessionStamp = Infer<typeof sessionStampValidator>

export function createSessionStamp(user: User, now = Date.now()): SessionStamp {
  return { version: user.sessionVersion, authenticatedAt: now, lastSeenAt: now }
}

/**
 * Mark the session as authenticated under the user's current session version.
 */
export function stampSession(session: Session, user: User) {
  session.put(SESSION_STAMP_KEY, createSessionStamp(user))
}

/**
 * The stamp of the session, or null when it carries none or one the app did
 * not write.
 */
export async function readSessionStamp(session: Session): Promise<SessionStamp | null> {
  try {
    return await sessionStampValidator.validate(session.get(SESSION_STAMP_KEY))
  } catch {
    return null
  }
}

/**
 * Whether the session sat idle for longer than the configured session age, or
 * has outlived the absolute lifetime.
 */
export function isSessionStampExpired(stamp: SessionStamp, session: Session, now = Date.now()) {
  const idleLifetime = string.seconds.parse(session.config.age) * 1000
  return now - stamp.lastSeenAt > idleLifetime || now - stamp.authenticatedAt > ABSOLUTE_LIFETIME
}

export function touchSessionStamp(session: Session, stamp: SessionStamp, now = Date.now()) {
  session.put(SESSION_STAMP_KEY, { ...stamp, lastSeenAt: now })
}

/**
 * Drop the login a session carries, keeping the rest of its data.
 */
export function forgetSessionLogin(session: Session, loginKey: string) {
  session.forget(loginKey)
  session.forget(SESSION_STAMP_KEY)
}

/**
 * Sign the user in on this browser, remembered, with a stamped session.
 */
export async function signIn({ auth, session }: HttpContext, user: User) {
  await auth.use('web').login(user, true)
  stampSession(session, user)
}
