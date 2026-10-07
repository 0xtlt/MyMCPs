import { DateTime } from 'luxon'
import {
  BuiltinToolError,
  type BuiltinTool,
  type BuiltinToolContext,
} from '#services/builtin/definition'
import {
  allowedCustomers,
  customerOf,
  googleAdsRequest,
  searchGoogleAds,
  type GoogleAdsRow,
} from '#services/builtin/google_ads/api'
import {
  customerLabel,
  customerNumber,
  fromMicros,
  languageCode,
  percent,
} from '#services/builtin/google_ads/format'
import { languagesByCode } from '#services/builtin/google_ads/lookup'
import { builtinTool } from '#services/builtin/tool_input'
import {
  campaignValidator,
  getPerformanceValidator,
  GOOGLE_ADS_ASSET_TYPES,
  GOOGLE_ADS_DATE_RANGES,
  GOOGLE_ADS_LIMITS,
  GOOGLE_ADS_PERFORMANCE_LEVELS,
  GOOGLE_ADS_SEGMENTS,
  keywordIdeasValidator,
  listAdGroupsValidator,
  listAssetsValidator,
  listCampaignsValidator,
  listChangesValidator,
  listInCampaignValidator,
  runQueryValidator,
  searchLocationsValidator,
} from '#validators/builtin_google_ads'
import { noArgumentsValidator } from '#validators/builtin_tools'

const MONEY = "Amounts are in the account's currency, which each result names."
const DEFAULT_ROWS = 100
const DEFAULT_DATE_RANGE = 'LAST_30_DAYS'
const MAX_LISTED_ACCOUNTS = 50
const MAX_LISTED_CLIENTS = 200

/** Averages are worked out from these, so that no unit is left to guess. */
const METRICS =
  'metrics.impressions, metrics.clicks, metrics.cost_micros, metrics.ctr, metrics.conversions, metrics.conversions_value'

const customerIdProperty = {
  customer_id: {
    type: 'string',
    description: 'Google Ads account ID, such as 123-456-7890, as returned by list_accounts.',
  },
} as const

const periodProperties = {
  date_range: {
    type: 'string',
    enum: [...GOOGLE_ADS_DATE_RANGES],
    default: DEFAULT_DATE_RANGE,
    description:
      "The days the figures cover, in the account's time zone. LAST_n_DAYS ranges end yesterday.",
  },
  start_date: {
    type: 'string',
    description: 'First day of a custom period, such as 2026-01-01. Set it with end_date.',
  },
  end_date: {
    type: 'string',
    description: 'Last day of a custom period, such as 2026-01-31. Set it with start_date.',
  },
} as const

const limitProperty = (what: string) =>
  ({
    limit: {
      type: 'integer',
      minimum: 1,
      maximum: GOOGLE_ADS_LIMITS.rows,
      default: DEFAULT_ROWS,
      description: `Most ${what} to return.`,
    },
  }) as const

const campaignFilterProperty = {
  campaign_id: { type: 'string', description: 'Only what belongs to this campaign.' },
} as const

const adGroupFilterProperty = {
  ad_group_id: { type: 'string', description: 'Only what belongs to this ad group.' },
} as const

type Period = { date_range?: string; start_date?: string; end_date?: string }

/** The days a report covers, as a GAQL condition and as the result says them back. */
function periodOf({ date_range: range, start_date: start, end_date: end }: Period) {
  if (!start && !end) {
    const named = range ?? DEFAULT_DATE_RANGE
    return { where: `segments.date DURING ${named}`, period: named }
  }
  if (!start || !end) {
    throw new BuiltinToolError('Set both start_date and end_date, or neither and use date_range')
  }
  if (start > end) {
    throw new BuiltinToolError('start_date must not be after end_date')
  }
  return { where: `segments.date BETWEEN '${start}' AND '${end}'`, period: `${start} to ${end}` }
}

function round(value: number, digits = 2) {
  const factor = 10 ** digits
  return Math.round(value * factor) / factor
}

/** The figures of a row, in whole units of the account's currency. */
function figures(metrics: Record<string, any> = {}) {
  const clicks = Number(metrics.clicks ?? 0)
  const cost = fromMicros(metrics.costMicros) ?? 0
  const conversions = Number(metrics.conversions ?? 0)
  return {
    impressions: Number(metrics.impressions ?? 0),
    clicks,
    cost: round(cost),
    ctr_percent: percent(metrics.ctr) ?? 0,
    average_cpc: clicks > 0 ? round(cost / clicks) : null,
    conversions: round(conversions),
    conversion_value: round(Number(metrics.conversionsValue ?? 0)),
    cost_per_conversion: conversions > 0 ? round(cost / conversions) : null,
  }
}

function filters(conditions: Array<string | false | null | undefined>) {
  return conditions.filter(Boolean).join(' AND ')
}

function currencyOf(rows: GoogleAdsRow[]) {
  return rows[0]?.customer?.currencyCode ?? null
}

function texts(assets: Array<{ text?: string }> | undefined) {
  return (assets ?? []).map((asset) => asset.text).filter(Boolean)
}

type Account = {
  customer_id: string
  name: string | null
  currency: string | null
  time_zone: string | null
  manager: boolean
  test_account: boolean
  status: string | null
}

function accountRow(account: Record<string, any>): Account {
  return {
    customer_id: customerLabel(String(account.id)),
    name: account.descriptiveName ?? null,
    currency: account.currencyCode ?? null,
    time_zone: account.timeZone ?? null,
    manager: Boolean(account.manager),
    test_account: Boolean(account.testAccount),
    status: account.status ?? null,
  }
}

async function listAccounts(context: BuiltinToolContext) {
  const allowed = allowedCustomers(context)
  const { resourceNames = [] } = (await googleAdsRequest(
    context,
    '/customers:listAccessibleCustomers',
    { method: 'GET' }
  )) as { resourceNames?: string[] }
  const direct = resourceNames
    .map((name) => name.replace('customers/', ''))
    .slice(0, MAX_LISTED_ACCOUNTS)

  const accounts: Array<Account | { customer_id: string; error: string }> = []
  const clients: Array<Account & { manager_id: string }> = []
  for (const customer of direct) {
    try {
      const { rows } = await searchGoogleAds(
        context,
        customer,
        'SELECT customer.id, customer.descriptive_name, customer.currency_code, customer.time_zone, customer.manager, customer.test_account, customer.status FROM customer LIMIT 1',
        1
      )
      const account = accountRow(rows[0]?.customer ?? { id: customer })
      accounts.push(account)

      if (account.manager) {
        const managed = await searchGoogleAds(
          context,
          customer,
          "SELECT customer_client.id, customer_client.descriptive_name, customer_client.currency_code, customer_client.time_zone, customer_client.manager, customer_client.test_account, customer_client.status FROM customer_client WHERE customer_client.level = 1 AND customer_client.status = 'ENABLED'",
          MAX_LISTED_CLIENTS
        )
        clients.push(
          ...managed.rows.map((row) => ({
            ...accountRow(row.customerClient ?? {}),
            manager_id: account.customer_id,
          }))
        )
      }
    } catch (error) {
      // One account that cannot be opened must not hide the others.
      if (!(error instanceof BuiltinToolError)) throw error
      accounts.push({ customer_id: customerLabel(customer), error: error.message })
    }
  }

  const usable = (account: { customer_id: string }) =>
    !allowed || allowed.includes(customerNumber(account.customer_id))
  return {
    accounts: accounts.filter(usable),
    client_accounts: clients.filter(usable),
    manager_account_id: context.settings.loginCustomerId
      ? customerLabel(context.settings.loginCustomerId)
      : null,
    limited_to: allowed?.map(customerLabel) ?? null,
  }
}

const levels = {
  account: { from: 'customer', fields: 'customer.id, customer.descriptive_name' },
  campaign: { from: 'campaign', fields: 'campaign.id, campaign.name, campaign.status' },
  ad_group: {
    from: 'ad_group',
    fields: 'ad_group.id, ad_group.name, ad_group.status, campaign.id, campaign.name',
  },
} as const

const segments = {
  date: 'segments.date',
  week: 'segments.week',
  month: 'segments.month',
  device: 'segments.device',
  network: 'segments.ad_network_type',
} as const

export const googleAdsReadTools: BuiltinTool[] = [
  builtinTool({
    name: 'list_accounts',
    description:
      'List the Google Ads accounts this MCP can use: the ones the connected Google sign-in opens directly, and the client accounts of those that are manager accounts. Campaigns live in client accounts, never in a manager account. Each account comes with its currency and time zone, which every amount and date of the other tools follows.',
    inputSchema: { type: 'object', properties: {}, additionalProperties: false },
    input: noArgumentsValidator,
    run: async (_input, context) => listAccounts(context),
  }),
  builtinTool({
    name: 'list_campaigns',
    description: `List the campaigns of an account with their status, type, bidding strategy, daily budget, and figures over a period: impressions, clicks, cost, click-through rate, average cost per click, conversions, and conversion value. Removed campaigns are left out. ${MONEY}`,
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        status: {
          type: 'string',
          enum: ['ENABLED', 'PAUSED', 'ALL'],
          default: 'ALL',
          description: 'Only campaigns with this status. ALL is enabled and paused ones.',
        },
        ...periodProperties,
        ...limitProperty('campaigns'),
      },
      required: ['customer_id'],
      additionalProperties: false,
    },
    input: listCampaignsValidator,
    run: async (
      { customer_id: customerId, status = 'ALL', limit = DEFAULT_ROWS, ...dates },
      context
    ) => {
      const customer = customerOf(context, customerId)
      const { where, period } = periodOf(dates)
      const { rows, truncated } = await searchGoogleAds(
        context,
        customer,
        `SELECT campaign.id, campaign.name, campaign.status, campaign.primary_status, campaign.advertising_channel_type, campaign.bidding_strategy_type, campaign.start_date_time, campaign.end_date_time, campaign_budget.amount_micros, campaign_budget.reference_count, customer.currency_code, ${METRICS} FROM campaign WHERE ${filters(
          [
            status === 'ALL' ? "campaign.status != 'REMOVED'" : `campaign.status = '${status}'`,
            where,
          ]
        )} ORDER BY metrics.cost_micros DESC LIMIT ${limit + 1}`,
        limit
      )
      return {
        currency: currencyOf(rows),
        period,
        campaigns: rows.map((row) => ({
          id: String(row.campaign!.id),
          name: row.campaign!.name,
          status: row.campaign!.status,
          serving: row.campaign!.primaryStatus ?? null,
          type: row.campaign!.advertisingChannelType,
          bidding_strategy: row.campaign!.biddingStrategyType,
          daily_budget: fromMicros(row.campaignBudget?.amountMicros) ?? null,
          budget_shared_by: Number(row.campaignBudget?.referenceCount ?? 1),
          start_date: row.campaign!.startDateTime?.slice(0, 10) ?? null,
          end_date: row.campaign!.endDateTime?.slice(0, 10) ?? null,
          ...figures(row.metrics),
        })),
        truncated,
      }
    },
  }),
  builtinTool({
    name: 'get_campaign',
    description: `Get one campaign in full: settings, budget, bidding, networks, the locations and languages it targets, its negative keywords, and its ad groups. Criterion IDs are the ones update_campaign_targeting removes. ${MONEY}`,
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        campaign_id: { type: 'string', description: 'Campaign ID, as returned by list_campaigns.' },
      },
      required: ['customer_id', 'campaign_id'],
      additionalProperties: false,
    },
    input: campaignValidator,
    run: async ({ customer_id: customerId, campaign_id: campaignId }, context) => {
      const customer = customerOf(context, customerId)
      const { rows } = await searchGoogleAds(
        context,
        customer,
        `SELECT campaign.id, campaign.name, campaign.status, campaign.primary_status, campaign.primary_status_reasons, campaign.advertising_channel_type, campaign.bidding_strategy_type, campaign.start_date_time, campaign.end_date_time, campaign.network_settings.target_google_search, campaign.network_settings.target_search_network, campaign.network_settings.target_content_network, campaign.target_spend.cpc_bid_ceiling_micros, campaign.maximize_conversions.target_cpa_micros, campaign.maximize_conversion_value.target_roas, campaign.contains_eu_political_advertising, campaign.optimization_score, campaign_budget.id, campaign_budget.amount_micros, campaign_budget.reference_count, customer.currency_code FROM campaign WHERE campaign.id = ${campaignId} LIMIT 1`,
        1
      )
      if (rows.length === 0) {
        throw new BuiltinToolError(
          `There is no campaign ${campaignId} in the Google Ads account ${customerLabel(customer)}`
        )
      }

      const criteria = await searchGoogleAds(
        context,
        customer,
        `SELECT campaign_criterion.criterion_id, campaign_criterion.type, campaign_criterion.negative, campaign_criterion.display_name, campaign_criterion.keyword.text, campaign_criterion.keyword.match_type FROM campaign_criterion WHERE campaign.id = ${campaignId} AND campaign_criterion.type IN ('LOCATION', 'LANGUAGE', 'KEYWORD') AND campaign_criterion.status != 'REMOVED' LIMIT ${GOOGLE_ADS_LIMITS.rows}`,
        GOOGLE_ADS_LIMITS.rows
      )
      const adGroups = await searchGoogleAds(
        context,
        customer,
        `SELECT ad_group.id, ad_group.name, ad_group.status, ad_group.type, ad_group.cpc_bid_micros FROM ad_group WHERE campaign.id = ${campaignId} AND ad_group.status != 'REMOVED' LIMIT ${GOOGLE_ADS_LIMITS.rows}`,
        GOOGLE_ADS_LIMITS.rows
      )

      const [{ campaign, campaignBudget: budget, customer: account }] = rows
      const ofType = (type: string) =>
        criteria.rows
          .map((row) => row.campaignCriterion!)
          .filter((criterion) => criterion.type === type)
      return {
        currency: account?.currencyCode ?? null,
        id: String(campaign!.id),
        name: campaign!.name,
        status: campaign!.status,
        serving: campaign!.primaryStatus ?? null,
        serving_reasons: campaign!.primaryStatusReasons ?? [],
        type: campaign!.advertisingChannelType,
        start_date: campaign!.startDateTime?.slice(0, 10) ?? null,
        end_date: campaign!.endDateTime?.slice(0, 10) ?? null,
        daily_budget: fromMicros(budget?.amountMicros) ?? null,
        budget_shared_by: Number(budget?.referenceCount ?? 1),
        bidding: {
          strategy: campaign!.biddingStrategyType,
          max_cpc: fromMicros(campaign!.targetSpend?.cpcBidCeilingMicros) || null,
          target_cpa: fromMicros(campaign!.maximizeConversions?.targetCpaMicros) || null,
          target_roas: campaign!.maximizeConversionValue?.targetRoas || null,
        },
        networks: {
          google_search: Boolean(campaign!.networkSettings?.targetGoogleSearch),
          search_partners: Boolean(campaign!.networkSettings?.targetSearchNetwork),
          display_network: Boolean(campaign!.networkSettings?.targetContentNetwork),
        },
        eu_political_ads:
          campaign!.containsEuPoliticalAdvertising === 'CONTAINS_EU_POLITICAL_ADVERTISING',
        optimization_score: campaign!.optimizationScore ?? null,
        locations: ofType('LOCATION').map((criterion) => ({
          criterion_id: String(criterion.criterionId),
          name: criterion.displayName ?? null,
          excluded: Boolean(criterion.negative),
        })),
        languages: ofType('LANGUAGE').map((criterion) => ({
          criterion_id: String(criterion.criterionId),
          name: criterion.displayName ?? null,
        })),
        negative_keywords: ofType('KEYWORD').map((criterion) => ({
          criterion_id: String(criterion.criterionId),
          text: criterion.keyword?.text ?? null,
          match_type: criterion.keyword?.matchType ?? null,
        })),
        ad_groups: adGroups.rows.map((row) => ({
          id: String(row.adGroup!.id),
          name: row.adGroup!.name,
          status: row.adGroup!.status,
          type: row.adGroup!.type,
          max_cpc: fromMicros(row.adGroup!.cpcBidMicros) || null,
        })),
      }
    },
  }),
  builtinTool({
    name: 'list_ad_groups',
    description: `List the ad groups of an account, or of one campaign, with their status, default bid, and figures over a period. Removed ad groups are left out. ${MONEY}`,
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        ...campaignFilterProperty,
        ...periodProperties,
        ...limitProperty('ad groups'),
      },
      required: ['customer_id'],
      additionalProperties: false,
    },
    input: listAdGroupsValidator,
    run: async (
      { customer_id: customerId, campaign_id: campaignId, limit = DEFAULT_ROWS, ...dates },
      context
    ) => {
      const customer = customerOf(context, customerId)
      const { where, period } = periodOf(dates)
      const { rows, truncated } = await searchGoogleAds(
        context,
        customer,
        `SELECT ad_group.id, ad_group.name, ad_group.status, ad_group.type, ad_group.cpc_bid_micros, campaign.id, campaign.name, customer.currency_code, ${METRICS} FROM ad_group WHERE ${filters(
          ["ad_group.status != 'REMOVED'", campaignId && `campaign.id = ${campaignId}`, where]
        )} ORDER BY metrics.cost_micros DESC LIMIT ${limit + 1}`,
        limit
      )
      return {
        currency: currencyOf(rows),
        period,
        ad_groups: rows.map((row) => ({
          id: String(row.adGroup!.id),
          name: row.adGroup!.name,
          status: row.adGroup!.status,
          type: row.adGroup!.type,
          max_cpc: fromMicros(row.adGroup!.cpcBidMicros) || null,
          campaign_id: String(row.campaign!.id),
          campaign: row.campaign!.name,
          ...figures(row.metrics),
        })),
        truncated,
      }
    },
  }),
  builtinTool({
    name: 'list_ads',
    description: `List the ads of an account, a campaign, or an ad group with their texts, landing page, status, Google's review of them (approval, policy topics, ad strength), and figures over a period. Removed ads are left out. ${MONEY}`,
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        ...campaignFilterProperty,
        ...adGroupFilterProperty,
        ...periodProperties,
        ...limitProperty('ads'),
      },
      required: ['customer_id'],
      additionalProperties: false,
    },
    input: listInCampaignValidator,
    run: async (
      {
        customer_id: customerId,
        campaign_id: campaignId,
        ad_group_id: adGroupId,
        limit = DEFAULT_ROWS,
        ...dates
      },
      context
    ) => {
      const customer = customerOf(context, customerId)
      const { where, period } = periodOf(dates)
      const { rows, truncated } = await searchGoogleAds(
        context,
        customer,
        `SELECT ad_group_ad.ad.id, ad_group_ad.ad.type, ad_group_ad.ad.final_urls, ad_group_ad.ad.responsive_search_ad.headlines, ad_group_ad.ad.responsive_search_ad.descriptions, ad_group_ad.ad.responsive_search_ad.path1, ad_group_ad.ad.responsive_search_ad.path2, ad_group_ad.ad.responsive_display_ad.headlines, ad_group_ad.ad.responsive_display_ad.long_headline, ad_group_ad.ad.responsive_display_ad.descriptions, ad_group_ad.ad.responsive_display_ad.business_name, ad_group_ad.status, ad_group_ad.ad_strength, ad_group_ad.policy_summary.approval_status, ad_group_ad.policy_summary.review_status, ad_group_ad.policy_summary.policy_topic_entries, ad_group.id, ad_group.name, campaign.id, campaign.name, customer.currency_code, ${METRICS} FROM ad_group_ad WHERE ${filters(
          [
            "ad_group_ad.status != 'REMOVED'",
            campaignId && `campaign.id = ${campaignId}`,
            adGroupId && `ad_group.id = ${adGroupId}`,
            where,
          ]
        )} ORDER BY metrics.impressions DESC LIMIT ${limit + 1}`,
        limit
      )
      return {
        currency: currencyOf(rows),
        period,
        ads: rows.map((row) => {
          const { ad = {}, policySummary: policy = {}, ...adGroupAd } = row.adGroupAd!
          const search = ad.responsiveSearchAd
          const display = ad.responsiveDisplayAd
          return {
            id: String(ad.id),
            type: ad.type,
            status: adGroupAd.status,
            final_url: ad.finalUrls?.[0] ?? null,
            headlines: texts((search ?? display)?.headlines),
            ...(display ? { long_headline: display.longHeadline?.text ?? null } : {}),
            descriptions: texts((search ?? display)?.descriptions),
            ...(search ? { path: [search.path1, search.path2].filter(Boolean).join('/') } : {}),
            ...(display ? { business_name: display.businessName ?? null } : {}),
            approval: policy.approvalStatus ?? null,
            review: policy.reviewStatus ?? null,
            policy_topics: (policy.policyTopicEntries ?? []).map(
              (entry: { topic?: string; type?: string }) => `${entry.topic} (${entry.type})`
            ),
            ad_strength: adGroupAd.adStrength ?? null,
            ad_group_id: String(row.adGroup!.id),
            ad_group: row.adGroup!.name,
            campaign_id: String(row.campaign!.id),
            campaign: row.campaign!.name,
            ...figures(row.metrics),
          }
        }),
        truncated,
      }
    },
  }),
  builtinTool({
    name: 'list_keywords',
    description: `List the keywords of an account, a campaign, or an ad group with their match type, status, bid, quality score, and figures over a period. Negative and removed keywords are left out: get_campaign lists the negative keywords of a campaign. ${MONEY}`,
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        ...campaignFilterProperty,
        ...adGroupFilterProperty,
        ...periodProperties,
        ...limitProperty('keywords'),
      },
      required: ['customer_id'],
      additionalProperties: false,
    },
    input: listInCampaignValidator,
    run: async (
      {
        customer_id: customerId,
        campaign_id: campaignId,
        ad_group_id: adGroupId,
        limit = DEFAULT_ROWS,
        ...dates
      },
      context
    ) => {
      const customer = customerOf(context, customerId)
      const { where, period } = periodOf(dates)
      const { rows, truncated } = await searchGoogleAds(
        context,
        customer,
        `SELECT ad_group_criterion.criterion_id, ad_group_criterion.keyword.text, ad_group_criterion.keyword.match_type, ad_group_criterion.status, ad_group_criterion.cpc_bid_micros, ad_group_criterion.effective_cpc_bid_micros, ad_group_criterion.quality_info.quality_score, ad_group_criterion.approval_status, ad_group.id, ad_group.name, campaign.id, campaign.name, customer.currency_code, ${METRICS} FROM keyword_view WHERE ${filters(
          [
            "ad_group_criterion.status != 'REMOVED'",
            campaignId && `campaign.id = ${campaignId}`,
            adGroupId && `ad_group.id = ${adGroupId}`,
            where,
          ]
        )} ORDER BY metrics.cost_micros DESC LIMIT ${limit + 1}`,
        limit
      )
      return {
        currency: currencyOf(rows),
        period,
        keywords: rows.map((row) => {
          const criterion = row.adGroupCriterion!
          return {
            criterion_id: String(criterion.criterionId),
            text: criterion.keyword?.text ?? null,
            match_type: criterion.keyword?.matchType ?? null,
            status: criterion.status,
            max_cpc:
              fromMicros(criterion.cpcBidMicros) ||
              fromMicros(criterion.effectiveCpcBidMicros) ||
              null,
            quality_score: criterion.qualityInfo?.qualityScore ?? null,
            approval: criterion.approvalStatus ?? null,
            ad_group_id: String(row.adGroup!.id),
            ad_group: row.adGroup!.name,
            campaign_id: String(row.campaign!.id),
            campaign: row.campaign!.name,
            ...figures(row.metrics),
          }
        }),
        truncated,
      }
    },
  }),
  builtinTool({
    name: 'list_search_terms',
    description: `List what people actually searched for before seeing the ads of an account, a campaign, or an ad group, with the figures of each search term over a period. Use it to find keywords to add and negative keywords to exclude. ${MONEY}`,
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        ...campaignFilterProperty,
        ...adGroupFilterProperty,
        ...periodProperties,
        ...limitProperty('search terms'),
      },
      required: ['customer_id'],
      additionalProperties: false,
    },
    input: listInCampaignValidator,
    run: async (
      {
        customer_id: customerId,
        campaign_id: campaignId,
        ad_group_id: adGroupId,
        limit = DEFAULT_ROWS,
        ...dates
      },
      context
    ) => {
      const customer = customerOf(context, customerId)
      const { where, period } = periodOf(dates)
      const { rows, truncated } = await searchGoogleAds(
        context,
        customer,
        `SELECT search_term_view.search_term, search_term_view.status, ad_group.id, ad_group.name, campaign.id, campaign.name, customer.currency_code, ${METRICS} FROM search_term_view WHERE ${filters(
          [
            campaignId && `campaign.id = ${campaignId}`,
            adGroupId && `ad_group.id = ${adGroupId}`,
            where,
          ]
        )} ORDER BY metrics.impressions DESC LIMIT ${limit + 1}`,
        limit
      )
      return {
        currency: currencyOf(rows),
        period,
        search_terms: rows.map((row) => ({
          search_term: row.searchTermView!.searchTerm,
          // Whether the term is already a keyword, or already excluded.
          status: row.searchTermView!.status,
          ad_group_id: String(row.adGroup!.id),
          ad_group: row.adGroup!.name,
          campaign_id: String(row.campaign!.id),
          campaign: row.campaign!.name,
          ...figures(row.metrics),
        })),
        truncated,
      }
    },
  }),
  builtinTool({
    name: 'get_performance',
    description: `Get the figures of an account, of its campaigns, or of its ad groups over a period, optionally broken down by day, week, month, device, or network. Rows without any impression are left out of a breakdown. ${MONEY}`,
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        level: {
          type: 'string',
          enum: [...GOOGLE_ADS_PERFORMANCE_LEVELS],
          default: 'campaign',
          description: 'What each row is about.',
        },
        segment: {
          type: 'string',
          enum: [...GOOGLE_ADS_SEGMENTS],
          description: 'Break each row down by this. Left out, a row covers the whole period.',
        },
        campaign_id: {
          type: 'string',
          description: 'Only this campaign. Not with the account level.',
        },
        ...periodProperties,
        ...limitProperty('rows'),
      },
      required: ['customer_id'],
      additionalProperties: false,
    },
    input: getPerformanceValidator,
    run: async (
      {
        customer_id: customerId,
        level = 'campaign',
        segment,
        campaign_id: campaignId,
        limit = DEFAULT_ROWS,
        ...dates
      },
      context
    ) => {
      const customer = customerOf(context, customerId)
      if (campaignId && level === 'account') {
        throw new BuiltinToolError('campaign_id narrows the campaign and ad_group levels only')
      }
      const { where, period } = periodOf(dates)
      const { from, fields } = levels[level]
      const breakdown = segment ? segments[segment] : null
      // Days, weeks, and months read in order. The other breakdowns by what costs most.
      const isTimeline = segment === 'date' || segment === 'week' || segment === 'month'
      const { rows, truncated } = await searchGoogleAds(
        context,
        customer,
        `SELECT ${fields}, ${breakdown ? `${breakdown}, ` : ''}customer.currency_code, ${METRICS} FROM ${from} WHERE ${filters(
          [campaignId && `campaign.id = ${campaignId}`, where]
        )} ORDER BY ${isTimeline ? breakdown : 'metrics.cost_micros DESC'} LIMIT ${limit + 1}`,
        limit
      )
      return {
        currency: currencyOf(rows),
        period,
        level,
        rows: rows.map((row) => ({
          ...(segment
            ? {
                [segment]:
                  row.segments?.[segment === 'network' ? 'adNetworkType' : segment] ?? null,
              }
            : {}),
          ...(level === 'account' ? { account: row.customer?.descriptiveName ?? null } : {}),
          ...(row.campaign
            ? { campaign_id: String(row.campaign.id), campaign: row.campaign.name }
            : {}),
          ...(row.adGroup
            ? { ad_group_id: String(row.adGroup.id), ad_group: row.adGroup.name }
            : {}),
          ...figures(row.metrics),
        })),
        truncated,
      }
    },
  }),
  builtinTool({
    name: 'list_assets',
    description:
      'List the assets of an account: images with their size in pixels and a link to view them, and the texts, sitelinks, and callouts ads can show. Image asset IDs are what create_responsive_display_ad and add_campaign_images take.',
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        type: {
          type: 'string',
          enum: [...GOOGLE_ADS_ASSET_TYPES],
          default: 'IMAGE',
          description: 'Only assets of this type. ALL is every type listed here.',
        },
        ...limitProperty('assets'),
      },
      required: ['customer_id'],
      additionalProperties: false,
    },
    input: listAssetsValidator,
    run: async ({ customer_id: customerId, type = 'IMAGE', limit = DEFAULT_ROWS }, context) => {
      const customer = customerOf(context, customerId)
      const types =
        type === 'ALL' ? GOOGLE_ADS_ASSET_TYPES.filter((name) => name !== 'ALL') : [type]
      const { rows, truncated } = await searchGoogleAds(
        context,
        customer,
        `SELECT asset.id, asset.name, asset.type, asset.final_urls, asset.image_asset.full_size.url, asset.image_asset.full_size.width_pixels, asset.image_asset.full_size.height_pixels, asset.image_asset.file_size, asset.text_asset.text, asset.sitelink_asset.link_text, asset.callout_asset.callout_text FROM asset WHERE asset.type IN (${types.map((name) => `'${name}'`).join(', ')}) LIMIT ${limit + 1}`,
        limit
      )
      return {
        assets: rows.map((row) => {
          const asset = row.asset!
          const image = asset.imageAsset
          return {
            id: String(asset.id),
            type: asset.type,
            name: asset.name ?? null,
            ...(image
              ? {
                  width: Number(image.fullSize?.widthPixels ?? 0),
                  height: Number(image.fullSize?.heightPixels ?? 0),
                  bytes: Number(image.fileSize ?? 0),
                  url: image.fullSize?.url ?? null,
                }
              : {
                  text:
                    asset.textAsset?.text ??
                    asset.sitelinkAsset?.linkText ??
                    asset.calloutAsset?.calloutText ??
                    null,
                }),
            ...(asset.finalUrls?.length ? { final_url: asset.finalUrls[0] } : {}),
          }
        }),
        truncated,
      }
    },
  }),
  builtinTool({
    name: 'list_changes',
    description:
      'List what was changed in an account lately, newest first: when, by whom, from which tool (the Google Ads website, the API, a script, a recommendation), on which campaign or ad group, and which fields. Google keeps 30 days of changes, and lists a change a few minutes after it was made.',
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        days: {
          type: 'integer',
          minimum: 1,
          maximum: GOOGLE_ADS_LIMITS.changeDays,
          default: 7,
          description: 'How many days back to look, today included.',
        },
        ...limitProperty('changes'),
      },
      required: ['customer_id'],
      additionalProperties: false,
    },
    input: listChangesValidator,
    run: async ({ customer_id: customerId, days = 7, limit = DEFAULT_ROWS }, context) => {
      const customer = customerOf(context, customerId)
      // Google refuses a start more than 30 days old. A day ahead covers the
      // accounts whose time zone is already tomorrow.
      const now = DateTime.utc()
      const from = now.minus({ days: days - 1 }).toFormat('yyyy-MM-dd')
      const to = now.plus({ days: 1 }).toFormat('yyyy-MM-dd')
      const { rows, truncated } = await searchGoogleAds(
        context,
        customer,
        `SELECT change_event.change_date_time, change_event.change_resource_type, change_event.resource_change_operation, change_event.changed_fields, change_event.client_type, change_event.user_email, change_event.campaign, change_event.ad_group, change_event.old_resource, change_event.new_resource FROM change_event WHERE change_event.change_date_time >= '${from}' AND change_event.change_date_time <= '${to}' ORDER BY change_event.change_date_time DESC LIMIT ${limit + 1}`,
        limit
      )
      return {
        changes: rows.map((row) => {
          const change = row.changeEvent!
          return {
            at: change.changeDateTime,
            resource: change.changeResourceType,
            operation: change.resourceChangeOperation,
            fields: change.changedFields ? String(change.changedFields).split(',') : [],
            by: change.userEmail ?? null,
            through: change.clientType ?? null,
            campaign_id: change.campaign?.split('/').pop() ?? null,
            ad_group_id: change.adGroup?.split('/').pop() ?? null,
            // As Google gives them: amounts ending in Micros are millionths of the currency.
            before: change.oldResource ?? null,
            after: change.newResource ?? null,
          }
        }),
        truncated,
      }
    },
  }),
  builtinTool({
    name: 'search_locations',
    description:
      'Find the locations Google Ads can target by name: countries, regions, cities, postal codes. Returns the location IDs that create_campaign and update_campaign_targeting take, with how many people each reaches.',
    inputSchema: {
      type: 'object',
      properties: {
        names: {
          type: 'array',
          items: { type: 'string', maxLength: 80 },
          minItems: 1,
          maxItems: GOOGLE_ADS_LIMITS.locationNames,
          description: 'Place names to look for, such as ["France", "Lyon"].',
        },
        country_code: {
          type: 'string',
          description: 'Only places in this country, as a two-letter code such as FR.',
        },
        locale: {
          type: 'string',
          default: 'en',
          description: 'Language the names are written in, as a two-letter code such as fr.',
        },
      },
      required: ['names'],
      additionalProperties: false,
    },
    input: searchLocationsValidator,
    run: async ({ names, country_code: countryCode, locale = 'en' }, context) => {
      const { geoTargetConstantSuggestions: suggestions = [] } = (await googleAdsRequest(
        context,
        '/geoTargetConstants:suggest',
        {
          body: {
            locale: locale.toLowerCase(),
            ...(countryCode ? { countryCode: countryCode.toUpperCase() } : {}),
            locationNames: { names },
          },
        }
      )) as { geoTargetConstantSuggestions?: Array<Record<string, any>> }
      return {
        locations: suggestions.map(({ geoTargetConstant: place = {}, searchTerm, reach }) => ({
          id: String(place.id),
          name: place.canonicalName ?? place.name ?? null,
          type: place.targetType ?? null,
          country_code: place.countryCode ?? null,
          status: place.status ?? null,
          reach: reach === undefined ? null : Number(reach),
          searched: searchTerm ?? null,
        })),
      }
    },
  }),
  builtinTool({
    name: 'generate_keyword_ideas',
    description: `Get keyword ideas from Google's Keyword Planner for seed keywords, a web page, or both: each idea with its average monthly searches, competition, and the range of bids that reach the top of the page. Google only answers this for Cloud projects with Basic or Standard access. ${MONEY}`,
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        keywords: {
          type: 'array',
          items: { type: 'string', maxLength: GOOGLE_ADS_LIMITS.keywordLength },
          minItems: 1,
          maxItems: GOOGLE_ADS_LIMITS.seedKeywords,
          description: 'Words or phrases to start from.',
        },
        url: { type: 'string', description: 'A web page to draw ideas from.' },
        location_ids: {
          type: 'array',
          items: { type: 'string' },
          minItems: 1,
          maxItems: GOOGLE_ADS_LIMITS.locations,
          description: 'Only searches from these locations, by the IDs search_locations returns.',
        },
        language: {
          type: 'string',
          default: 'en',
          description: 'Language of the searches, as a code such as en, fr, or pt_BR.',
        },
        ...limitProperty('ideas'),
      },
      required: ['customer_id'],
      additionalProperties: false,
    },
    input: keywordIdeasValidator,
    run: async (
      {
        customer_id: customerId,
        keywords,
        url,
        location_ids: locationIds = [],
        language = 'en',
        limit = DEFAULT_ROWS,
      },
      context
    ) => {
      const customer = customerOf(context, customerId)
      const [spoken] = await languagesByCode(context, customer, [languageCode(language)])
      const seed =
        keywords && url
          ? { keywordAndUrlSeed: { keywords, url } }
          : keywords
            ? { keywordSeed: { keywords } }
            : { urlSeed: { url } }
      const { results = [] } = (await googleAdsRequest(
        context,
        `/customers/${customer}:generateKeywordIdeas`,
        {
          body: {
            language: `languageConstants/${spoken.id}`,
            geoTargetConstants: locationIds.map((id) => `geoTargetConstants/${id}`),
            keywordPlanNetwork: 'GOOGLE_SEARCH',
            pageSize: limit,
            ...seed,
          },
        }
      )) as { results?: Array<Record<string, any>> }
      return {
        language: spoken.name,
        ideas: results.slice(0, limit).map(({ text, keywordIdeaMetrics: metrics = {} }) => ({
          text,
          average_monthly_searches: Number(metrics.avgMonthlySearches ?? 0),
          competition: metrics.competition ?? null,
          competition_index:
            metrics.competitionIndex === undefined ? null : Number(metrics.competitionIndex),
          low_top_of_page_bid: fromMicros(metrics.lowTopOfPageBidMicros) ?? null,
          high_top_of_page_bid: fromMicros(metrics.highTopOfPageBidMicros) ?? null,
        })),
      }
    },
  }),
  builtinTool({
    name: 'run_query',
    description:
      'Run a read-only Google Ads Query Language (GAQL) query for what the other tools do not cover, such as conversion actions, recommendations, audiences, or figures by hour. Rows come back as Google returns them: field names in camelCase, identifiers and counts as text, and amounts ending in Micros in millionths of the account currency (2500000 is 2.50). A query cannot change anything.',
    inputSchema: {
      type: 'object',
      properties: {
        ...customerIdProperty,
        query: {
          type: 'string',
          maxLength: GOOGLE_ADS_LIMITS.queryLength,
          description:
            "A GAQL query, such as: SELECT campaign.name, metrics.clicks FROM campaign WHERE segments.date DURING LAST_7_DAYS AND campaign.status = 'ENABLED' ORDER BY metrics.clicks DESC",
        },
        ...limitProperty('rows'),
      },
      required: ['customer_id', 'query'],
      additionalProperties: false,
    },
    input: runQueryValidator,
    run: async ({ customer_id: customerId, query, limit = DEFAULT_ROWS }, context) => {
      const { rows, truncated } = await searchGoogleAds(
        context,
        customerOf(context, customerId),
        query,
        limit
      )
      return { rows, truncated }
    },
  }),
]
