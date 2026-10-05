import { OauthClientSchema } from '#database/schema'
import { hasMany } from '@adonisjs/lucid/orm'
import type { HasMany } from '@adonisjs/lucid/types/relations'
import AccessToken from '#models/access_token'
import OauthAuthorizationCode from '#models/oauth_authorization_code'

/**
 * Registration stores at most two grant types and one response type. Older
 * rows may hold an arbitrarily long list, which is not worth parsing.
 */
const MAX_TYPE_LIST_CHARS = 256

function parseStringList(value: string, maxChars = Number.POSITIVE_INFINITY): string[] {
  if (value.length > maxChars) return []

  try {
    const parsed: unknown = JSON.parse(value)
    return Array.isArray(parsed)
      ? parsed.filter((item): item is string => typeof item === 'string')
      : []
  } catch {
    return []
  }
}

export default class OauthClient extends OauthClientSchema {
  @hasMany(() => OauthAuthorizationCode, { foreignKey: 'oauthClientId' })
  declare authorizationCodes: HasMany<typeof OauthAuthorizationCode>

  @hasMany(() => AccessToken, { foreignKey: 'oauthClientId' })
  declare accessTokens: HasMany<typeof AccessToken>

  get redirectUriList() {
    return parseStringList(this.redirectUris)
  }

  get grantTypeList() {
    return parseStringList(this.grantTypes, MAX_TYPE_LIST_CHARS)
  }

  get responseTypeList() {
    return parseStringList(this.responseTypes, MAX_TYPE_LIST_CHARS)
  }
}
