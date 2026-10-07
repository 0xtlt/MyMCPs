import type { BuiltinOauthMcpDefinition } from '#services/builtin/definition'
import { googleAdsRequest } from '#services/builtin/google_ads/api'
import { customerNumber } from '#services/builtin/google_ads/format'
import { googleAdsReadTools } from '#services/builtin/google_ads/read_tools'
import { googleAdsWriteTools, imageUpload } from '#services/builtin/google_ads/write_tools'

const CUSTOMER_ID = /^\d{3}-?\d{3}-?\d{4}$/
const MAX_ALLOWED_ACCOUNTS = 50

/**
 * Google has no MCP for Google Ads that a self-hosted gateway can sign in to,
 * so this one talks to the Google Ads API through an OAuth client the admin
 * creates in their own Google Cloud project. What the project may reach
 * (test accounts only, or production ones) is its access level at Google:
 * developer tokens, which used to carry it, were retired in September 2026.
 */
export const googleAdsMcp: BuiltinOauthMcpDefinition = {
  key: 'google-ads',
  name: 'Google Ads',
  oauth: {
    issuer: 'https://accounts.google.com',
    authorizeUrl: 'https://accounts.google.com/o/oauth2/v2/auth',
    tokenUrl: 'https://oauth2.googleapis.com/token',
    // The one scope of the Google Ads API. It reads and writes: what agents
    // may change is decided in MyMCPs, by write access and tool approvals.
    scopes: ['https://www.googleapis.com/auth/adwords'],
    writeScopes: [],
    scopeSeparator: ' ',
    // Google only issues a refresh token for offline access, and only issues
    // one again to an account that already consented when asked to consent.
    authorizeParams: { access_type: 'offline', prompt: 'consent' },
    sendsRedirectUriWithCode: true,
    clientIdPattern: /^[\w-]+\.apps\.googleusercontent\.com$/,
    clientIdHint: 'The Google Client ID ends in .apps.googleusercontent.com',
  },
  settings: [
    {
      key: 'loginCustomerId',
      pattern: /^\d{10}$/,
      hint: 'Enter the ID of the manager account, such as 123-456-7890',
      normalize: customerNumber,
    },
    {
      key: 'customerIds',
      pattern: new RegExp(`^\\d{10}( \\d{10}){0,${MAX_ALLOWED_ACCOUNTS - 1}}$`),
      hint: `Enter up to ${MAX_ALLOWED_ACCOUNTS} Google Ads account IDs, such as 123-456-7890, separated by commas`,
      // Stored as the IDs alone, separated by spaces. What is not an ID is
      // kept as written, for the pattern to refuse.
      normalize: (value) =>
        value
          .split(/[\s,;]+/)
          .filter(Boolean)
          .map((id) => (CUSTOMER_ID.test(id) ? customerNumber(id) : id))
          .join(' '),
    },
  ],
  tools: [...googleAdsReadTools, ...googleAdsWriteTools],
  verify: async (context) => {
    await googleAdsRequest(context, '/customers:listAccessibleCustomers', { method: 'GET' })
  },
  upload: imageUpload,
}
