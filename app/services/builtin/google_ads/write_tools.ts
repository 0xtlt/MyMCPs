import { randomUUID } from 'node:crypto'
import type { Tool } from '@modelcontextprotocol/sdk/types.js'
import type { VineValidator } from '@vinejs/vine'
import type { Infer, SchemaTypes } from '@vinejs/vine/types'
import {
  BuiltinToolError,
  type ApprovalDetail,
  type ApprovalSummary,
  type BuiltinTool,
  type BuiltinToolContext,
  type BuiltinUploadTarget,
} from '#services/builtin/definition'
import { builtinUploadUrl } from '#services/builtin/file_link'
import {
  customerOf,
  mutateGoogleAds,
  resourceId,
  searchGoogleAds,
  type GoogleAdsOperation,
} from '#services/builtin/google_ads/api'
import { money, toMicros } from '#services/builtin/google_ads/format'
import {
  imageInfo,
  imageShape,
  IMAGE_SHAPES,
  SQUARE_LOGO_MIN_PIXELS,
} from '#services/builtin/google_ads/images'
import {
  accountFacts,
  adFacts,
  adGroupFacts,
  campaignFacts,
  imageAssetsById,
  keywordFacts,
  languagesByCode,
  locationsById,
  type AccountFacts,
  type AdGroupFacts,
  type CampaignFacts,
  type ImageAssetFacts,
} from '#services/builtin/google_ads/lookup'
import { builtinTool, toolInput } from '#services/builtin/tool_input'
import {
  BUILTIN_UPLOAD_MINUTES,
  findBuiltinUpload,
  readBuiltinUpload,
} from '#services/builtin/upload_store'
import {
  addCampaignImagesValidator,
  addKeywordsValidator,
  createAdGroupValidator,
  createCampaignValidator,
  createDisplayAdValidator,
  createImageAssetValidator,
  createImageUploadLinkValidator,
  createSearchAdValidator,
  GOOGLE_ADS_BIDDING_STRATEGIES,
  GOOGLE_ADS_CHANNELS,
  GOOGLE_ADS_LIMITS,
  GOOGLE_ADS_MATCH_TYPES,
  GOOGLE_ADS_STATUSES,
  imageUploadReferenceValidator,
  setAdStatusValidator,
  setCampaignStatusValidator,
  updateAdGroupValidator,
  updateCampaignBudgetValidator,
  updateCampaignTargetingValidator,
  updateCampaignValidator,
  updateKeywordValidator,
  type GoogleAdsKeyword,
} from '#validators/builtin_google_ads'

const MONEY = "Amounts are in the account's currency."
const DEFAULT_LINK_MINUTES = 15
const MAX_IMAGE_MEGABYTES = GOOGLE_ADS_LIMITS.imageBytes / 1_048_576
/** Google never bills more in a month than the daily budget times this. */
const DAYS_IN_A_MONTH = 30.4
/** How many of a long list a person is shown one by one before being sent to the arguments. */
const MAX_LISTED = 30
/** One page of a report, which holds the locations and languages of any campaign. */
const MAX_CAMPAIGN_TARGETS = 10_000

type BiddingStrategy = (typeof GOOGLE_ADS_BIDDING_STRATEGIES)[number]
type Status = (typeof GOOGLE_ADS_STATUSES)[number]

/**
 * What a tool is about to do: the changes Google Ads is sent, what the person
 * asked to approve them reads, and what the agent gets back. Both the changes
 * and the summary come from the same reading of the arguments.
 */
type Plan = {
  customer: string
  operations: GoogleAdsOperation[]
  summary: ApprovalSummary
  /** `created` holds the resource name each operation produced, in their order. */
  result: (created: Array<string | null>) => unknown
}

type WriteTool<Schema extends SchemaTypes> = {
  name: string
  description: string
  inputSchema: Tool['inputSchema']
  input: VineValidator<Schema, any>
  approval?: 'ask'
  plan: (input: Infer<Schema>, context: BuiltinToolContext) => Promise<Plan>
}

/**
 * A tool that changes an account. Before anyone is asked to approve a call,
 * Google checks the very changes it would make, without making them.
 */
function writeTool<Schema extends SchemaTypes>({ plan, ...tool }: WriteTool<Schema>): BuiltinTool {
  return builtinTool({
    ...tool,
    write: true,
    run: async (input, context: BuiltinToolContext) => {
      const { customer, operations, result } = await plan(input, context)
      return result(await mutateGoogleAds(context, customer, operations))
    },
    describe: async (input, context: BuiltinToolContext) => {
      const { customer, operations, summary } = await plan(input, context)
      await mutateGoogleAds(context, customer, operations, { validateOnly: true })
      return summary
    },
  })
}

const customerIdProperty = {
  customer_id: {
    type: 'string',
    description: 'Google Ads account ID, such as 123-456-7890, as returned by list_accounts.',
  },
} as const

const campaignIdProperty = {
  campaign_id: { type: 'string', description: 'Campaign ID, as returned by list_campaigns.' },
} as const

const adGroupIdProperty = {
  ad_group_id: { type: 'string', description: 'Ad group ID, as returned by list_ad_groups.' },
} as const

const biddingProperties = {
  max_cpc: {
    type: 'number',
    minimum: 0.01,
    maximum: GOOGLE_ADS_LIMITS.bid,
    description: 'With MAXIMIZE_CLICKS only: the most a click may cost.',
  },
  target_cpa: {
    type: 'number',
    minimum: 0.01,
    maximum: GOOGLE_ADS_LIMITS.bid,
    description: 'With MAXIMIZE_CONVERSIONS only: the cost per conversion to aim at.',
  },
  target_roas: {
    type: 'number',
    minimum: 0.01,
    maximum: GOOGLE_ADS_LIMITS.targetRoas,
    description:
      'With MAXIMIZE_CONVERSION_VALUE only: the return on ad spend to aim at, as a ratio. 4 is 400%.',
  },
} as const

const keywordListProperty = (what: string) =>
  ({
    type: 'array',
    items: {
      type: 'object',
      properties: {
        text: { type: 'string', maxLength: GOOGLE_ADS_LIMITS.keywordLength },
        match_type: { type: 'string', enum: [...GOOGLE_ADS_MATCH_TYPES] },
        max_cpc: { type: 'number', minimum: 0.01, maximum: GOOGLE_ADS_LIMITS.bid },
      },
      required: ['text', 'match_type'],
    },
    minItems: 1,
    maxItems: GOOGLE_ADS_LIMITS.keywords,
    description: what,
  }) as const

const idListProperty = (max: number, description: string) =>
  ({ type: 'array', items: { type: 'string' }, minItems: 1, maxItems: max, description }) as const

const textListProperty = (min: number, max: number, length: number, description: string) =>
  ({
    type: 'array',
    items: { type: 'string', maxLength: length },
    minItems: min,
    maxItems: max,
    description,
  }) as const

const STATUS_WORDS: Record<string, string> = {
  ENABLED: 'Enabled',
  PAUSED: 'Paused',
  REMOVED: 'Removed',
}

const MATCH_WORDS: Record<string, string> = {
  EXACT: 'exact match',
  PHRASE: 'phrase match',
  BROAD: 'broad match',
}

function statusWord(status: string) {
  return STATUS_WORDS[status] ?? status
}

/** A row that says what a value replaces, when it replaces another one. */
function change(label: string, value: string, before?: string): ApprovalDetail {
  return before === undefined || before === value ? { label, value } : { label, value, before }
}

function accountRow(account: AccountFacts): ApprovalDetail {
  return { label: 'Account', value: account.label }
}

function campaignRow(campaign: Pick<CampaignFacts, 'name' | 'status'>): ApprovalDetail {
  return {
    label: 'Campaign',
    value: `${campaign.name} (${statusWord(campaign.status).toLowerCase()})`,
  }
}

function adGroupRows(adGroup: AdGroupFacts): ApprovalDetail[] {
  return [
    accountRow(adGroup.account),
    campaignRow(adGroup.campaign),
    { label: 'Ad group', value: `${adGroup.name} (${statusWord(adGroup.status).toLowerCase()})` },
  ]
}

/** Each item on a row of its own, up to a number a person still reads. */
function listed(label: string, values: string[]): ApprovalDetail[] {
  const rows = values.slice(0, MAX_LISTED).map((value, index) => ({
    label: values.length === 1 ? label : `${label} ${index + 1}`,
    value,
  }))
  return values.length > MAX_LISTED
    ? [
        ...rows,
        {
          label: `More`,
          value: `${values.length - MAX_LISTED} more, listed in the exact arguments below`,
        },
      ]
    : rows
}

function daily(amount: number, currency: string) {
  return `${money(amount, currency)} a day`
}

function monthlyRow(amount: number, currency: string): ApprovalDetail {
  return {
    label: 'Most it can cost in a month',
    value: money(amount * DAYS_IN_A_MONTH, currency),
  }
}

function keywordLabel({ text, matchType, maxCpc }: GoogleAdsKeyword, currency: string) {
  return [
    `"${text}"`,
    MATCH_WORDS[matchType],
    maxCpc === undefined ? null : `at most ${money(maxCpc, currency)} a click`,
  ]
    .filter(Boolean)
    .join(' · ')
}

function day(date: string, endOfDay = false) {
  return `${date} ${endOfDay ? '23:59:59' : '00:00:00'}`
}

type BiddingTargets = { max_cpc?: number; target_cpa?: number; target_roas?: number }

const BIDDING_WORDS: Record<string, string> = {
  MAXIMIZE_CLICKS: 'Maximize clicks',
  TARGET_SPEND: 'Maximize clicks',
  MAXIMIZE_CONVERSIONS: 'Maximize conversions',
  MAXIMIZE_CONVERSION_VALUE: 'Maximize conversion value',
  MANUAL_CPC: 'Manual cost per click',
}

function biddingLabel(strategy: string, targets: BiddingTargets, currency: string) {
  const target =
    targets.max_cpc !== undefined
      ? `at most ${money(targets.max_cpc, currency)} a click`
      : targets.target_cpa !== undefined
        ? `aiming at ${money(targets.target_cpa, currency)} a conversion`
        : targets.target_roas !== undefined
          ? `aiming at a return of ${Math.round(targets.target_roas * 100)}%`
          : null
  return [BIDDING_WORDS[strategy] ?? strategy, target].filter(Boolean).join(', ')
}

/**
 * How a campaign bids, as the fields Google Ads takes and the paths that
 * name them in an update. Each strategy has one target, and takes no other.
 */
function biddingOf(strategy: BiddingStrategy, targets: BiddingTargets) {
  const own = {
    MAXIMIZE_CLICKS: 'max_cpc',
    MAXIMIZE_CONVERSIONS: 'target_cpa',
    MAXIMIZE_CONVERSION_VALUE: 'target_roas',
    MANUAL_CPC: null,
  }[strategy]
  const foreign = (['max_cpc', 'target_cpa', 'target_roas'] as const).find(
    (name) => name !== own && targets[name] !== undefined
  )
  if (foreign) {
    throw new BuiltinToolError(
      strategy === 'MANUAL_CPC' && foreign === 'max_cpc'
        ? 'With MANUAL_CPC, bids are set on ad groups and keywords: leave max_cpc out here and set it with create_ad_group or add_keywords'
        : `${foreign} does not go with ${strategy}${own ? `, which takes ${own}` : ''}`
    )
  }

  switch (strategy) {
    case 'MAXIMIZE_CLICKS':
      return {
        fields: {
          targetSpend:
            targets.max_cpc === undefined ? {} : { cpcBidCeilingMicros: toMicros(targets.max_cpc) },
        },
        mask: ['target_spend.cpc_bid_ceiling_micros'],
      }
    case 'MAXIMIZE_CONVERSIONS':
      return {
        fields: {
          maximizeConversions:
            targets.target_cpa === undefined
              ? {}
              : { targetCpaMicros: toMicros(targets.target_cpa) },
        },
        mask: ['maximize_conversions.target_cpa_micros'],
      }
    case 'MAXIMIZE_CONVERSION_VALUE':
      return {
        fields: {
          maximizeConversionValue:
            targets.target_roas === undefined ? {} : { targetRoas: targets.target_roas },
        },
        mask: ['maximize_conversion_value.target_roas'],
      }
    case 'MANUAL_CPC':
      return {
        fields: { manualCpc: { enhancedCpcEnabled: false } },
        mask: ['manual_cpc.enhanced_cpc_enabled'],
      }
  }
}

function noLanguagesForSearch() {
  return new BuiltinToolError(
    'Google no longer takes languages for Search campaigns: their ads follow the language of their own text and landing page. Leave the languages out.'
  )
}

function removed(what: string, name: string, rows: ApprovalDetail[]): ApprovalSummary {
  return {
    title: `Remove the ${what} "${name}" for good`,
    details: rows,
    warnings: [`A removed ${what} cannot be restored, and its statistics stop growing.`],
  }
}

/** The image Google Ads would take for `use`, or the sentence that says why it does not. */
function requireImage(
  asset: ImageAssetFacts,
  argument: string,
  use: { shape: (typeof IMAGE_SHAPES)[number]['name']; minWidth: number; minHeight: number }
) {
  const wanted = IMAGE_SHAPES.find((shape) => shape.name === use.shape)!
  const fits =
    imageShape(asset)?.name === use.shape &&
    asset.width >= use.minWidth &&
    asset.height >= use.minHeight
  if (!fits) {
    throw new BuiltinToolError(
      `${argument} takes ${wanted.label} images of at least ${use.minWidth}×${use.minHeight} pixels, and the image asset ${asset.id} is ${asset.width}×${asset.height}`
    )
  }
  return asset
}

function imageLabel(asset: ImageAssetFacts) {
  return `${asset.name || `Asset ${asset.id}`} (${asset.width}×${asset.height})`
}

const LANDSCAPE = { shape: 'landscape', minWidth: 600, minHeight: 314 } as const
const SQUARE = { shape: 'square', minWidth: 300, minHeight: 300 } as const
const SQUARE_LOGO = {
  shape: 'square',
  minWidth: SQUARE_LOGO_MIN_PIXELS,
  minHeight: SQUARE_LOGO_MIN_PIXELS,
} as const
const WIDE_LOGO = { shape: 'wide_logo', minWidth: 512, minHeight: 128 } as const

/**
 * The link an agent sends an image to. It outlives the call that made it, so
 * what it refers to is checked again when the file arrives.
 */
export async function imageUpload(reference: unknown): Promise<BuiltinUploadTarget> {
  const {
    upload,
    filename,
    content_type: contentType,
  } = await toolInput(imageUploadReferenceValidator, reference)
  return { id: upload, filename, contentType, maxBytes: GOOGLE_ADS_LIMITS.imageBytes }
}

export const googleAdsWriteTools: BuiltinTool[] = [
  writeTool({
    name: 'create_campaign',
    description: `Create a Search or Display campaign with its own daily budget, bidding strategy, and targeting. It is created paused unless status says otherwise, and needs an ad group, ads, and for Search keywords before it can show anything: add them with create_ad_group, add_keywords, and create_responsive_search_ad or create_responsive_display_ad, then enable it with set_campaign_status. ${MONEY}`,
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        name: {
          type: 'string',
          maxLength: GOOGLE_ADS_LIMITS.nameLength,
          description: 'Campaign name. No other campaign of the account may have it.',
        },
        channel: {
          type: 'string',
          enum: [...GOOGLE_ADS_CHANNELS],
          description:
            'SEARCH shows text ads on Google search results. DISPLAY shows image ads on websites and apps.',
        },
        daily_budget: {
          type: 'number',
          minimum: 0.01,
          maximum: GOOGLE_ADS_LIMITS.dailyBudget,
          description:
            'Average amount to spend a day, such as 25 or 12.5. Google may spend up to twice that on a day, and never more than 30.4 times it in a month.',
        },
        bidding_strategy: {
          type: 'string',
          enum: [...GOOGLE_ADS_BIDDING_STRATEGIES],
          description:
            'MAXIMIZE_CLICKS needs no conversion tracking. MAXIMIZE_CONVERSIONS and MAXIMIZE_CONVERSION_VALUE need conversions to be tracked. MANUAL_CPC bids what the ad groups and keywords say.',
        },
        ...biddingProperties,
        location_ids: idListProperty(
          GOOGLE_ADS_LIMITS.locations,
          'Locations to show the ads in, by the IDs search_locations returns. Left out, the campaign targets every country.'
        ),
        languages: idListProperty(
          GOOGLE_ADS_LIMITS.languages,
          'Display campaigns only: languages of the people to reach, as codes such as en, fr, or pt_BR. Left out, every language.'
        ),
        start_date: {
          type: 'string',
          description:
            "First day the campaign may run, such as 2026-01-31, in the account's time zone.",
        },
        end_date: {
          type: 'string',
          description: 'Last day the campaign runs. Left out, it has no end.',
        },
        status: {
          type: 'string',
          enum: ['PAUSED', 'ENABLED'],
          default: 'PAUSED',
          description: 'ENABLED lets the campaign spend as soon as it has approved ads.',
        },
        search_partners: {
          type: 'boolean',
          default: false,
          description:
            'Search campaigns only: also show the ads on the sites of Google search partners.',
        },
        display_network: {
          type: 'boolean',
          default: false,
          description: 'Search campaigns only: also show the ads on the Google Display Network.',
        },
        eu_political_ads: {
          type: 'boolean',
          default: false,
          description:
            'Declares that the campaign carries political advertising aimed at the European Union, which Google then does not serve there.',
        },
      },
      required: ['customer_id', 'name', 'channel', 'daily_budget', 'bidding_strategy'],
      additionalProperties: false,
    },
    input: createCampaignValidator,
    approval: 'ask',
    plan: async (input, context) => {
      const customer = customerOf(context, input.customer_id)
      const isSearch = input.channel === 'SEARCH'
      if (
        !isSearch &&
        (input.search_partners !== undefined || input.display_network !== undefined)
      ) {
        throw new BuiltinToolError(
          'search_partners and display_network are for Search campaigns only'
        )
      }
      if (isSearch && input.languages) throw noLanguagesForSearch()
      if (input.start_date && input.end_date && input.start_date > input.end_date) {
        throw new BuiltinToolError('start_date must not be after end_date')
      }

      const status = input.status ?? 'PAUSED'
      const bidding = biddingOf(input.bidding_strategy, input)
      const account = await accountFacts(context, customer)
      const locations = await locationsById(context, customer, input.location_ids ?? [])
      const languages = await languagesByCode(context, customer, input.languages ?? [])

      // Negative IDs stand for what this very request creates. No two may be alike.
      const budget = `customers/${customer}/campaignBudgets/-1`
      const campaign = `customers/${customer}/campaigns/-2`
      const operations: GoogleAdsOperation[] = [
        {
          campaignBudgetOperation: {
            create: {
              resourceName: budget,
              amountMicros: toMicros(input.daily_budget),
              deliveryMethod: 'STANDARD',
              // Left out, Google makes it a budget other campaigns can share.
              explicitlyShared: false,
            },
          },
        },
        {
          campaignOperation: {
            create: {
              resourceName: campaign,
              name: input.name,
              // Left out, Google enables the campaign.
              status,
              advertisingChannelType: input.channel,
              campaignBudget: budget,
              // A Display campaign shows on the Display Network and nowhere
              // else: Google sets its networks itself.
              ...(isSearch
                ? {
                    networkSettings: {
                      targetGoogleSearch: true,
                      targetSearchNetwork: input.search_partners ?? false,
                      targetContentNetwork: input.display_network ?? false,
                      targetPartnerSearchNetwork: false,
                    },
                  }
                : {}),
              ...bidding.fields,
              ...(input.start_date ? { startDateTime: day(input.start_date) } : {}),
              ...(input.end_date ? { endDateTime: day(input.end_date, true) } : {}),
              // Google refuses a campaign that does not say.
              containsEuPoliticalAdvertising: input.eu_political_ads
                ? 'CONTAINS_EU_POLITICAL_ADVERTISING'
                : 'DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING',
            },
          },
        },
        ...locations.map(({ id }) => ({
          campaignCriterionOperation: {
            create: { campaign, location: { geoTargetConstant: `geoTargetConstants/${id}` } },
          },
        })),
        ...languages.map(({ id }) => ({
          campaignCriterionOperation: {
            create: { campaign, language: { languageConstant: `languageConstants/${id}` } },
          },
        })),
      ]

      const { currency } = account
      return {
        customer,
        operations,
        summary: {
          title: `Create the ${isSearch ? 'Search' : 'Display'} campaign "${input.name}" with a budget of ${daily(input.daily_budget, currency)}`,
          details: [
            accountRow(account),
            { label: 'Campaign name', value: input.name },
            { label: 'Type', value: isSearch ? 'Search' : 'Display' },
            { label: 'Daily budget', value: daily(input.daily_budget, currency) },
            monthlyRow(input.daily_budget, currency),
            { label: 'Bidding', value: biddingLabel(input.bidding_strategy, input, currency) },
            {
              label: 'Locations',
              value: locations.map(({ name }) => name).join('; ') || 'Every country',
            },
            ...(isSearch
              ? [
                  {
                    label: 'Networks',
                    value: [
                      'Google Search',
                      input.search_partners ? 'search partners' : null,
                      input.display_network ? 'Display Network' : null,
                    ]
                      .filter(Boolean)
                      .join(', '),
                  },
                ]
              : [
                  {
                    label: 'Languages',
                    value: languages.map(({ name }) => name).join(', ') || 'Every language',
                  },
                ]),
            {
              label: 'Runs',
              value: `${input.start_date ?? 'From today'} to ${input.end_date ?? 'no end date'}`,
            },
            {
              label: 'Status',
              value:
                status === 'ENABLED'
                  ? 'Enabled: it spends as soon as it has approved ads'
                  : 'Paused: it spends nothing until it is enabled',
            },
            ...(input.eu_political_ads
              ? [
                  {
                    label: 'EU political advertising',
                    value: 'Declared: Google does not serve it in the EU',
                  },
                ]
              : []),
          ],
          warnings: [
            ...(status === 'ENABLED'
              ? [
                  `The campaign is created enabled: it can spend ${daily(input.daily_budget, currency)} without another approval.`,
                ]
              : []),
            ...(locations.length === 0
              ? ['No location is set: the ads can show in every country.']
              : []),
          ],
        },
        result: ([, created]) => ({
          campaign_id: resourceId(created),
          name: input.name,
          status,
          daily_budget: input.daily_budget,
          currency,
          next:
            status === 'PAUSED'
              ? 'The campaign is paused. Add an ad group, ads, and keywords, then enable it with set_campaign_status.'
              : 'The campaign is enabled. It shows nothing until it has an ad group with approved ads.',
        }),
      }
    },
  }),
  writeTool({
    name: 'update_campaign',
    description: `Change the settings of a campaign: its name, its bidding strategy and target, its start and end dates, and for a Search campaign the extra networks it shows on. Only what you pass is changed. Its budget is changed with update_campaign_budget, its status with set_campaign_status, and its targeting with update_campaign_targeting. ${MONEY}`,
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        ...campaignIdProperty,
        name: { type: 'string', maxLength: GOOGLE_ADS_LIMITS.nameLength, description: 'New name.' },
        bidding_strategy: {
          type: 'string',
          enum: [...GOOGLE_ADS_BIDDING_STRATEGIES],
          description:
            'New bidding strategy. Left out, a target passed below applies to the current one.',
        },
        ...biddingProperties,
        start_date: { type: 'string', description: 'New first day, such as 2026-01-31.' },
        end_date: { type: 'string', description: 'New last day, such as 2026-03-31.' },
        search_partners: {
          type: 'boolean',
          description:
            'Search campaigns only: show the ads on the sites of Google search partners.',
        },
        display_network: {
          type: 'boolean',
          description: 'Search campaigns only: show the ads on the Google Display Network.',
        },
      },
      required: ['customer_id', 'campaign_id'],
      additionalProperties: false,
    },
    input: updateCampaignValidator,
    approval: 'ask',
    plan: async (input, context) => {
      const customer = customerOf(context, input.customer_id)
      const campaign = await campaignFacts(context, customer, input.campaign_id)
      const { currency } = campaign.account
      const fields: Record<string, unknown> = {}
      const mask: string[] = []
      const rows: ApprovalDetail[] = []

      if (input.name !== undefined) {
        fields.name = input.name
        mask.push('name')
        rows.push(change('Name', input.name, campaign.name))
      }

      const changesBidding =
        input.bidding_strategy !== undefined ||
        input.max_cpc !== undefined ||
        input.target_cpa !== undefined ||
        input.target_roas !== undefined
      if (changesBidding) {
        const current =
          campaign.biddingStrategy === 'TARGET_SPEND' ? 'MAXIMIZE_CLICKS' : campaign.biddingStrategy
        const strategy = input.bidding_strategy ?? current
        const known: readonly string[] = GOOGLE_ADS_BIDDING_STRATEGIES
        if (!known.includes(strategy)) {
          throw new BuiltinToolError(
            `This campaign bids with ${campaign.biddingStrategy}, which has no target to change here. Pass bidding_strategy to move it to one of: ${GOOGLE_ADS_BIDDING_STRATEGIES.join(', ')}`
          )
        }
        const bidding = biddingOf(strategy as BiddingStrategy, input)
        Object.assign(fields, bidding.fields)
        mask.push(...bidding.mask)
        rows.push(
          change(
            'Bidding',
            biddingLabel(strategy, input, currency),
            biddingLabel(
              campaign.biddingStrategy,
              {
                max_cpc: campaign.maxCpc,
                target_cpa: campaign.targetCpa,
                target_roas: campaign.targetRoas,
              },
              currency
            )
          )
        )
      }

      const startDate = input.start_date ?? campaign.startDate
      const endDate = input.end_date ?? campaign.endDate
      if (startDate && endDate && startDate > endDate) {
        throw new BuiltinToolError('The start date must not be after the end date')
      }
      if (input.start_date !== undefined) {
        fields.startDateTime = day(input.start_date)
        mask.push('start_date_time')
        rows.push(change('First day', input.start_date, campaign.startDate ?? 'Not set'))
      }
      if (input.end_date !== undefined) {
        fields.endDateTime = day(input.end_date, true)
        mask.push('end_date_time')
        rows.push(change('Last day', input.end_date, campaign.endDate ?? 'No end date'))
      }

      const networks: Record<string, boolean> = {}
      const shown = (isShown: boolean) => (isShown ? 'Shown' : 'Not shown')
      if (input.search_partners !== undefined) {
        networks.targetSearchNetwork = input.search_partners
        mask.push('network_settings.target_search_network')
        rows.push(
          change('Search partners', shown(input.search_partners), shown(campaign.searchPartners))
        )
      }
      if (input.display_network !== undefined) {
        networks.targetContentNetwork = input.display_network
        mask.push('network_settings.target_content_network')
        rows.push(
          change('Display Network', shown(input.display_network), shown(campaign.displayNetwork))
        )
      }
      if (Object.keys(networks).length > 0) {
        if (campaign.channel !== 'SEARCH') {
          throw new BuiltinToolError(
            'search_partners and display_network are for Search campaigns only'
          )
        }
        fields.networkSettings = networks
      }

      if (mask.length === 0) {
        throw new BuiltinToolError('Pass at least one setting to change')
      }
      return {
        customer,
        operations: [
          {
            campaignOperation: {
              update: { resourceName: campaign.resourceName, ...fields },
              updateMask: mask.join(','),
            },
          },
        ],
        summary: {
          title: `Change the settings of the campaign "${campaign.name}"`,
          details: [accountRow(campaign.account), campaignRow(campaign), ...rows],
          warnings:
            changesBidding && campaign.status === 'ENABLED'
              ? [
                  'The campaign is live: the new bidding applies at once, and Google relearns for a few days.',
                ]
              : [],
        },
        result: () => ({ campaign_id: campaign.id, changed: mask }),
      }
    },
  }),
  writeTool({
    name: 'set_campaign_status',
    description:
      'Enable, pause, or remove a campaign. An enabled campaign spends its daily budget as soon as it has approved ads. A removed campaign cannot be restored.',
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        ...campaignIdProperty,
        status: { type: 'string', enum: [...GOOGLE_ADS_STATUSES] },
      },
      required: ['customer_id', 'campaign_id', 'status'],
      additionalProperties: false,
    },
    input: setCampaignStatusValidator,
    approval: 'ask',
    plan: async ({ customer_id: customerId, campaign_id: campaignId, status }, context) => {
      const customer = customerOf(context, customerId)
      const campaign = await campaignFacts(context, customer, campaignId)
      const { currency } = campaign.account
      const budget = daily(campaign.budget.amount, currency)
      const rows = [
        accountRow(campaign.account),
        { label: 'Campaign', value: campaign.name },
        change('Status', statusWord(status), statusWord(campaign.status)),
      ]

      const summaries: Record<Status, ApprovalSummary> = {
        ENABLED: {
          title: `Enable the campaign "${campaign.name}", which can spend ${budget}`,
          details: [
            ...rows,
            { label: 'Daily budget', value: budget },
            monthlyRow(campaign.budget.amount, currency),
          ],
          warnings: [`Once enabled, the campaign spends up to ${budget} without another approval.`],
        },
        PAUSED: {
          title: `Pause the campaign "${campaign.name}"`,
          details: rows,
          warnings: ['Its ads stop showing until it is enabled again.'],
        },
        REMOVED: removed('campaign', campaign.name, rows),
      }
      return {
        customer,
        operations: [
          {
            campaignOperation:
              status === 'REMOVED'
                ? { remove: campaign.resourceName }
                : { update: { resourceName: campaign.resourceName, status }, updateMask: 'status' },
          },
        ],
        summary: summaries[status],
        result: () => ({ campaign_id: campaign.id, name: campaign.name, status }),
      }
    },
  }),
  writeTool({
    name: 'update_campaign_budget',
    description: `Set the average amount a campaign spends a day. Google may spend up to twice that on a day, and never more than 30.4 times it in a month. ${MONEY}`,
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        ...campaignIdProperty,
        daily_budget: {
          type: 'number',
          minimum: 0.01,
          maximum: GOOGLE_ADS_LIMITS.dailyBudget,
          description: 'New average amount to spend a day, such as 25 or 12.5.',
        },
      },
      required: ['customer_id', 'campaign_id', 'daily_budget'],
      additionalProperties: false,
    },
    input: updateCampaignBudgetValidator,
    approval: 'ask',
    plan: async (
      { customer_id: customerId, campaign_id: campaignId, daily_budget: amount },
      context
    ) => {
      const customer = customerOf(context, customerId)
      const campaign = await campaignFacts(context, customer, campaignId)
      const { currency } = campaign.account
      const before = campaign.budget.amount
      const times = before > 0 ? amount / before : 0

      return {
        customer,
        operations: [
          {
            campaignBudgetOperation: {
              update: {
                resourceName: campaign.budget.resourceName,
                amountMicros: toMicros(amount),
              },
              updateMask: 'amount_micros',
            },
          },
        ],
        summary: {
          title: `Change the daily budget of the campaign "${campaign.name}" from ${money(before, currency)} to ${money(amount, currency)}`,
          details: [
            accountRow(campaign.account),
            campaignRow(campaign),
            change('Daily budget', daily(amount, currency), daily(before, currency)),
            change(
              monthlyRow(amount, currency).label,
              monthlyRow(amount, currency).value,
              monthlyRow(before, currency).value
            ),
          ],
          warnings: [
            ...(times >= 2
              ? [`The new budget is ${Number(times.toFixed(1))} times the current one.`]
              : []),
            ...(campaign.budget.campaigns > 1
              ? [
                  `This budget is shared by ${campaign.budget.campaigns} campaigns: the change applies to all of them.`,
                ]
              : []),
            ...(campaign.status === 'ENABLED'
              ? ['The campaign is live: the new budget applies at once.']
              : []),
          ],
        },
        result: () => ({
          campaign_id: campaign.id,
          name: campaign.name,
          daily_budget: amount,
          previous_daily_budget: before,
          currency,
        }),
      }
    },
  }),
  writeTool({
    name: 'update_campaign_targeting',
    description:
      'Change who a campaign reaches: add or remove locations, add or remove languages (Display campaigns only), add negative keywords that keep its ads away from searches, and remove any criterion by the ID get_campaign lists.',
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        ...campaignIdProperty,
        add_location_ids: idListProperty(
          GOOGLE_ADS_LIMITS.locations,
          'Locations to add, by the IDs search_locations returns.'
        ),
        remove_location_ids: idListProperty(
          GOOGLE_ADS_LIMITS.locations,
          'Locations to stop targeting. An excluded location is not one: remove its exclusion with remove_criterion_ids.'
        ),
        add_languages: idListProperty(
          GOOGLE_ADS_LIMITS.languages,
          'Display campaigns only: languages to add, as codes such as en or fr.'
        ),
        remove_languages: idListProperty(
          GOOGLE_ADS_LIMITS.languages,
          'Languages to stop targeting.'
        ),
        add_negative_keywords: keywordListProperty(
          'Searches the campaign must not show for. A negative keyword takes no max_cpc.'
        ),
        remove_criterion_ids: idListProperty(
          GOOGLE_ADS_LIMITS.criteria,
          'Criteria to remove, such as negative keywords, by the criterion IDs get_campaign returns.'
        ),
      },
      required: ['customer_id', 'campaign_id'],
      additionalProperties: false,
    },
    input: updateCampaignTargetingValidator,
    plan: async (input, context) => {
      const customer = customerOf(context, input.customer_id)
      const campaign = await campaignFacts(context, customer, input.campaign_id)
      if (campaign.channel === 'SEARCH' && input.add_languages) throw noLanguagesForSearch()
      const negatives = input.add_negative_keywords ?? []
      if (negatives.some((keyword) => keyword.maxCpc !== undefined)) {
        throw new BuiltinToolError('A negative keyword takes no max_cpc')
      }

      const addedLocations = await locationsById(context, customer, input.add_location_ids ?? [])
      const removedLocations = await locationsById(
        context,
        customer,
        input.remove_location_ids ?? []
      )
      const addedLanguages = await languagesByCode(context, customer, input.add_languages ?? [])
      const removedLanguages = await languagesByCode(
        context,
        customer,
        input.remove_languages ?? []
      )

      // What is removed is read from the campaign first: each criterion goes
      // by the name Google gives it, and only as what the agent said it is.
      const criterionIds = input.remove_criterion_ids ?? []
      const criterionFields =
        'campaign_criterion.resource_name, campaign_criterion.criterion_id, campaign_criterion.type, campaign_criterion.negative, campaign_criterion.display_name, campaign_criterion.keyword.text, campaign_criterion.location.geo_target_constant, campaign_criterion.language.language_constant'
      const targets =
        removedLocations.length + removedLanguages.length > 0
          ? await searchGoogleAds(
              context,
              customer,
              `SELECT ${criterionFields} FROM campaign_criterion WHERE campaign.id = ${campaign.id} AND campaign_criterion.type IN ('LOCATION', 'LANGUAGE')`,
              MAX_CAMPAIGN_TARGETS
            )
          : { rows: [] }
      const named =
        criterionIds.length > 0
          ? await searchGoogleAds(
              context,
              customer,
              `SELECT ${criterionFields} FROM campaign_criterion WHERE campaign.id = ${campaign.id} AND campaign_criterion.criterion_id IN (${criterionIds.join(', ')})`,
              criterionIds.length
            )
          : { rows: [] }

      const targeted = targets.rows.map((row) => row.campaignCriterion!)
      const locationCriteria = removedLocations.map(({ id, name }) => {
        const criterion = targeted.find(
          (candidate) => candidate.location?.geoTargetConstant === `geoTargetConstants/${id}`
        )
        if (!criterion) {
          throw new BuiltinToolError(`The campaign ${campaign.id} does not target ${name}`)
        }
        // Removing an exclusion opens a place instead of closing one.
        if (criterion.negative) {
          throw new BuiltinToolError(
            `The campaign ${campaign.id} excludes ${name}, and removing that exclusion would let its ads show there. If that is what is wanted, pass its criterion ID ${criterion.criterionId} in remove_criterion_ids.`
          )
        }
        return criterion
      })
      const languageCriteria = removedLanguages.map(({ id, name }) => {
        const criterion = targeted.find(
          (candidate) => candidate.language?.languageConstant === `languageConstants/${id}`
        )
        if (!criterion) {
          throw new BuiltinToolError(`The campaign ${campaign.id} does not target ${name}`)
        }
        return criterion
      })
      const otherCriteria = criterionIds.map((id) => {
        const criterion = named.rows
          .map((row) => row.campaignCriterion!)
          .find((candidate) => String(candidate.criterionId) === id)
        if (!criterion) {
          throw new BuiltinToolError(
            `The campaign ${campaign.id} has no criterion ${id}. get_campaign lists its criteria.`
          )
        }
        return criterion
      })
      const criterionLabel = (criterion: Record<string, any>) => {
        const kind = String(criterion.type).toLowerCase()
        const what =
          criterion.type === 'KEYWORD'
            ? 'negative keyword'
            : criterion.negative
              ? `excluded ${kind}`
              : kind
        return `${criterion.keyword?.text ?? criterion.displayName ?? criterion.criterionId} (${what})`
      }

      const create = (fields: Record<string, unknown>): GoogleAdsOperation => ({
        campaignCriterionOperation: { create: { campaign: campaign.resourceName, ...fields } },
      })
      const remove = (criterion: Record<string, any>): GoogleAdsOperation => ({
        campaignCriterionOperation: { remove: criterion.resourceName },
      })
      const operations = [
        ...[...locationCriteria, ...languageCriteria, ...otherCriteria].map(remove),
        ...addedLocations.map(({ id }) =>
          create({ location: { geoTargetConstant: `geoTargetConstants/${id}` } })
        ),
        ...addedLanguages.map(({ id }) =>
          create({ language: { languageConstant: `languageConstants/${id}` } })
        ),
        ...negatives.map(({ text, matchType }) =>
          create({ negative: true, keyword: { text, matchType } })
        ),
      ]
      if (operations.length === 0) {
        throw new BuiltinToolError('Pass at least one thing to add or remove')
      }

      const names = (items: Array<{ name: string }>) => items.map(({ name }) => name).join('; ')
      return {
        customer,
        operations,
        summary: {
          title: `Change the targeting of the campaign "${campaign.name}"`,
          details: [
            accountRow(campaign.account),
            campaignRow(campaign),
            ...(addedLocations.length
              ? [{ label: 'Add locations', value: names(addedLocations) }]
              : []),
            ...(removedLocations.length
              ? [{ label: 'Remove locations', value: names(removedLocations) }]
              : []),
            ...(addedLanguages.length
              ? [{ label: 'Add languages', value: names(addedLanguages) }]
              : []),
            ...(removedLanguages.length
              ? [{ label: 'Remove languages', value: names(removedLanguages) }]
              : []),
            ...listed(
              'Add negative keyword',
              negatives.map((keyword) => keywordLabel(keyword, campaign.account.currency))
            ),
            ...listed('Remove', otherCriteria.map(criterionLabel)),
          ],
        },
        result: () => ({
          campaign_id: campaign.id,
          added: addedLocations.length + addedLanguages.length + negatives.length,
          removed: removedLocations.length + removedLanguages.length + criterionIds.length,
        }),
      }
    },
  }),
  writeTool({
    name: 'create_ad_group',
    description: `Create an ad group in a campaign, to hold ads and, in a Search campaign, the keywords they show for. ${MONEY}`,
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        ...campaignIdProperty,
        name: {
          type: 'string',
          maxLength: GOOGLE_ADS_LIMITS.nameLength,
          description: 'Ad group name. No other ad group of the campaign may have it.',
        },
        max_cpc: {
          type: 'number',
          minimum: 0.01,
          maximum: GOOGLE_ADS_LIMITS.bid,
          description:
            'The most a click may cost, for the keywords that set no bid of their own. Used by campaigns that bid with MANUAL_CPC.',
        },
        status: { type: 'string', enum: ['ENABLED', 'PAUSED'], default: 'ENABLED' },
      },
      required: ['customer_id', 'campaign_id', 'name'],
      additionalProperties: false,
    },
    input: createAdGroupValidator,
    plan: async (input, context) => {
      const customer = customerOf(context, input.customer_id)
      const campaign = await campaignFacts(context, customer, input.campaign_id)
      const types: Record<string, string> = {
        SEARCH: 'SEARCH_STANDARD',
        DISPLAY: 'DISPLAY_STANDARD',
      }
      const type = types[campaign.channel]
      if (!type) {
        throw new BuiltinToolError(
          `This MCP creates ad groups in Search and Display campaigns, and the campaign ${campaign.id} is a ${campaign.channel} one`
        )
      }

      const status = input.status ?? 'ENABLED'
      const { currency } = campaign.account
      return {
        customer,
        operations: [
          {
            adGroupOperation: {
              create: {
                campaign: campaign.resourceName,
                name: input.name,
                status,
                type,
                ...(input.max_cpc === undefined ? {} : { cpcBidMicros: toMicros(input.max_cpc) }),
              },
            },
          },
        ],
        summary: {
          title: `Create the ad group "${input.name}" in the campaign "${campaign.name}"`,
          details: [
            accountRow(campaign.account),
            campaignRow(campaign),
            { label: 'Ad group name', value: input.name },
            ...(input.max_cpc === undefined
              ? []
              : [{ label: 'Most a click may cost', value: money(input.max_cpc, currency) }]),
            { label: 'Status', value: statusWord(status) },
          ],
        },
        result: ([created]) => ({ ad_group_id: resourceId(created), name: input.name, status }),
      }
    },
  }),
  writeTool({
    name: 'update_ad_group',
    description: `Change an ad group: its name, its default bid, or its status. Only what you pass is changed. A removed ad group cannot be restored. ${MONEY}`,
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        ...adGroupIdProperty,
        name: { type: 'string', maxLength: GOOGLE_ADS_LIMITS.nameLength, description: 'New name.' },
        max_cpc: {
          type: 'number',
          minimum: 0.01,
          maximum: GOOGLE_ADS_LIMITS.bid,
          description: 'New most a click may cost.',
        },
        status: { type: 'string', enum: [...GOOGLE_ADS_STATUSES] },
      },
      required: ['customer_id', 'ad_group_id'],
      additionalProperties: false,
    },
    input: updateAdGroupValidator,
    plan: async (input, context) => {
      const customer = customerOf(context, input.customer_id)
      const adGroup = await adGroupFacts(context, customer, input.ad_group_id)
      const { currency } = adGroup.account
      const where = [accountRow(adGroup.account), campaignRow(adGroup.campaign)]

      if (input.status === 'REMOVED') {
        if (input.name !== undefined || input.max_cpc !== undefined) {
          throw new BuiltinToolError('An ad group that is removed takes no other change')
        }
        return {
          customer,
          operations: [{ adGroupOperation: { remove: adGroup.resourceName } }],
          summary: removed('ad group', adGroup.name, [
            ...where,
            { label: 'Ad group', value: adGroup.name },
          ]),
          result: () => ({ ad_group_id: adGroup.id, status: 'REMOVED' }),
        }
      }

      const fields: Record<string, unknown> = {}
      const mask: string[] = []
      const rows: ApprovalDetail[] = []
      if (input.name !== undefined) {
        fields.name = input.name
        mask.push('name')
        rows.push(change('Name', input.name, adGroup.name))
      }
      if (input.max_cpc !== undefined) {
        fields.cpcBidMicros = toMicros(input.max_cpc)
        mask.push('cpc_bid_micros')
        rows.push(
          change(
            'Most a click may cost',
            money(input.max_cpc, currency),
            adGroup.maxCpc === undefined ? 'Not set' : money(adGroup.maxCpc, currency)
          )
        )
      }
      if (input.status !== undefined) {
        fields.status = input.status
        mask.push('status')
        rows.push(change('Status', statusWord(input.status), statusWord(adGroup.status)))
      }
      if (mask.length === 0) {
        throw new BuiltinToolError('Pass at least one field to change')
      }

      return {
        customer,
        operations: [
          {
            adGroupOperation: {
              update: { resourceName: adGroup.resourceName, ...fields },
              updateMask: mask.join(','),
            },
          },
        ],
        summary: {
          title: `Change the ad group "${adGroup.name}"`,
          details: [...where, { label: 'Ad group', value: adGroup.name }, ...rows],
        },
        result: () => ({ ad_group_id: adGroup.id, changed: mask }),
      }
    },
  }),
  writeTool({
    name: 'add_keywords',
    description: `Add keywords to an ad group of a Search campaign: the searches its ads show for, or with negative the searches they must not show for. Every keyword says how closely a search has to match it. ${MONEY}`,
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        ...adGroupIdProperty,
        keywords: keywordListProperty(
          'Keywords to add. EXACT matches searches with the same meaning, PHRASE searches that include that meaning, and BROAD searches related to it. max_cpc is the most a click may cost for this keyword, when the campaign bids with MANUAL_CPC.'
        ),
        negative: {
          type: 'boolean',
          default: false,
          description: 'Add them as negative keywords, which take no max_cpc.',
        },
      },
      required: ['customer_id', 'ad_group_id', 'keywords'],
      additionalProperties: false,
    },
    input: addKeywordsValidator,
    plan: async (
      { customer_id: customerId, ad_group_id: adGroupId, keywords, negative = false },
      context
    ) => {
      const customer = customerOf(context, customerId)
      if (negative && keywords.some((keyword) => keyword.maxCpc !== undefined)) {
        throw new BuiltinToolError('A negative keyword takes no max_cpc')
      }
      const adGroup = await adGroupFacts(context, customer, adGroupId)
      const { currency } = adGroup.account
      const what = `${keywords.length} ${negative ? 'negative ' : ''}${keywords.length === 1 ? 'keyword' : 'keywords'}`

      return {
        customer,
        operations: keywords.map(({ text, matchType, maxCpc }) => ({
          adGroupCriterionOperation: {
            create: {
              adGroup: adGroup.resourceName,
              keyword: { text, matchType },
              ...(negative ? { negative: true } : { status: 'ENABLED' }),
              ...(maxCpc === undefined ? {} : { cpcBidMicros: toMicros(maxCpc) }),
            },
          },
        })),
        summary: {
          title: `Add ${what} to the ad group "${adGroup.name}"`,
          details: [
            ...adGroupRows(adGroup),
            ...listed(
              negative ? 'Negative keyword' : 'Keyword',
              keywords.map((keyword) => keywordLabel(keyword, currency))
            ),
          ],
        },
        result: (created) => ({
          ad_group_id: adGroup.id,
          keywords: keywords.map(({ text, matchType }, index) => ({
            criterion_id: resourceId(created[index]),
            text,
            match_type: matchType,
          })),
        }),
      }
    },
  }),
  writeTool({
    name: 'update_keyword',
    description: `Change a keyword of an ad group: pause it, enable it, remove it, or set the most a click may cost for it. The text and match type of a keyword cannot be changed: remove it and add another. ${MONEY}`,
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        ...adGroupIdProperty,
        criterion_id: { type: 'string', description: 'Keyword ID, as returned by list_keywords.' },
        max_cpc: {
          type: 'number',
          minimum: 0.01,
          maximum: GOOGLE_ADS_LIMITS.bid,
          description: 'New most a click may cost.',
        },
        status: { type: 'string', enum: [...GOOGLE_ADS_STATUSES] },
      },
      required: ['customer_id', 'ad_group_id', 'criterion_id'],
      additionalProperties: false,
    },
    input: updateKeywordValidator,
    plan: async (input, context) => {
      const customer = customerOf(context, input.customer_id)
      const keyword = await keywordFacts(context, customer, input.ad_group_id, input.criterion_id)
      const { currency } = keyword.adGroup.account
      const label = `"${keyword.text}" (${MATCH_WORDS[keyword.matchType] ?? keyword.matchType}${keyword.negative ? ', negative' : ''})`
      const rows = [...adGroupRows(keyword.adGroup), { label: 'Keyword', value: label }]

      if (input.status === 'REMOVED') {
        if (input.max_cpc !== undefined) {
          throw new BuiltinToolError('A keyword that is removed takes no other change')
        }
        return {
          customer,
          operations: [{ adGroupCriterionOperation: { remove: keyword.resourceName } }],
          summary: removed('keyword', keyword.text, rows),
          result: () => ({ criterion_id: input.criterion_id, status: 'REMOVED' }),
        }
      }
      if (keyword.negative) {
        throw new BuiltinToolError('A negative keyword can only be removed')
      }

      const fields: Record<string, unknown> = {}
      const mask: string[] = []
      if (input.max_cpc !== undefined) {
        fields.cpcBidMicros = toMicros(input.max_cpc)
        mask.push('cpc_bid_micros')
        rows.push(
          change(
            'Most a click may cost',
            money(input.max_cpc, currency),
            keyword.maxCpc === undefined
              ? 'The bid of its ad group'
              : money(keyword.maxCpc, currency)
          )
        )
      }
      if (input.status !== undefined) {
        fields.status = input.status
        mask.push('status')
        rows.push(change('Status', statusWord(input.status), statusWord(keyword.status)))
      }
      return {
        customer,
        operations: [
          {
            adGroupCriterionOperation: {
              update: { resourceName: keyword.resourceName, ...fields },
              updateMask: mask.join(','),
            },
          },
        ],
        summary: { title: `Change the keyword "${keyword.text}"`, details: rows },
        result: () => ({ criterion_id: input.criterion_id, changed: mask }),
      }
    },
  }),
  writeTool({
    name: 'create_responsive_search_ad',
    description:
      'Create a text ad in an ad group of a Search campaign. Google shows up to 3 of the headlines and 2 of the descriptions at a time, in the combinations that perform best, so each must make sense on its own and none may repeat another. Google reviews an ad before it shows.',
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        ...adGroupIdProperty,
        headlines: textListProperty(
          3,
          GOOGLE_ADS_LIMITS.headlines,
          GOOGLE_ADS_LIMITS.headlineLength,
          'Headlines, all different.'
        ),
        descriptions: textListProperty(
          2,
          GOOGLE_ADS_LIMITS.descriptions,
          GOOGLE_ADS_LIMITS.descriptionLength,
          'Descriptions, all different.'
        ),
        final_url: { type: 'string', description: 'The page a click opens.' },
        path1: {
          type: 'string',
          description: `First part of the path shown after the domain, such as "shoes" in example.com/shoes/running. At most ${GOOGLE_ADS_LIMITS.pathLength} characters.`,
        },
        path2: { type: 'string', description: 'Second part of that path. Set it with path1.' },
        status: { type: 'string', enum: ['ENABLED', 'PAUSED'], default: 'ENABLED' },
      },
      required: ['customer_id', 'ad_group_id', 'headlines', 'descriptions', 'final_url'],
      additionalProperties: false,
    },
    input: createSearchAdValidator,
    plan: async (input, context) => {
      const customer = customerOf(context, input.customer_id)
      if (input.path2 && !input.path1) {
        throw new BuiltinToolError('path2 is the second part of the path: set path1 as well')
      }
      const adGroup = await adGroupFacts(context, customer, input.ad_group_id)
      const status = input.status ?? 'ENABLED'

      return {
        customer,
        operations: [
          {
            adGroupAdOperation: {
              create: {
                adGroup: adGroup.resourceName,
                status,
                ad: {
                  finalUrls: [input.final_url],
                  responsiveSearchAd: {
                    headlines: input.headlines.map((text) => ({ text })),
                    descriptions: input.descriptions.map((text) => ({ text })),
                    ...(input.path1 ? { path1: input.path1 } : {}),
                    ...(input.path2 ? { path2: input.path2 } : {}),
                  },
                },
              },
            },
          },
        ],
        summary: {
          title: `Create a search ad in the ad group "${adGroup.name}"`,
          details: [
            ...adGroupRows(adGroup),
            { label: 'Opens', value: input.final_url },
            ...(input.path1
              ? [
                  {
                    label: 'Path shown',
                    value: [input.path1, input.path2].filter(Boolean).join('/'),
                  },
                ]
              : []),
            ...listed('Headline', input.headlines),
            ...listed('Description', input.descriptions),
            { label: 'Status', value: statusWord(status) },
          ],
        },
        result: ([created]) => ({ ad_id: resourceId(created), ad_group_id: adGroup.id, status }),
      }
    },
  }),
  writeTool({
    name: 'create_responsive_display_ad',
    description:
      'Create an image ad in an ad group of a Display campaign, from image assets and texts that Google assembles to fit each placement. It needs at least one landscape image (1.91:1, 600×314 pixels or more) and one square image (1:1, 300×300 or more): add images with create_image_upload_link and create_image_asset, or find them with list_assets. Google reviews an ad before it shows.',
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        ...adGroupIdProperty,
        marketing_image_asset_ids: idListProperty(
          GOOGLE_ADS_LIMITS.images,
          'Landscape images (1.91:1, at least 600×314), by asset ID.'
        ),
        square_marketing_image_asset_ids: idListProperty(
          GOOGLE_ADS_LIMITS.images,
          'Square images (1:1, at least 300×300), by asset ID. With the landscape ones, at most 15.'
        ),
        square_logo_asset_ids: idListProperty(
          GOOGLE_ADS_LIMITS.logos,
          'Square logos (1:1, at least 128×128), by asset ID.'
        ),
        wide_logo_asset_ids: idListProperty(
          GOOGLE_ADS_LIMITS.logos,
          'Wide logos (4:1, at least 512×128), by asset ID. With the square ones, at most 5.'
        ),
        headlines: textListProperty(
          1,
          GOOGLE_ADS_LIMITS.displayTexts,
          GOOGLE_ADS_LIMITS.headlineLength,
          'Short headlines.'
        ),
        long_headline: {
          type: 'string',
          maxLength: GOOGLE_ADS_LIMITS.longHeadlineLength,
          description: 'The headline shown where there is room for a longer one.',
        },
        descriptions: textListProperty(
          1,
          GOOGLE_ADS_LIMITS.displayTexts,
          GOOGLE_ADS_LIMITS.descriptionLength,
          'Descriptions.'
        ),
        business_name: {
          type: 'string',
          maxLength: GOOGLE_ADS_LIMITS.businessNameLength,
          description: 'The name of the advertiser, as the ad shows it.',
        },
        final_url: { type: 'string', description: 'The page a click opens.' },
        status: { type: 'string', enum: ['ENABLED', 'PAUSED'], default: 'ENABLED' },
      },
      required: [
        'customer_id',
        'ad_group_id',
        'marketing_image_asset_ids',
        'square_marketing_image_asset_ids',
        'headlines',
        'long_headline',
        'descriptions',
        'business_name',
        'final_url',
      ],
      additionalProperties: false,
    },
    input: createDisplayAdValidator,
    plan: async (input, context) => {
      const customer = customerOf(context, input.customer_id)
      const squareLogoIds = input.square_logo_asset_ids ?? []
      const wideLogoIds = input.wide_logo_asset_ids ?? []
      if (
        input.marketing_image_asset_ids.length + input.square_marketing_image_asset_ids.length >
        GOOGLE_ADS_LIMITS.images
      ) {
        throw new BuiltinToolError(
          `An ad takes at most ${GOOGLE_ADS_LIMITS.images} landscape and square images together`
        )
      }
      if (squareLogoIds.length + wideLogoIds.length > GOOGLE_ADS_LIMITS.logos) {
        throw new BuiltinToolError(`An ad takes at most ${GOOGLE_ADS_LIMITS.logos} logos together`)
      }

      const adGroup = await adGroupFacts(context, customer, input.ad_group_id)
      const assets = await imageAssetsById(context, customer, [
        ...input.marketing_image_asset_ids,
        ...input.square_marketing_image_asset_ids,
        ...squareLogoIds,
        ...wideLogoIds,
      ])
      const byId = new Map(assets.map((asset) => [asset.id, asset]))
      const images = (ids: string[], argument: string, use: Parameters<typeof requireImage>[2]) =>
        ids.map((id) => requireImage(byId.get(id)!, argument, use))
      const landscape = images(
        input.marketing_image_asset_ids,
        'marketing_image_asset_ids',
        LANDSCAPE
      )
      const square = images(
        input.square_marketing_image_asset_ids,
        'square_marketing_image_asset_ids',
        SQUARE
      )
      const squareLogos = images(squareLogoIds, 'square_logo_asset_ids', SQUARE_LOGO)
      const wideLogos = images(wideLogoIds, 'wide_logo_asset_ids', WIDE_LOGO)

      const status = input.status ?? 'ENABLED'
      const linked = (items: ImageAssetFacts[]) =>
        items.map((asset) => ({ asset: asset.resourceName }))
      const shown = (label: string, items: ImageAssetFacts[]) =>
        items.length ? [{ label, value: items.map(imageLabel).join('; ') }] : []
      return {
        customer,
        operations: [
          {
            adGroupAdOperation: {
              create: {
                adGroup: adGroup.resourceName,
                status,
                ad: {
                  finalUrls: [input.final_url],
                  responsiveDisplayAd: {
                    marketingImages: linked(landscape),
                    squareMarketingImages: linked(square),
                    ...(squareLogos.length ? { squareLogoImages: linked(squareLogos) } : {}),
                    ...(wideLogos.length ? { logoImages: linked(wideLogos) } : {}),
                    headlines: input.headlines.map((text) => ({ text })),
                    longHeadline: { text: input.long_headline },
                    descriptions: input.descriptions.map((text) => ({ text })),
                    businessName: input.business_name,
                  },
                },
              },
            },
          },
        ],
        summary: {
          title: `Create a display ad in the ad group "${adGroup.name}"`,
          details: [
            ...adGroupRows(adGroup),
            { label: 'Opens', value: input.final_url },
            { label: 'Business name', value: input.business_name },
            ...shown('Landscape images', landscape),
            ...shown('Square images', square),
            ...shown('Logos', [...squareLogos, ...wideLogos]),
            ...listed('Headline', input.headlines),
            { label: 'Long headline', value: input.long_headline },
            ...listed('Description', input.descriptions),
            { label: 'Status', value: statusWord(status) },
          ],
        },
        result: ([created]) => ({ ad_id: resourceId(created), ad_group_id: adGroup.id, status }),
      }
    },
  }),
  writeTool({
    name: 'set_ad_status',
    description:
      'Enable, pause, or remove an ad. The texts and images of an ad cannot be changed here: create a new ad and pause or remove the old one. A removed ad cannot be restored.',
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        ...adGroupIdProperty,
        ad_id: { type: 'string', description: 'Ad ID, as returned by list_ads.' },
        status: { type: 'string', enum: [...GOOGLE_ADS_STATUSES] },
      },
      required: ['customer_id', 'ad_group_id', 'ad_id', 'status'],
      additionalProperties: false,
    },
    input: setAdStatusValidator,
    plan: async (
      { customer_id: customerId, ad_group_id: adGroupId, ad_id: adId, status },
      context
    ) => {
      const customer = customerOf(context, customerId)
      const ad = await adFacts(context, customer, adGroupId, adId)
      const name = ad.headline ?? `Ad ${adId}`
      const rows = [
        ...adGroupRows(ad.adGroup),
        { label: 'Ad', value: `${name} (ad ${adId})` },
        change('Status', statusWord(status), statusWord(ad.status)),
      ]
      const verbs: Record<Status, string> = {
        ENABLED: 'Enable',
        PAUSED: 'Pause',
        REMOVED: 'Remove',
      }

      return {
        customer,
        operations: [
          {
            adGroupAdOperation:
              status === 'REMOVED'
                ? { remove: ad.resourceName }
                : { update: { resourceName: ad.resourceName, status }, updateMask: 'status' },
          },
        ],
        summary:
          status === 'REMOVED'
            ? removed('ad', name, rows)
            : { title: `${verbs[status]} the ad "${name}"`, details: rows },
        result: () => ({ ad_id: adId, ad_group_id: adGroupId, status }),
      }
    },
  }),
  builtinTool({
    name: 'create_image_upload_link',
    write: true,
    description: `Get a temporary link to upload one image, so that create_image_asset can add it to a Google Ads account. Send the file as the body of a PUT request to the link, for example with \`curl -T banner.png "<url>"\`, then pass \`upload_id\` to create_image_asset. The link takes one JPEG, PNG, or GIF file of at most ${MAX_IMAGE_MEGABYTES} MB, without signing in, from anyone who has it, until it expires: use it yourself or give it to the user, and never post it anywhere else. An uploaded image can be used for ${BUILTIN_UPLOAD_MINUTES} minutes.`,
    inputSchema: {
      type: 'object',
      properties: {
        filename: {
          type: 'string',
          maxLength: GOOGLE_ADS_LIMITS.filenameLength,
          description: 'Name of the file, such as banner.png, without a folder.',
        },
        content_type: {
          type: 'string',
          description:
            'Media type of the file, such as image/png. Defaults to the one of its extension.',
        },
        expires_in_minutes: {
          type: 'integer',
          minimum: 1,
          maximum: GOOGLE_ADS_LIMITS.linkMinutes,
          default: DEFAULT_LINK_MINUTES,
          description: 'How long the link takes a file.',
        },
      },
      required: ['filename'],
      additionalProperties: false,
    },
    input: createImageUploadLinkValidator,
    run: async (
      { filename, content_type: contentType, expires_in_minutes: minutes = DEFAULT_LINK_MINUTES },
      { mcpId }: BuiltinToolContext
    ) => {
      const expiresInMs = minutes * 60_000
      const uploadId = randomUUID()
      return {
        upload_id: uploadId,
        url: builtinUploadUrl(
          mcpId,
          { upload: uploadId, filename, content_type: contentType },
          expiresInMs
        ),
        method: 'PUT',
        expires_at: new Date(Date.now() + expiresInMs).toISOString(),
        filename,
        max_bytes: GOOGLE_ADS_LIMITS.imageBytes,
      }
    },
  }),
  writeTool({
    name: 'create_image_asset',
    description:
      'Add an uploaded image to the assets of a Google Ads account, where ads can use it. Returns its asset ID and what Google Ads can use it as: a landscape image (1.91:1), a square image or logo (1:1), a wide logo (4:1), or a portrait image (4:5). An asset cannot be deleted or changed afterwards.',
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        upload_id: {
          type: 'string',
          description:
            'The upload_id create_image_upload_link returned, once the file was sent to its link.',
        },
        name: {
          type: 'string',
          maxLength: GOOGLE_ADS_LIMITS.nameLength,
          description:
            'Name of the asset in the account, such as "Spring sale banner". No other image asset may have it.',
        },
      },
      required: ['customer_id', 'upload_id', 'name'],
      additionalProperties: false,
    },
    input: createImageAssetValidator,
    plan: async ({ customer_id: customerId, upload_id: uploadId, name }, context) => {
      const customer = customerOf(context, customerId)
      const upload = await findBuiltinUpload(context.mcpId, uploadId)
      const bytes = upload ? await readBuiltinUpload(context.mcpId, uploadId) : null
      if (!upload || !bytes) {
        throw new BuiltinToolError(
          `No file is uploaded as "${uploadId}". Send the image to the link create_image_upload_link returned with this upload_id, then try again. An uploaded image can be used for ${BUILTIN_UPLOAD_MINUTES} minutes.`
        )
      }
      const image = imageInfo(bytes)
      if (!image) {
        throw new BuiltinToolError(
          `${upload.filename} is not a JPEG, PNG, or GIF image, which is what Google Ads takes`
        )
      }
      const shape = imageShape(image)
      if (!shape) {
        throw new BuiltinToolError(
          `${upload.filename} is ${image.width}×${image.height} pixels, a shape Google Ads has no use for. It takes ${IMAGE_SHAPES.map(({ label }) => label).join(', ')} images.`
        )
      }

      const account = await accountFacts(context, customer)
      const size = `${image.width}×${image.height} pixels`
      return {
        customer,
        operations: [
          {
            assetOperation: {
              create: { name, type: 'IMAGE', imageAsset: { data: bytes.toString('base64') } },
            },
          },
        ],
        summary: {
          title: `Add the image "${name}" to the assets of ${account.label}`,
          details: [
            accountRow(account),
            { label: 'Asset name', value: name },
            {
              label: 'File',
              value: `${upload.filename} (${image.format.toUpperCase()}, ${Math.ceil(upload.size / 1000)} KB)`,
            },
            { label: 'Size', value: `${size}, ${shape.label}` },
          ],
          warnings: ['An asset cannot be deleted from Google Ads once it is added.'],
        },
        result: ([created]) => ({
          asset_id: resourceId(created),
          name,
          width: image.width,
          height: image.height,
          shape: shape.name,
          // Google also takes smaller squares as logos, and nothing below these sizes otherwise.
          large_enough:
            image.width >= shape.minWidth && image.height >= shape.minHeight
              ? true
              : `Below the ${shape.minWidth}×${shape.minHeight} pixels Google Ads asks of ${shape.label} images${shape.name === 'square' && image.width >= SQUARE_LOGO_MIN_PIXELS ? ', but usable as a logo' : ''}`,
        }),
      }
    },
  }),
  writeTool({
    name: 'add_campaign_images',
    description:
      'Show images beside the text ads of a Search campaign, from image assets of the account. They must be square (1:1, at least 300×300 pixels) or landscape (1.91:1, at least 600×314). Google reviews them, and only shows images for advertisers it has verified.',
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        ...campaignIdProperty,
        asset_ids: idListProperty(GOOGLE_ADS_LIMITS.images, 'Image assets to show, by asset ID.'),
      },
      required: ['customer_id', 'campaign_id', 'asset_ids'],
      additionalProperties: false,
    },
    input: addCampaignImagesValidator,
    plan: async (
      { customer_id: customerId, campaign_id: campaignId, asset_ids: assetIds },
      context
    ) => {
      const customer = customerOf(context, customerId)
      const campaign = await campaignFacts(context, customer, campaignId)
      const assets = await imageAssetsById(context, customer, assetIds)
      for (const asset of assets) {
        requireImage(
          asset,
          'asset_ids',
          imageShape(asset)?.name === 'landscape' ? LANDSCAPE : SQUARE
        )
      }

      return {
        customer,
        operations: assets.map((asset) => ({
          campaignAssetOperation: {
            create: {
              campaign: campaign.resourceName,
              asset: asset.resourceName,
              fieldType: 'AD_IMAGE',
            },
          },
        })),
        summary: {
          title: `Show ${assets.length} ${assets.length === 1 ? 'image' : 'images'} with the ads of the campaign "${campaign.name}"`,
          details: [
            accountRow(campaign.account),
            campaignRow(campaign),
            ...listed('Image', assets.map(imageLabel)),
          ],
        },
        result: () => ({ campaign_id: campaign.id, asset_ids: assetIds }),
      }
    },
  }),
]
