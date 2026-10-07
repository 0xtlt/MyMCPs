/**
 * Vine schemas for values the app reads back from the browser session.
 */
import vine from '@vinejs/vine'

/**
 * The stamp of an authenticated session: the user's session version at
 * sign-in and two millisecond timestamps. Strict numbers, because the server
 * wrote them as numbers and anything else is not a stamp it issued.
 */
export const sessionStampValidator = vine.create({
  version: vine.number({ strict: true }).withoutDecimals().positive(),
  authenticatedAt: vine.number({ strict: true }).withoutDecimals().positive(),
  lastSeenAt: vine.number({ strict: true }).withoutDecimals().positive(),
})

/**
 * Where sign-in returns to when it interrupted a gateway authorization
 * request. The app only ever stores a path on its own authorization endpoint.
 */
export const oauthReturnToValidator = vine.create(
  vine.string().startsWith('/authorize?').maxLength(1536)
)

/**
 * Where sign-in returns to when it interrupted someone opening an approval
 * link. The app only ever stores the path of one request.
 */
export const approvalReturnToValidator = vine.create(
  vine.string().regex(/^\/approvals\/[A-Za-z0-9_-]{32}$/)
)

/**
 * The id of a record flashed for the next page, such as the MCP whose dialog
 * should reopen.
 */
export const flashedRecordIdValidator = vine.create(
  vine.number({ strict: true }).withoutDecimals().positive()
)

/**
 * A text flashed for the next page, such as an access token shown once.
 */
export const flashedTextValidator = vine.create(vine.string())
