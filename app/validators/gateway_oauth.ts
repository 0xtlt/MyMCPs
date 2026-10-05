/**
 * Vine schemas for the gateway's own OAuth 2.1 authorization server. Each
 * validator stands for one decision of the protocol: the code that runs it
 * answers a failure with the one OAuth error that decision has.
 */
import vine, { Vine } from '@vinejs/vine'
import { GATEWAY_OAUTH_SCOPE, LOOPBACK_HOSTS } from '#services/gateway_oauth_constants'

/**
 * The app converts blank strings to null before validating
 * (start/validator.ts), which suits HTML forms. An OAuth parameter is judged
 * as the client sent it: `scope=` names no scope and a blank `client_id`
 * names an unknown client, neither was left out. The validators below are
 * therefore created by an instance that leaves values as they are.
 */
const verbatim = new Vine()

const NATIVE_APP_REDIRECT_URIS = new Set(['cursor://anysphere.cursor-mcp/oauth/callback'])
const CLIENT_AUTH_METHODS = ['none', 'client_secret_post', 'client_secret_basic'] as const
const CLIENT_GRANT_TYPES = ['authorization_code', 'refresh_token'] as const

/**
 * Query strings and form bodies can carry a parameter several times or as a
 * nested value. An optional parameter sent in such a shape has always been
 * read as not sent, rather than refused.
 */
function stringOrAbsent(value: unknown) {
  return typeof value === 'string' ? value : undefined
}

/** RFC 6749, section 3.3: scopes travel as one space-delimited string. */
function scopeList(value: unknown) {
  return typeof value === 'string' ? value.split(' ').filter(Boolean) : undefined
}

/** A value repeated in a list is dropped, not refused. */
function distinctValues(value: unknown) {
  return Array.isArray(value) ? [...new Set(value)] : value
}

/** A parameter the request cannot do without: any string but the empty one. */
const requiredParameter = () => vine.string().minLength(1)

/**
 * Where a client may ask to be sent back: HTTPS anywhere, plain HTTP on the
 * user's own device, or a native-app callback approved by name.
 */
const redirectTarget = vine.createRule((value, _options, field) => {
  if (typeof value !== 'string') {
    return
  }
  // Cursor historically uses this private-use callback when its localhost
  // listener is unavailable. Keep the exception exact so arbitrary custom
  // schemes cannot be registered as OAuth redirect targets.
  if (NATIVE_APP_REDIRECT_URIS.has(value)) {
    return
  }
  const url = URL.parse(value)
  const isSecure =
    url?.protocol === 'https:' || (url?.protocol === 'http:' && LOOPBACK_HOSTS.has(url.hostname))
  if (!url || !isSecure || url.hash || url.username || url.password) {
    field.report('The {{ field }} field must be an allowed redirect URI', 'redirectTarget', field)
  }
})

/** RFC 8252 allows native loopback clients to choose their callback port at runtime. */
function redirectUriMatches(requested: string, registered: string) {
  if (requested === registered) return true

  try {
    const requestUrl = new URL(requested)
    const registeredUrl = new URL(registered)
    if (!LOOPBACK_HOSTS.has(requestUrl.hostname) || !LOOPBACK_HOSTS.has(registeredUrl.hostname)) {
      return false
    }

    return (
      requestUrl.protocol === registeredUrl.protocol &&
      requestUrl.hostname === registeredUrl.hostname &&
      requestUrl.pathname === registeredUrl.pathname &&
      requestUrl.search === registeredUrl.search &&
      requestUrl.hash === registeredUrl.hash
    )
  } catch {
    return false
  }
}

/** An authorization request may only name a redirect URI its client registered. */
const registeredRedirectUri = vine.createRule((value, _options, field) => {
  if (typeof value !== 'string') {
    return
  }
  const registered: string[] = field.meta.registeredRedirectUris
  if (!registered.some((uri) => redirectUriMatches(value, uri))) {
    field.report(
      'The {{ field }} field must be a redirect URI the client registered',
      'registeredRedirectUri',
      field
    )
  }
})

/**
 * Tokens are only issued for this gateway. Clients spell its URL in
 * equivalent ways, so the comparison is between normalized URLs.
 */
const gatewayResource = vine.createRule((value, _options, field) => {
  if (typeof value !== 'string') {
    return
  }
  if (URL.parse(value)?.href !== field.meta.gatewayResource) {
    field.report('The {{ field }} field must be the gateway resource', 'gatewayResource', field)
  }
})

/** Refresh tokens extend a grant that an authorization code has to start. */
const withAuthorizationCode = vine.createRule((value, _options, field) => {
  if (Array.isArray(value) && !value.includes('authorization_code')) {
    field.report(
      'The {{ field }} field must include authorization_code',
      'withAuthorizationCode',
      field
    )
  }
})

/**
 * Redirect URIs of a client registering itself. The MCP SDK schema has
 * checked the shape of the metadata by then; this and the validators that
 * follow decide what the gateway accepts of it.
 */
export const clientRedirectUrisValidator = verbatim.create(
  vine.array(vine.string().maxLength(2048).use(redirectTarget())).minLength(1).maxLength(10)
)

/**
 * How a registering client authenticates at the token endpoint. RFC 7591
 * makes `client_secret_basic` the default.
 */
export const clientAuthMethodValidator = verbatim.create(
  vine.enum(CLIENT_AUTH_METHODS).parse((value) => value ?? 'client_secret_basic')
)

/**
 * Grant types of a registering client, both by default. The list is stored
 * and parsed again on every /authorize and /token request, so it is reduced
 * to its distinct values and bounded before its members are looked at.
 */
export const clientGrantTypesValidator = verbatim.create(
  vine
    .array(vine.enum(CLIENT_GRANT_TYPES))
    .parse((value) => distinctValues(value ?? CLIENT_GRANT_TYPES))
    .maxLength(CLIENT_GRANT_TYPES.length)
    .use(withAuthorizationCode())
)

/**
 * Response types of a registering client, reduced and bounded like its grant
 * types. OAuth 2.1 only keeps the authorization code flow.
 */
export const clientResponseTypesValidator = verbatim.create(
  vine
    .array(vine.literal('code'))
    .parse((value) => distinctValues(value ?? ['code']))
    .fixedLength(1)
)

/**
 * The `scope` of a registration or an authorization request. When sent, it
 * must name the gateway scope and nothing else.
 */
export const requestedScopeValidator = verbatim.create(
  vine.array(vine.literal(GATEWAY_OAUTH_SCOPE)).parse(scopeList).fixedLength(1).optional()
)

/**
 * Name of a registering client, shown on the consent screen and in the list
 * of connections.
 */
export const clientNameValidator = verbatim.create(vine.string().trim().maxLength(120).optional())

/**
 * `client_id` of an authorization request.
 */
export const authorizationClientIdValidator = verbatim.create(requiredParameter())

/**
 * `redirect_uri` of an authorization request, judged against the URIs of the
 * client the request names.
 */
export const authorizationRedirectUriValidator = verbatim
  .withMetaData<{ registeredRedirectUris: string[] }>()
  .create(requiredParameter().use(registeredRedirectUri()))

/**
 * `state` as the client sent it, to be echoed with whatever answer the
 * request gets. In any shape but a string it is no state at all.
 */
export const authorizationStateValidator = verbatim.create(vine.string().optional())

/**
 * The longest `state` an authorization request may carry.
 */
export const authorizationStateLengthValidator = verbatim.create(
  vine.string().maxLength(2048).nullable()
)

/**
 * `response_type` of an authorization request.
 */
export const authorizationResponseTypeValidator = verbatim.create(vine.literal('code'))

/**
 * PKCE (RFC 7636) parameters of an authorization request: a base64url
 * SHA-256 challenge, the only method accepted.
 */
export const pkceChallengeValidator = verbatim.create({
  code_challenge: vine.string().regex(/^[A-Za-z0-9_-]{43,128}$/),
  code_challenge_method: vine.literal('S256'),
})

/**
 * `resource` of an authorization or token request (RFC 8707).
 */
export const gatewayResourceValidator = verbatim
  .withMetaData<{ gatewayResource: string }>()
  .create(vine.string().use(gatewayResource()))

/**
 * Answer of the consent form. Only an explicit approval grants access.
 */
export const consentApprovalValidator = verbatim.create(vine.literal('approve'))

/**
 * Client credentials sent in the request body: `client_secret_post`, or a
 * public client naming itself.
 */
export const postedClientCredentialsValidator = verbatim.create({
  client_id: requiredParameter(),
  client_secret: vine.string().parse(stringOrAbsent).optional(),
})

/**
 * What every token request must say. This schema and the three request
 * schemas below only ask for parameters to be present, so the first field
 * that fails names a missing parameter.
 */
export const tokenRequestValidator = verbatim.create({
  grant_type: requiredParameter(),
})

/**
 * Parameters of the `authorization_code` grant.
 */
export const authorizationCodeGrantValidator = verbatim.create({
  code: requiredParameter(),
  code_verifier: requiredParameter(),
  redirect_uri: requiredParameter(),
  resource: requiredParameter(),
})

/**
 * Parameters of the `refresh_token` grant.
 */
export const refreshTokenGrantValidator = verbatim.create({
  refresh_token: requiredParameter(),
  scope: vine.string().parse(stringOrAbsent).optional(),
  resource: requiredParameter(),
})

/**
 * Parameters of a revocation request (RFC 7009).
 */
export const revocationRequestValidator = verbatim.create({
  token: requiredParameter(),
})

/**
 * PKCE code verifier, in the form RFC 7636 gives it.
 */
export const pkceVerifierValidator = verbatim.create(
  vine.string().regex(/^[A-Za-z0-9._~-]{43,128}$/)
)

/**
 * `scope` of a refresh request: it may repeat the scope of the grant and
 * cannot ask for another.
 */
export const refreshScopeValidator = verbatim.create(vine.literal(GATEWAY_OAUTH_SCOPE).nullable())
