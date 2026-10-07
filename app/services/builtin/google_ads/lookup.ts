import { BuiltinToolError, type BuiltinToolContext } from '#services/builtin/definition'
import { searchGoogleAds, type GoogleAdsRow } from '#services/builtin/google_ads/api'
import { customerLabel, fromMicros } from '#services/builtin/google_ads/format'

/**
 * What Google Ads says about the things a call names, read before the call
 * changes them. The tools build their requests from it, and the people who
 * approve a call read it: names and current values never come from the agent.
 */

export type AccountFacts = {
  customer: string
  /** As a person recognizes it: `Acme Shoes (123-456-7890)`. */
  label: string
  currency: string
  timeZone: string
}

export type CampaignFacts = {
  resourceName: string
  id: string
  name: string
  status: string
  channel: string
  biddingStrategy: string
  /** A day as `YYYY-MM-DD`, in the account's time zone. */
  startDate?: string
  endDate?: string
  searchPartners: boolean
  displayNetwork: boolean
  maxCpc?: number
  targetCpa?: number
  targetRoas?: number
  budget: {
    resourceName: string
    /** A day's budget in the account's currency. */
    amount: number
    /** How many campaigns spend from it. */
    campaigns: number
  }
  account: AccountFacts
}

export type AdGroupFacts = {
  resourceName: string
  id: string
  name: string
  status: string
  maxCpc?: number
  campaign: Pick<CampaignFacts, 'id' | 'name' | 'status' | 'channel'>
  account: AccountFacts
}

export type KeywordFacts = {
  resourceName: string
  text: string
  matchType: string
  negative: boolean
  status: string
  maxCpc?: number
  adGroup: AdGroupFacts
}

const ACCOUNT_FIELDS =
  'customer.id, customer.descriptive_name, customer.currency_code, customer.time_zone'
const CAMPAIGN_FIELDS =
  'campaign.resource_name, campaign.id, campaign.name, campaign.status, campaign.advertising_channel_type'
const AD_GROUP_FIELDS = `ad_group.resource_name, ad_group.id, ad_group.name, ad_group.status, ad_group.cpc_bid_micros, ${CAMPAIGN_FIELDS}, ${ACCOUNT_FIELDS}`

function accountOf(customer: string, row: GoogleAdsRow): AccountFacts {
  const name = row.customer?.descriptiveName
  return {
    customer,
    label: name ? `${name} (${customerLabel(customer)})` : customerLabel(customer),
    currency: row.customer?.currencyCode ?? 'USD',
    timeZone: row.customer?.timeZone ?? 'UTC',
  }
}

/** The day of a `yyyy-MM-dd HH:mm:ss` timestamp, which is how Google dates a campaign. */
function dayOf(timestamp: string | undefined) {
  return timestamp?.slice(0, 10) || undefined
}

function adGroupOf(customer: string, row: GoogleAdsRow): AdGroupFacts {
  return {
    resourceName: row.adGroup!.resourceName,
    id: String(row.adGroup!.id),
    name: row.adGroup!.name,
    status: row.adGroup!.status,
    maxCpc: fromMicros(row.adGroup!.cpcBidMicros) || undefined,
    campaign: {
      id: String(row.campaign!.id),
      name: row.campaign!.name,
      status: row.campaign!.status,
      channel: row.campaign!.advertisingChannelType,
    },
    account: accountOf(customer, row),
  }
}

async function only(
  context: BuiltinToolContext,
  customer: string,
  query: string,
  missing: string
): Promise<GoogleAdsRow> {
  const { rows } = await searchGoogleAds(context, customer, query, 1)
  if (rows.length === 0) {
    throw new BuiltinToolError(`${missing} in the Google Ads account ${customerLabel(customer)}`)
  }
  return rows[0]
}

export async function accountFacts(context: BuiltinToolContext, customer: string) {
  const row = await only(
    context,
    customer,
    `SELECT ${ACCOUNT_FIELDS} FROM customer LIMIT 1`,
    'Nothing was found'
  )
  return accountOf(customer, row)
}

export async function campaignFacts(
  context: BuiltinToolContext,
  customer: string,
  campaignId: string
): Promise<CampaignFacts> {
  const row = await only(
    context,
    customer,
    `SELECT ${CAMPAIGN_FIELDS}, campaign.bidding_strategy_type, campaign.start_date_time, campaign.end_date_time, campaign.network_settings.target_search_network, campaign.network_settings.target_content_network, campaign.target_spend.cpc_bid_ceiling_micros, campaign.maximize_conversions.target_cpa_micros, campaign.maximize_conversion_value.target_roas, campaign.campaign_budget, campaign_budget.amount_micros, campaign_budget.reference_count, ${ACCOUNT_FIELDS} FROM campaign WHERE campaign.id = ${campaignId} LIMIT 1`,
    `There is no campaign ${campaignId}`
  )
  const campaign = row.campaign!
  return {
    resourceName: campaign.resourceName,
    id: String(campaign.id),
    name: campaign.name,
    status: campaign.status,
    channel: campaign.advertisingChannelType,
    biddingStrategy: campaign.biddingStrategyType,
    startDate: dayOf(campaign.startDateTime),
    endDate: dayOf(campaign.endDateTime),
    searchPartners: Boolean(campaign.networkSettings?.targetSearchNetwork),
    displayNetwork: Boolean(campaign.networkSettings?.targetContentNetwork),
    // Google answers 0 for a limit or a target that is not set.
    maxCpc: fromMicros(campaign.targetSpend?.cpcBidCeilingMicros) || undefined,
    targetCpa: fromMicros(campaign.maximizeConversions?.targetCpaMicros) || undefined,
    targetRoas: campaign.maximizeConversionValue?.targetRoas || undefined,
    budget: {
      resourceName: campaign.campaignBudget,
      amount: fromMicros(row.campaignBudget?.amountMicros) ?? 0,
      campaigns: Number(row.campaignBudget?.referenceCount ?? 1),
    },
    account: accountOf(customer, row),
  }
}

export async function adGroupFacts(
  context: BuiltinToolContext,
  customer: string,
  adGroupId: string
): Promise<AdGroupFacts> {
  const row = await only(
    context,
    customer,
    `SELECT ${AD_GROUP_FIELDS} FROM ad_group WHERE ad_group.id = ${adGroupId} LIMIT 1`,
    `There is no ad group ${adGroupId}`
  )
  return adGroupOf(customer, row)
}

export async function keywordFacts(
  context: BuiltinToolContext,
  customer: string,
  adGroupId: string,
  criterionId: string
): Promise<KeywordFacts> {
  const row = await only(
    context,
    customer,
    `SELECT ad_group_criterion.resource_name, ad_group_criterion.keyword.text, ad_group_criterion.keyword.match_type, ad_group_criterion.negative, ad_group_criterion.status, ad_group_criterion.cpc_bid_micros, ${AD_GROUP_FIELDS} FROM ad_group_criterion WHERE ad_group.id = ${adGroupId} AND ad_group_criterion.criterion_id = ${criterionId} AND ad_group_criterion.type = 'KEYWORD' LIMIT 1`,
    `There is no keyword ${criterionId} in the ad group ${adGroupId}`
  )
  const criterion = row.adGroupCriterion!
  return {
    resourceName: criterion.resourceName,
    text: criterion.keyword?.text ?? '',
    matchType: criterion.keyword?.matchType ?? '',
    negative: Boolean(criterion.negative),
    status: criterion.status,
    maxCpc: fromMicros(criterion.cpcBidMicros) || undefined,
    adGroup: adGroupOf(customer, row),
  }
}

export type AdFacts = {
  resourceName: string
  status: string
  type: string
  /** What the ad says first, to tell it from the others. */
  headline?: string
  adGroup: AdGroupFacts
}

export async function adFacts(
  context: BuiltinToolContext,
  customer: string,
  adGroupId: string,
  adId: string
): Promise<AdFacts> {
  const row = await only(
    context,
    customer,
    `SELECT ad_group_ad.resource_name, ad_group_ad.status, ad_group_ad.ad.type, ad_group_ad.ad.responsive_search_ad.headlines, ad_group_ad.ad.responsive_display_ad.headlines, ${AD_GROUP_FIELDS} FROM ad_group_ad WHERE ad_group.id = ${adGroupId} AND ad_group_ad.ad.id = ${adId} LIMIT 1`,
    `There is no ad ${adId} in the ad group ${adGroupId}`
  )
  const ad = row.adGroupAd!.ad ?? {}
  return {
    resourceName: row.adGroupAd!.resourceName,
    status: row.adGroupAd!.status,
    type: ad.type,
    headline: (ad.responsiveSearchAd ?? ad.responsiveDisplayAd)?.headlines?.[0]?.text,
    adGroup: adGroupOf(customer, row),
  }
}

export type LanguageFacts = { id: string; code: string; name: string }

/**
 * The languages Google Ads knows by these codes, in the order given. Throws
 * for a code it does not know, naming it.
 */
export async function languagesByCode(
  context: BuiltinToolContext,
  customer: string,
  codes: readonly string[]
): Promise<LanguageFacts[]> {
  if (codes.length === 0) return []

  const { rows } = await searchGoogleAds(
    context,
    customer,
    `SELECT language_constant.id, language_constant.code, language_constant.name FROM language_constant WHERE language_constant.code IN (${codes.map((code) => `'${code}'`).join(', ')})`,
    codes.length
  )
  const known = new Map(
    rows.map((row) => [
      String(row.languageConstant!.code).toLowerCase(),
      {
        id: String(row.languageConstant!.id),
        code: row.languageConstant!.code,
        name: row.languageConstant!.name,
      },
    ])
  )
  return codes.map((code) => {
    const language = known.get(code.toLowerCase())
    if (!language) {
      throw new BuiltinToolError(
        `Google Ads has no language with the code "${code}". Codes look like en, fr, or pt_BR.`
      )
    }
    return language
  })
}

export type LocationFacts = { id: string; name: string }

/**
 * The places behind these location IDs, in the order given. Throws for an ID
 * Google Ads does not know, so nobody approves a place they cannot read.
 */
export async function locationsById(
  context: BuiltinToolContext,
  customer: string,
  ids: readonly string[]
): Promise<LocationFacts[]> {
  if (ids.length === 0) return []

  const { rows } = await searchGoogleAds(
    context,
    customer,
    `SELECT geo_target_constant.id, geo_target_constant.canonical_name, geo_target_constant.name FROM geo_target_constant WHERE geo_target_constant.id IN (${ids.join(', ')})`,
    ids.length
  )
  const known = new Map(
    rows.map((row) => [
      String(row.geoTargetConstant!.id),
      String(row.geoTargetConstant!.canonicalName ?? row.geoTargetConstant!.name),
    ])
  )
  return ids.map((id) => {
    const name = known.get(id)
    if (!name) {
      throw new BuiltinToolError(
        `Google Ads has no location with the ID ${id}. Find location IDs with search_locations.`
      )
    }
    return { id, name }
  })
}

export type ImageAssetFacts = {
  id: string
  resourceName: string
  name: string
  width: number
  height: number
}

/** The image assets behind these IDs, in the order given. Throws for one that is not an image of the account. */
export async function imageAssetsById(
  context: BuiltinToolContext,
  customer: string,
  ids: readonly string[]
): Promise<ImageAssetFacts[]> {
  if (ids.length === 0) return []

  const unique = [...new Set(ids)]
  const { rows } = await searchGoogleAds(
    context,
    customer,
    `SELECT asset.resource_name, asset.id, asset.name, asset.type, asset.image_asset.full_size.width_pixels, asset.image_asset.full_size.height_pixels FROM asset WHERE asset.id IN (${unique.join(', ')}) AND asset.type = 'IMAGE'`,
    unique.length
  )
  const known = new Map(
    rows.map((row) => [
      String(row.asset!.id),
      {
        id: String(row.asset!.id),
        resourceName: row.asset!.resourceName,
        name: row.asset!.name ?? '',
        width: Number(row.asset!.imageAsset?.fullSize?.widthPixels ?? 0),
        height: Number(row.asset!.imageAsset?.fullSize?.heightPixels ?? 0),
      },
    ])
  )
  return ids.map((id) => {
    const asset = known.get(id)
    if (!asset) {
      throw new BuiltinToolError(
        `There is no image asset ${id} in the Google Ads account ${customerLabel(customer)}. List them with list_assets, or add one with create_image_asset.`
      )
    }
    return asset
  })
}
