/**
 * Vine schemas for the built-in Google Ads MCP: the arguments of its tools,
 * what its upload links refer to, and the JSON Google answers a refused
 * request with. A schema lists the arguments in the order they are checked:
 * of several wrong ones, the agent is told about the first.
 */
import vine from '@vinejs/vine'
import { languageCode } from '#services/builtin/google_ads/format'
import { isBuiltinUploadId } from '#services/builtin/upload_store'
import {
  blankAsMissing,
  boolean,
  choice,
  integer,
  isBlank,
  line,
  mediaType,
  number,
  pattern,
  text,
  toolVine,
  uploadedFileName,
  VineArgument,
} from '#validators/builtin_tools'

/** The bounds the tools advertise, and the schemas below enforce. */
export const GOOGLE_ADS_LIMITS = {
  rows: 1000,
  queryLength: 10_000,
  nameLength: 255,
  /** A daily budget or a bid above these is a typing mistake, in any currency Google bills in. */
  dailyBudget: 1_000_000,
  bid: 10_000,
  targetRoas: 1000,
  keywords: 100,
  keywordLength: 80,
  headlines: 15,
  headlineLength: 30,
  descriptions: 4,
  descriptionLength: 90,
  pathLength: 15,
  displayTexts: 5,
  longHeadlineLength: 90,
  businessNameLength: 25,
  images: 15,
  logos: 5,
  locations: 100,
  languages: 30,
  locationNames: 25,
  seedKeywords: 20,
  urlLength: 2048,
  criteria: 100,
  changeDays: 30,
  linkMinutes: 60,
  filenameLength: 255,
  /** Google Ads takes images of up to 5120 KB. */
  imageBytes: 5_242_880,
} as const

export const GOOGLE_ADS_DATE_RANGES = [
  'TODAY',
  'YESTERDAY',
  'LAST_7_DAYS',
  'LAST_14_DAYS',
  'LAST_30_DAYS',
  'THIS_MONTH',
  'LAST_MONTH',
] as const

export const GOOGLE_ADS_STATUSES = ['ENABLED', 'PAUSED', 'REMOVED'] as const
export const GOOGLE_ADS_MATCH_TYPES = ['EXACT', 'PHRASE', 'BROAD'] as const
export const GOOGLE_ADS_CHANNELS = ['SEARCH', 'DISPLAY'] as const
export const GOOGLE_ADS_BIDDING_STRATEGIES = [
  'MAXIMIZE_CLICKS',
  'MAXIMIZE_CONVERSIONS',
  'MAXIMIZE_CONVERSION_VALUE',
  'MANUAL_CPC',
] as const
export const GOOGLE_ADS_PERFORMANCE_LEVELS = ['account', 'campaign', 'ad_group'] as const
export const GOOGLE_ADS_SEGMENTS = ['date', 'week', 'month', 'device', 'network'] as const
export const GOOGLE_ADS_ASSET_TYPES = ['IMAGE', 'TEXT', 'SITELINK', 'CALLOUT', 'ALL'] as const

export type GoogleAdsMatchType = (typeof GOOGLE_ADS_MATCH_TYPES)[number]
export type GoogleAdsKeyword = { text: string; matchType: GoogleAdsMatchType; maxCpc?: number }

/** With or without its dashes, as Google Ads shows it. */
const customerId = () =>
  pattern(/^\d{3}-?\d{3}-?\d{4}$/, 'a Google Ads account ID such as 123-456-7890')

/**
 * Identifiers are 64-bit numbers, which JSON numbers cannot all hold: they
 * are text here, and a number is taken as its digits.
 */
const id = (what: string) => pattern(/^\d{1,19}$/, `the numeric ID of ${what}`)

const amount = (max: number) => number({ min: 0.01, max })

const calendarDateRule = toolVine.createRule(
  (value, _options, field) => {
    const written = typeof value === 'string' ? value.trim() : ''
    const [year, month, day] = /^\d{4}-\d{2}-\d{2}$/.test(written)
      ? written.split('-').map(Number)
      : []
    const date = new Date(Date.UTC(year, month - 1, day))
    if (
      !year ||
      date.getUTCFullYear() !== year ||
      date.getUTCMonth() !== month - 1 ||
      date.getUTCDate() !== day
    ) {
      field.report('{{ field }} must be a date such as 2026-01-31', 'calendarDate', field)
      return
    }
    field.mutate(written, field)
  },
  {
    toJSONSchema: (schema) => {
      schema.type = 'string'
    },
  }
)

/** A day in the account's time zone, as `YYYY-MM-DD`. */
const calendarDate = () => new VineArgument<string>(calendarDateRule()).parse(blankAsMissing)

type ListOptions<Item> = {
  min?: number
  max: number
  sentence: string
  /** The item as the tool takes it, or `undefined` when it is not one. */
  item: (value: unknown) => Item | undefined
  /** How one item reads in a JSON Schema. */
  schema: Record<string, unknown>
}

/** One sentence whatever is wrong with the list, so one rule for all of it. */
const listRule = toolVine.createRule<ListOptions<unknown>>(
  (value, { min = 1, max, sentence, item }, field) => {
    const items = Array.isArray(value) ? value.map(item) : []
    if (
      !Array.isArray(value) ||
      items.length < min ||
      items.length > max ||
      items.includes(undefined)
    ) {
      field.report(sentence, 'list', field)
      return
    }
    field.mutate(items, field)
  },
  {
    toJSONSchema: (schema, { min = 1, max, schema: items }) => {
      Object.assign(schema, { type: 'array', items, minItems: min, maxItems: max })
    },
  }
)

/** A list checked as a whole. A single value is a list of one. */
function list<Item>(options: ListOptions<Item>) {
  return new VineArgument<Item[]>(listRule(options as ListOptions<unknown>)).parse((value) =>
    isBlank(value) ? undefined : Array.isArray(value) ? value : [value]
  )
}

function trimmed(value: unknown, max: number) {
  const written = typeof value === 'string' ? value.trim() : ''
  return written && written.length <= max && !/\p{Cc}/u.test(written) ? written : undefined
}

const texts = (what: string, bounds: { min: number; max: number; length: number }) =>
  list({
    ...bounds,
    sentence: `{{ field }} must be a list of ${bounds.min} to ${bounds.max} ${what} of at most ${bounds.length} characters each`,
    item: (value) => trimmed(value, bounds.length),
    schema: { type: 'string', maxLength: bounds.length },
  })

const ids = (what: string, max: number) =>
  list({
    max,
    sentence: `{{ field }} must be a list of 1 to ${max} numeric IDs of ${what}`,
    item: (value) => {
      const written = typeof value === 'number' ? String(value) : value
      return typeof written === 'string' && /^\d{1,19}$/.test(written.trim())
        ? written.trim()
        : undefined
    },
    schema: { type: 'string' },
  })

/** As Google Ads names languages: `en`, `fr`, and with a region `pt_BR` or `zh_CN`. */
const languages = () =>
  list({
    max: GOOGLE_ADS_LIMITS.languages,
    sentence: `{{ field }} must be a list of 1 to ${GOOGLE_ADS_LIMITS.languages} language codes such as en, fr, or pt_BR`,
    item: (value) =>
      typeof value === 'string' && /^[a-z]{2}([_-][a-z]{2})?$/i.test(value.trim())
        ? languageCode(value)
        : undefined,
    schema: { type: 'string' },
  })

function keyword(value: unknown): GoogleAdsKeyword | undefined {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return undefined
  const { text: written, match_type: matchType, max_cpc: maxCpc } = value as Record<string, unknown>

  const keywordText = trimmed(written, GOOGLE_ADS_LIMITS.keywordLength)
  const known: readonly unknown[] = GOOGLE_ADS_MATCH_TYPES
  if (!keywordText || !known.includes(matchType)) return undefined
  if (isBlank(maxCpc)) return { text: keywordText, matchType: matchType as GoogleAdsMatchType }

  const bid = typeof maxCpc === 'string' ? Number(maxCpc) : maxCpc
  return typeof bid === 'number' && bid >= 0.01 && bid <= GOOGLE_ADS_LIMITS.bid
    ? { text: keywordText, matchType: matchType as GoogleAdsMatchType, maxCpc: bid }
    : undefined
}

const keywords = () =>
  list({
    max: GOOGLE_ADS_LIMITS.keywords,
    sentence: `{{ field }} must be a list of 1 to ${GOOGLE_ADS_LIMITS.keywords} keywords such as {"text": "running shoes", "match_type": "PHRASE"}, with a text of at most ${GOOGLE_ADS_LIMITS.keywordLength} characters, a match_type among ${GOOGLE_ADS_MATCH_TYPES.join(', ')}, and an optional max_cpc between 0.01 and ${GOOGLE_ADS_LIMITS.bid}`,
    item: keyword,
    schema: { type: 'object' },
  })

const webAddressRule = toolVine.createRule((value, _options, field) => {
  let url: URL | null = null
  try {
    url = new URL(value as string)
  } catch {
    // Reported below.
  }
  if (!url || (url.protocol !== 'https:' && url.protocol !== 'http:')) {
    field.report(
      '{{ field }} must be a web address such as https://example.com/page',
      'webAddress',
      field
    )
  }
})

/** Where an ad sends people. */
const webAddress = () => line(GOOGLE_ADS_LIMITS.urlLength).use(webAddressRule())

/** One part of the path an ad shows after its domain: no space and no slash. */
const displayPath = () =>
  pattern(
    new RegExp(`^[^\\s/]{1,${GOOGLE_ADS_LIMITS.pathLength}}$`, 'u'),
    `a display path of at most ${GOOGLE_ADS_LIMITS.pathLength} characters, without spaces or slashes`
  )

const selectRule = toolVine.createRule((value, _options, field) => {
  const query = (value as string).trim()
  if (!/^select\s/i.test(query)) {
    field.report(
      '{{ field }} must be a Google Ads Query Language query starting with SELECT',
      'select',
      field
    )
    return
  }
  field.mutate(query, field)
})

/** For an argument that may only be left out when `other` is given. */
const requiredUnlessRule = toolVine.createRule<{ other: string; sentence: string }>(
  (value, { other, sentence }, field) => {
    if (isBlank(value) && isBlank(field.parent[other])) {
      field.report(sentence, 'requiredUnless', field)
    }
  },
  { implicit: true }
)

/** The days the figures cover: a named range, or a first and a last day. */
const period = () => ({
  date_range: choice(GOOGLE_ADS_DATE_RANGES).optional(),
  start_date: calendarDate().optional(),
  end_date: calendarDate().optional(),
})

const rows = () => integer({ min: 1, max: GOOGLE_ADS_LIMITS.rows }).optional()

const bidding = () => ({
  max_cpc: amount(GOOGLE_ADS_LIMITS.bid).optional(),
  target_cpa: amount(GOOGLE_ADS_LIMITS.bid).optional(),
  target_roas: number({ min: 0.01, max: GOOGLE_ADS_LIMITS.targetRoas }).optional(),
})

export const listCampaignsValidator = toolVine.create({
  customer_id: customerId(),
  status: choice(['ENABLED', 'PAUSED', 'ALL']).optional(),
  ...period(),
  limit: rows(),
})

export const campaignValidator = toolVine.create({
  customer_id: customerId(),
  campaign_id: id('a campaign'),
})

/** What a list is narrowed to, and the days its figures cover. */
export const listInCampaignValidator = toolVine.create({
  customer_id: customerId(),
  campaign_id: id('a campaign').optional(),
  ad_group_id: id('an ad group').optional(),
  ...period(),
  limit: rows(),
})

export const listAdGroupsValidator = toolVine.create({
  customer_id: customerId(),
  campaign_id: id('a campaign').optional(),
  ...period(),
  limit: rows(),
})

export const getPerformanceValidator = toolVine.create({
  customer_id: customerId(),
  level: choice(GOOGLE_ADS_PERFORMANCE_LEVELS).optional(),
  segment: choice(GOOGLE_ADS_SEGMENTS).optional(),
  campaign_id: id('a campaign').optional(),
  ...period(),
  limit: rows(),
})

export const listAssetsValidator = toolVine.create({
  customer_id: customerId(),
  type: choice(GOOGLE_ADS_ASSET_TYPES).optional(),
  limit: rows(),
})

export const listChangesValidator = toolVine.create({
  customer_id: customerId(),
  days: integer({ min: 1, max: GOOGLE_ADS_LIMITS.changeDays }).optional(),
  limit: rows(),
})

export const searchLocationsValidator = toolVine.create({
  names: texts('place names', { min: 1, max: GOOGLE_ADS_LIMITS.locationNames, length: 80 }),
  country_code: pattern(/^[A-Za-z]{2}$/, 'a two-letter country code such as FR').optional(),
  locale: pattern(/^[A-Za-z]{2}$/, 'a two-letter language code such as fr').optional(),
})

export const keywordIdeasValidator = toolVine.create({
  customer_id: customerId(),
  keywords: texts('seed keywords', {
    min: 1,
    max: GOOGLE_ADS_LIMITS.seedKeywords,
    length: GOOGLE_ADS_LIMITS.keywordLength,
  }).optional(),
  url: webAddress()
    .optional()
    .use(requiredUnlessRule({ other: 'keywords', sentence: 'Set keywords, url, or both' })),
  location_ids: ids('locations', GOOGLE_ADS_LIMITS.locations).optional(),
  language: pattern(
    /^[A-Za-z]{2}([_-][A-Za-z]{2})?$/,
    'a language code such as en, fr, or pt_BR'
  ).optional(),
  limit: rows(),
})

export const runQueryValidator = toolVine.create({
  customer_id: customerId(),
  query: text(GOOGLE_ADS_LIMITS.queryLength).use(selectRule()),
  limit: rows(),
})

export const createCampaignValidator = toolVine.create({
  customer_id: customerId(),
  name: line(GOOGLE_ADS_LIMITS.nameLength),
  channel: choice(GOOGLE_ADS_CHANNELS),
  daily_budget: amount(GOOGLE_ADS_LIMITS.dailyBudget),
  bidding_strategy: choice(GOOGLE_ADS_BIDDING_STRATEGIES),
  ...bidding(),
  location_ids: ids('locations', GOOGLE_ADS_LIMITS.locations).optional(),
  languages: languages().optional(),
  start_date: calendarDate().optional(),
  end_date: calendarDate().optional(),
  status: choice(['PAUSED', 'ENABLED']).optional(),
  search_partners: boolean().optional(),
  display_network: boolean().optional(),
  eu_political_ads: boolean().optional(),
})

export const updateCampaignValidator = toolVine.create({
  customer_id: customerId(),
  campaign_id: id('a campaign'),
  name: line(GOOGLE_ADS_LIMITS.nameLength).optional(),
  bidding_strategy: choice(GOOGLE_ADS_BIDDING_STRATEGIES).optional(),
  ...bidding(),
  start_date: calendarDate().optional(),
  end_date: calendarDate().optional(),
  search_partners: boolean().optional(),
  display_network: boolean().optional(),
})

export const setCampaignStatusValidator = toolVine.create({
  customer_id: customerId(),
  campaign_id: id('a campaign'),
  status: choice(GOOGLE_ADS_STATUSES),
})

export const updateCampaignBudgetValidator = toolVine.create({
  customer_id: customerId(),
  campaign_id: id('a campaign'),
  daily_budget: amount(GOOGLE_ADS_LIMITS.dailyBudget),
})

export const updateCampaignTargetingValidator = toolVine.create({
  customer_id: customerId(),
  campaign_id: id('a campaign'),
  add_location_ids: ids('locations', GOOGLE_ADS_LIMITS.locations).optional(),
  remove_location_ids: ids('locations', GOOGLE_ADS_LIMITS.locations).optional(),
  add_languages: languages().optional(),
  remove_languages: languages().optional(),
  add_negative_keywords: keywords().optional(),
  remove_criterion_ids: ids('campaign criteria', GOOGLE_ADS_LIMITS.criteria).optional(),
})

export const createAdGroupValidator = toolVine.create({
  customer_id: customerId(),
  campaign_id: id('a campaign'),
  name: line(GOOGLE_ADS_LIMITS.nameLength),
  max_cpc: amount(GOOGLE_ADS_LIMITS.bid).optional(),
  status: choice(['ENABLED', 'PAUSED']).optional(),
})

export const updateAdGroupValidator = toolVine.create({
  customer_id: customerId(),
  ad_group_id: id('an ad group'),
  name: line(GOOGLE_ADS_LIMITS.nameLength).optional(),
  max_cpc: amount(GOOGLE_ADS_LIMITS.bid).optional(),
  status: choice(GOOGLE_ADS_STATUSES).optional(),
})

export const addKeywordsValidator = toolVine.create({
  customer_id: customerId(),
  ad_group_id: id('an ad group'),
  keywords: keywords(),
  negative: boolean().optional(),
})

export const updateKeywordValidator = toolVine.create({
  customer_id: customerId(),
  ad_group_id: id('an ad group'),
  criterion_id: id('a keyword'),
  max_cpc: amount(GOOGLE_ADS_LIMITS.bid).optional(),
  status: choice(GOOGLE_ADS_STATUSES)
    .optional()
    .use(requiredUnlessRule({ other: 'max_cpc', sentence: 'Set status, max_cpc, or both' })),
})

export const createSearchAdValidator = toolVine.create({
  customer_id: customerId(),
  ad_group_id: id('an ad group'),
  headlines: texts('headlines', {
    min: 3,
    max: GOOGLE_ADS_LIMITS.headlines,
    length: GOOGLE_ADS_LIMITS.headlineLength,
  }),
  descriptions: texts('descriptions', {
    min: 2,
    max: GOOGLE_ADS_LIMITS.descriptions,
    length: GOOGLE_ADS_LIMITS.descriptionLength,
  }),
  final_url: webAddress(),
  path1: displayPath().optional(),
  path2: displayPath().optional(),
  status: choice(['ENABLED', 'PAUSED']).optional(),
})

export const createDisplayAdValidator = toolVine.create({
  customer_id: customerId(),
  ad_group_id: id('an ad group'),
  marketing_image_asset_ids: ids('landscape image assets', GOOGLE_ADS_LIMITS.images),
  square_marketing_image_asset_ids: ids('square image assets', GOOGLE_ADS_LIMITS.images),
  square_logo_asset_ids: ids('square logo assets', GOOGLE_ADS_LIMITS.logos).optional(),
  wide_logo_asset_ids: ids('wide logo assets', GOOGLE_ADS_LIMITS.logos).optional(),
  headlines: texts('headlines', {
    min: 1,
    max: GOOGLE_ADS_LIMITS.displayTexts,
    length: GOOGLE_ADS_LIMITS.headlineLength,
  }),
  long_headline: line(GOOGLE_ADS_LIMITS.longHeadlineLength),
  descriptions: texts('descriptions', {
    min: 1,
    max: GOOGLE_ADS_LIMITS.displayTexts,
    length: GOOGLE_ADS_LIMITS.descriptionLength,
  }),
  business_name: line(GOOGLE_ADS_LIMITS.businessNameLength),
  final_url: webAddress(),
  status: choice(['ENABLED', 'PAUSED']).optional(),
})

export const setAdStatusValidator = toolVine.create({
  customer_id: customerId(),
  ad_group_id: id('an ad group'),
  ad_id: id('an ad'),
  status: choice(GOOGLE_ADS_STATUSES),
})

const imageFileName = () => uploadedFileName(GOOGLE_ADS_LIMITS.filenameLength)

export const createImageUploadLinkValidator = toolVine.create({
  filename: imageFileName(),
  content_type: mediaType().optional(),
  expires_in_minutes: integer({ min: 1, max: GOOGLE_ADS_LIMITS.linkMinutes }).optional(),
})

const uploadIdRule = toolVine.createRule(
  (value, _options, field) => {
    const written = typeof value === 'string' ? value.trim().toLowerCase() : ''
    if (!isBuiltinUploadId(written)) {
      field.report(
        '{{ field }} must be an upload ID, as returned by create_image_upload_link',
        'uploadId',
        field
      )
      return
    }
    field.mutate(written, field)
  },
  {
    toJSONSchema: (schema) => {
      schema.type = 'string'
    },
  }
)

const uploadId = () => new VineArgument<string>(uploadIdRule()).parse(blankAsMissing)

/** What create_image_upload_link puts in a link, and gets back when a file is sent to it. */
export const imageUploadReferenceValidator = toolVine.create({
  upload: uploadId(),
  filename: imageFileName(),
  content_type: mediaType().optional(),
})

export const createImageAssetValidator = toolVine.create({
  customer_id: customerId(),
  upload_id: uploadId(),
  name: line(GOOGLE_ADS_LIMITS.nameLength),
})

export const addCampaignImagesValidator = toolVine.create({
  customer_id: customerId(),
  campaign_id: id('a campaign'),
  asset_ids: ids('image assets', GOOGLE_ADS_LIMITS.images),
})

/**
 * One page of a report. Each row holds one object for each resource the query
 * selects from, whose fields are the ones that were selected.
 */
export const googleAdsRowsValidator = vine.create({
  results: vine.array(vine.record(vine.any())).optional(),
  nextPageToken: vine.string().optional(),
})

/** What a mutate answers: for each operation, one key naming what it changed. */
export const googleAdsMutationValidator = vine.create({
  mutateOperationResponses: vine
    .array(vine.record(vine.object({ resourceName: vine.string().optional() })))
    .optional(),
})

/**
 * The body of a response Google sent with an error status. Each detail may
 * carry the failure of the Google Ads API itself, with one entry per mistake
 * in the request.
 */
export const googleAdsFailureValidator = vine.create({
  error: vine.object({
    code: vine.number().optional(),
    message: vine.string().optional(),
    status: vine.string().optional(),
    details: vine
      .array(
        vine.object({
          errors: vine
            .array(
              vine.object({
                // One key naming the kind of error, such as `{ "authorizationError": "USER_PERMISSION_DENIED" }`.
                errorCode: vine.record(vine.string()).optional(),
                message: vine.string().optional(),
                location: vine
                  .object({
                    fieldPathElements: vine
                      .array(
                        vine.object({
                          fieldName: vine.string().optional(),
                          index: vine.number().optional(),
                        })
                      )
                      .optional(),
                  })
                  .optional(),
              })
            )
            .optional(),
          requestId: vine.string().optional(),
        })
      )
      .optional(),
  }),
})
