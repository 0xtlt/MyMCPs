const CREDENTIAL_QUERY_KEYS = new Set([
  'accesskey',
  'accesstoken',
  'accountkey',
  'apikey',
  'auth',
  'authentication',
  'authorization',
  'awsaccesskeyid',
  'awssecretaccesskey',
  'clientsecret',
  'code',
  'credential',
  'credentials',
  'csrftoken',
  'googleaccessid',
  'jsessionid',
  'key',
  'ocpapimsubscriptionkey',
  'password',
  'passphrase',
  'privatetoken',
  'privatekey',
  'refreshtoken',
  'sastoken',
  'securitytoken',
  'sessionid',
  'sessiontoken',
  'sessionkey',
  'secret',
  'secretkey',
  'sharedaccesskey',
  'sharedaccesssignature',
  'sig',
  'signature',
  'sid',
  'subscriptionkey',
  'token',
  'xamzcredential',
  'xamzsecuritytoken',
  'xamzsignature',
  'xapikey',
  'xsrftoken',
  'xgoogcredential',
  'xgoogsignature',
])

const CREDENTIAL_KEY_SUFFIXES = [
  'accesskey',
  'apikey',
  'credential',
  'password',
  'privatekey',
  'secret',
  'sessionkey',
  'signature',
  'token',
]

function normalizedQueryKey(value: string) {
  return value.toLowerCase().replaceAll(/[^a-z0-9]/g, '')
}

export function isCredentialKey(value: string) {
  let decoded = value
  try {
    decoded = decodeURIComponent(value.replaceAll('+', ' '))
  } catch {
    // Check the raw key when malformed percent escapes cannot be decoded.
  }
  const normalized = normalizedQueryKey(decoded)
  return (
    CREDENTIAL_QUERY_KEYS.has(normalized) ||
    CREDENTIAL_KEY_SUFFIXES.some((suffix) => normalized.endsWith(suffix))
  )
}

function hasCredentialQueryKey(url: URL) {
  for (const key of url.searchParams.keys()) {
    if (isCredentialKey(key)) return true
  }

  // URLSearchParams does not treat semicolons as separators, but a number of
  // servers do. Parse literal ampersand/semicolon segments as well so a URL
  // cannot hide `token=...` inside another parameter's value.
  for (const segment of url.search.slice(1).split(/[&;]/)) {
    const rawKey = segment.split('=', 1)[0]
    let key = rawKey
    try {
      key = decodeURIComponent(rawKey.replaceAll('+', ' '))
    } catch {
      // The URL parser accepted this query but its key is not independently
      // decodable. The normalized raw key remains safe to check.
    }
    if (isCredentialKey(key)) return true
  }
  return false
}

/**
 * Parse an operator-supplied endpoint without allowing credentials to be
 * embedded in plaintext URL fields.
 */
export function ensureSafeHttpUrl(value: string, label: string) {
  let url: URL
  try {
    url = new URL(value)
  } catch {
    throw new Error(`${label} must be a valid URL`)
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new Error(`${label} must use HTTP or HTTPS`)
  }
  if (url.username || url.password) {
    throw new Error(`${label} must not include URL credentials`)
  }
  if (url.hash) {
    throw new Error(`${label} must not include a fragment`)
  }
  if (hasCredentialQueryKey(url)) {
    throw new Error(`${label} must not include credential query parameters`)
  }
  return url
}
