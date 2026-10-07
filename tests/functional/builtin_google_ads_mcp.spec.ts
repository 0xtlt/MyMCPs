import { test } from '@japa/runner'
import type { ApiClient } from '@japa/api-client'
import ApprovalRequest from '#models/approval_request'
import Mcp from '#models/mcp'
import ApprovalService from '#services/approvals/approval_service'
import { saveBuiltinUpload } from '#services/builtin/upload_store'
import McpEnvironmentStore from '#services/mcp_environment_store'
import McpSecretStore from '#services/mcp_secret_store'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAccessToken, createAdmin } from '#tests/helpers/factories'
import { gatewayRpc, resultText } from '#tests/helpers/gateway'
import {
  createGoogleAdsMcp,
  GOOGLE_ADS_CUSTOMER,
  GOOGLE_ADS_SCOPE,
  googleAdsFailure,
  googleAdsFixtures,
  googleJson,
  mockGoogleAds,
} from '#tests/helpers/google_ads'

const googleAdsForm = {
  'name': 'Google Ads',
  'transport': 'builtin',
  'builtinKey': 'google-ads',
  'authType': 'auto',
  'oauthClientId': '1234567890-abc.apps.googleusercontent.com',
  'oauthClientSecret': 'google-client-secret',
  'builtinSettings[loginCustomerId]': '987-654-3210',
  'builtinSettings[customerIds]': '123-456-7890, 2345678901',
  'enabled': 'on',
}

/** Call a Google Ads tool through the gateway and read its JSON answer. */
async function callTool(
  client: ApiClient,
  plaintext: string,
  tool: string,
  args: Record<string, unknown>
) {
  const response = await gatewayRpc(client, plaintext, 'tools/call', {
    name: `google-ads__${tool}`,
    arguments: args,
  })
  const text = resultText(response)
  return { isError: response.result?.isError === true, text, data: parsed(text) }
}

function parsed(text: string) {
  try {
    return JSON.parse(text)
  } catch {
    return null
  }
}

/** A 1×1 PNG stretched in its header to the size a test needs: tools only read the header. */
function png(width: number, height: number) {
  const bytes = Buffer.alloc(33)
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]).copy(bytes)
  bytes.writeUInt32BE(13, 8)
  bytes.write('IHDR', 12, 'latin1')
  bytes.writeUInt32BE(width, 16)
  bytes.writeUInt32BE(height, 20)
  return bytes
}

async function* chunks(bytes: Buffer) {
  yield bytes
}

test.group('Built-in Google Ads MCP: setup', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('creates the MCP with its accounts and waits for authorization', async ({
    client,
    assert,
  }) => {
    const google = mockGoogleAds()
    try {
      const admin = await createAdmin()
      const response = await client
        .post('/mcps')
        .loginAs(admin)
        .withCsrfToken()
        .redirects(0)
        .form(googleAdsForm)

      response.assertStatus(302)
      response.assertFlashMessage('success', 'MCP created')
      const mcp = await Mcp.findByOrFail('slug', 'google-ads')
      assert.equal(mcp.builtinKey, 'google-ads')
      assert.equal(McpSecretStore.decrypt(mcp.oauthClientSecret), 'google-client-secret')
      // Stored without their dashes, and encrypted like the rest of the dialog.
      assert.deepEqual(McpEnvironmentStore.decrypt(mcp.builtinSettings), {
        loginCustomerId: '9876543210',
        customerIds: '1234567890 2345678901',
      })
      assert.notInclude(mcp.builtinSettings!, '9876543210')
      assert.equal(mcp.status, 'draft')
      assert.isTrue(Boolean(mcp.oauthRequired))
      assert.lengthOf(google.requests, 0)

      const page = await client.get('/mcps').loginAs(admin).withInertia()
      page.assertInertiaPropsContains({
        mcps: [
          {
            id: mcp.id,
            builtinKey: 'google-ads',
            builtinSettings: {
              loginCustomerId: '9876543210',
              customerIds: '1234567890 2345678901',
            },
            // One scope reads and writes, so there is nothing to grant again for write access.
            builtinWriteGranted: true,
          },
        ],
      })
    } finally {
      google.restore()
    }
  })

  test('reports every wrong field of the dialog at once', async ({ client, assert }) => {
    const admin = await createAdmin()
    const response = await client
      .post('/mcps')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({
        ...googleAdsForm,
        'oauthClientId': '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-field',
        'builtinSettings[loginCustomerId]': '12345',
        'builtinSettings[customerIds]': '123-456-7890, acme',
      })

    assert.deepEqual(response.flashMessage('inputErrorsBag'), {
      'oauthClientId': ['The Google Client ID ends in .apps.googleusercontent.com'],
      'builtinSettings.loginCustomerId': [
        'Enter the ID of the manager account, such as 123-456-7890',
      ],
      'builtinSettings.customerIds': [
        'Enter up to 50 Google Ads account IDs, such as 123-456-7890, separated by commas',
      ],
    })
    assert.lengthOf(await Mcp.all(), 0)
  })

  test('needs neither a manager nor a list of accounts', async ({ client, assert }) => {
    const admin = await createAdmin()
    const { 'builtinSettings[loginCustomerId]': manager, ...form } = googleAdsForm
    void manager
    await client
      .post('/mcps')
      .loginAs(admin)
      .withCsrfToken()
      .redirects(0)
      .form({ ...form, 'builtinSettings[customerIds]': '' })

    const mcp = await Mcp.findByOrFail('slug', 'google-ads')
    assert.isNull(mcp.builtinSettings)
  })
})

test.group('Built-in Google Ads MCP: OAuth', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('asks Google for offline access and sends the redirect URI again with the code', async ({
    client,
    assert,
  }) => {
    const google = mockGoogleAds()
    try {
      const admin = await createAdmin({ email: 'ads@example.com' })
      const mcp = await createGoogleAdsMcp(admin.id, { connected: false })
      const login = await client
        .post('/login')
        .withCsrfToken()
        .redirects(0)
        .form({ email: 'ads@example.com', password: 'password123' })
      const start = await client
        .get(`/mcps/${mcp.id}/oauth/start`)
        .withSession(login.session())
        .redirects(0)

      const authorizationUrl = new URL(start.header('location')!)
      assert.equal(
        authorizationUrl.origin + authorizationUrl.pathname,
        'https://accounts.google.com/o/oauth2/v2/auth'
      )
      assert.equal(authorizationUrl.searchParams.get('scope'), GOOGLE_ADS_SCOPE)
      assert.equal(authorizationUrl.searchParams.get('access_type'), 'offline')
      assert.equal(authorizationUrl.searchParams.get('prompt'), 'consent')

      const state = authorizationUrl.searchParams.get('state')!
      const callback = await client
        .get(`/mcps/oauth/callback?state=${state}&code=google-code&scope=${GOOGLE_ADS_SCOPE}`)
        .withSession(start.session())
        .redirects(0)
      callback.assertFlashMessage('success', 'OAuth connected')

      const [exchange] = google.tokenRequests()
      assert.deepEqual(Object.fromEntries(exchange.form!), {
        client_id: '1234567890-abc.apps.googleusercontent.com',
        client_secret: 'google-client-secret',
        grant_type: 'authorization_code',
        code: 'google-code',
        redirect_uri: 'http://localhost:3333/mcps/oauth/callback',
      })

      const saved = await Mcp.findOrFail(mcp.id)
      assert.equal(McpSecretStore.decrypt(saved.oauthRefreshToken), 'google-refresh-token')
      assert.equal(saved.oauthScopes, GOOGLE_ADS_SCOPE)
      assert.equal(saved.status, 'ready')
      // The connection is checked with one request, which carries no developer token.
      const [check] = google.requests.filter(
        ({ url }) => url.hostname === 'googleads.googleapis.com'
      )
      assert.equal(check.url.pathname, '/v25/customers:listAccessibleCustomers')
      assert.equal(check.headers.get('authorization'), 'Bearer google-access-token')
      assert.isNull(check.headers.get('developer-token'))
    } finally {
      google.restore()
    }
  })
})

test.group('Built-in Google Ads MCP: monitoring', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('lists campaigns with amounts in the currency of the account', async ({
    client,
    assert,
  }) => {
    const google = mockGoogleAds()
    try {
      const admin = await createAdmin()
      await createGoogleAdsMcp(admin.id, { loginCustomerId: '9876543210' })
      const { plaintext } = await createAccessToken(admin.id)

      const { data } = await callTool(client, plaintext, 'list_campaigns', {
        customer_id: '123-456-7890',
        date_range: 'LAST_7_DAYS',
      })

      assert.deepEqual(data, {
        currency: 'EUR',
        period: 'LAST_7_DAYS',
        campaigns: [
          {
            id: '111',
            name: 'Spring sale',
            status: 'ENABLED',
            serving: 'ELIGIBLE',
            type: 'SEARCH',
            bidding_strategy: 'TARGET_SPEND',
            daily_budget: 2.5,
            budget_shared_by: 1,
            start_date: '2026-03-01',
            end_date: null,
            impressions: 12000,
            clicks: 300,
            cost: 150,
            ctr_percent: 2.5,
            average_cpc: 0.5,
            conversions: 12,
            conversion_value: 960,
            cost_per_conversion: 12.5,
          },
        ],
        truncated: false,
      })

      const [request] = google.requests
      assert.equal(request.url.pathname, `/v25/customers/${GOOGLE_ADS_CUSTOMER}/googleAds:search`)
      // The manager account the sign-in acts through, without dashes.
      assert.equal(request.headers.get('login-customer-id'), '9876543210')
      assert.include(request.json.query, "campaign.status != 'REMOVED'")
      assert.include(request.json.query, 'segments.date DURING LAST_7_DAYS')
      assert.notProperty(request.json, 'pageSize')
    } finally {
      google.restore()
    }
  })

  test('says when an account has more than it returned', async ({ client, assert }) => {
    const second = {
      ...googleAdsFixtures.campaign,
      campaign: { ...googleAdsFixtures.campaign.campaign, id: '112', name: 'Summer sale' },
    }
    const google = mockGoogleAds(({ json }) => {
      if (/ FROM campaign /.test(json?.query ?? '')) {
        return googleJson({ results: [googleAdsFixtures.campaign, second] })
      }
    })
    try {
      const admin = await createAdmin()
      await createGoogleAdsMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)

      const { data } = await callTool(client, plaintext, 'list_campaigns', {
        customer_id: GOOGLE_ADS_CUSTOMER,
        limit: 1,
      })

      assert.lengthOf(data.campaigns, 1)
      assert.isTrue(data.truncated)
      // One row more than asked for is how Google is made to say there is more.
      assert.match(google.queries()[0], / LIMIT 2$/)
    } finally {
      google.restore()
    }
  })

  test('takes a custom period, and wants both of its ends', async ({ client, assert }) => {
    const google = mockGoogleAds()
    try {
      const admin = await createAdmin()
      await createGoogleAdsMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)

      const custom = await callTool(client, plaintext, 'get_performance', {
        customer_id: GOOGLE_ADS_CUSTOMER,
        level: 'account',
        segment: 'date',
        start_date: '2026-09-01',
        end_date: '2026-09-30',
      })
      assert.equal(custom.data.period, '2026-09-01 to 2026-09-30')
      assert.include(google.queries()[0], "segments.date BETWEEN '2026-09-01' AND '2026-09-30'")
      assert.include(google.queries()[0], 'ORDER BY segments.date')

      const half = await callTool(client, plaintext, 'list_campaigns', {
        customer_id: GOOGLE_ADS_CUSTOMER,
        start_date: '2026-09-01',
      })
      assert.isTrue(half.isError)
      assert.equal(half.text, 'Set both start_date and end_date, or neither and use date_range')

      const impossible = await callTool(client, plaintext, 'list_campaigns', {
        customer_id: GOOGLE_ADS_CUSTOMER,
        start_date: '2026-02-30',
        end_date: '2026-03-01',
      })
      assert.equal(impossible.text, 'start_date must be a date such as 2026-01-31')
      assert.lengthOf(google.queries(), 1)
    } finally {
      google.restore()
    }
  })

  test('keeps agents to the accounts the admin listed', async ({ client, assert }) => {
    const google = mockGoogleAds()
    try {
      const admin = await createAdmin()
      await createGoogleAdsMcp(admin.id, { customerIds: ['5555555555'] })
      const { plaintext } = await createAccessToken(admin.id)

      const refused = await callTool(client, plaintext, 'list_campaigns', {
        customer_id: '123-456-7890',
      })
      assert.isTrue(refused.isError)
      assert.include(
        refused.text,
        'This MCP may not use the Google Ads account 123-456-7890. It is limited to: 555-555-5555.'
      )
      assert.lengthOf(google.requests, 0)

      const { data } = await callTool(client, plaintext, 'list_accounts', {})
      assert.deepEqual(data.accounts, [])
      assert.deepEqual(data.limited_to, ['555-555-5555'])
    } finally {
      google.restore()
    }
  })

  test('lists the accounts of the sign-in and the clients of its managers', async ({
    client,
    assert,
  }) => {
    const manager = { ...googleAdsFixtures.account, id: '9876543210', manager: true }
    const google = mockGoogleAds(({ url, json }) => {
      if (url.pathname === '/v25/customers:listAccessibleCustomers') {
        return googleJson({ resourceNames: ['customers/9876543210', 'customers/4444444444'] })
      }
      if (url.pathname.includes('/customers/4444444444/')) {
        return googleAdsFailure(
          [{ errorCode: { authorizationError: 'CUSTOMER_NOT_ENABLED' }, message: 'Not enabled.' }],
          403
        )
      }
      if (/ FROM customer_client /.test(json?.query ?? '')) {
        return googleJson({ results: [{ customerClient: googleAdsFixtures.account }] })
      }
      if (/ FROM customer /.test(json?.query ?? '')) {
        return googleJson({ results: [{ customer: manager }] })
      }
    })
    try {
      const admin = await createAdmin()
      await createGoogleAdsMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)

      const { data } = await callTool(client, plaintext, 'list_accounts', {})

      assert.equal(data.accounts[0].customer_id, '987-654-3210')
      assert.isTrue(data.accounts[0].manager)
      // An account that cannot be opened does not hide the others.
      assert.equal(data.accounts[1].customer_id, '444-444-4444')
      assert.include(data.accounts[1].error, 'This Google Ads account is not active')
      assert.deepEqual(data.client_accounts, [
        {
          customer_id: '123-456-7890',
          name: 'Acme Shoes',
          currency: 'EUR',
          time_zone: 'Europe/Paris',
          manager: false,
          test_account: false,
          status: 'ENABLED',
          manager_id: '987-654-3210',
        },
      ])
    } finally {
      google.restore()
    }
  })

  test('says what Google refused, and what to do about a project without access', async ({
    client,
    assert,
  }) => {
    const google = mockGoogleAds(({ json }) => {
      if (/FROM keyword_view/.test(json?.query ?? '')) {
        return googleAdsFailure(
          [
            {
              errorCode: { authorizationError: 'CLOUD_PROJECT_NOT_APPROVED_FOR_PRODUCTION' },
              message: 'The Cloud project is only approved for use with test accounts.',
            },
          ],
          403
        )
      }
      if (/bogus/.test(json?.query ?? '')) {
        return googleAdsFailure([
          {
            errorCode: { queryError: 'UNRECOGNIZED_FIELD' },
            message: "Unrecognized field in the query: 'campaign.bogus'.",
          },
        ])
      }
    })
    try {
      const admin = await createAdmin()
      await createGoogleAdsMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)

      const noAccess = await callTool(client, plaintext, 'list_keywords', {
        customer_id: GOOGLE_ADS_CUSTOMER,
      })
      assert.isTrue(noAccess.isError)
      assert.include(noAccess.text, '(CLOUD_PROJECT_NOT_APPROVED_FOR_PRODUCTION)')
      assert.include(noAccess.text, 'Apply for Explorer access on the Google Ads API Overview page')

      const badQuery = await callTool(client, plaintext, 'run_query', {
        customer_id: GOOGLE_ADS_CUSTOMER,
        query: 'SELECT campaign.bogus FROM campaign',
      })
      assert.equal(
        badQuery.text,
        "Google Ads refused the request: Unrecognized field in the query: 'campaign.bogus'. (UNRECOGNIZED_FIELD)"
      )

      const notAQuery = await callTool(client, plaintext, 'run_query', {
        customer_id: GOOGLE_ADS_CUSTOMER,
        query: 'DELETE FROM campaign',
      })
      assert.equal(
        notAQuery.text,
        'query must be a Google Ads Query Language query starting with SELECT'
      )
    } finally {
      google.restore()
    }
  })

  test('asks to re-authorize when Google no longer knows the token', async ({ client, assert }) => {
    const google = mockGoogleAds(({ url }) => {
      if (url.hostname === 'googleads.googleapis.com') {
        return googleJson(
          { error: { code: 401, message: 'Invalid credentials', status: 'UNAUTHENTICATED' } },
          401
        )
      }
    })
    try {
      const admin = await createAdmin()
      await createGoogleAdsMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)

      const { text } = await callTool(client, plaintext, 'list_accounts', {})
      assert.equal(
        text,
        'Google rejected the saved authorization. Re-authorize this MCP in MyMCPs.'
      )
    } finally {
      google.restore()
    }
  })
})

test.group('Built-in Google Ads MCP: changes', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  /** A connected MCP that may write, with every tool running on its own unless `asks` says otherwise. */
  async function writableMcp(adminId: number, asks: string[] = []) {
    const mcp = await createGoogleAdsMcp(adminId, { writeEnabled: true })
    const tools = [
      'create_campaign',
      'update_campaign',
      'set_campaign_status',
      'update_campaign_budget',
    ]
    mcp.toolApprovals = JSON.stringify(
      Object.fromEntries(tools.filter((tool) => !asks.includes(tool)).map((tool) => [tool, 'auto']))
    )
    await mcp.save()
    return mcp
  }

  test('lists no write tool until write access is allowed', async ({ client, assert }) => {
    const google = mockGoogleAds()
    try {
      const admin = await createAdmin()
      const mcp = await createGoogleAdsMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)
      const names = async () => {
        const listed = await gatewayRpc(client, plaintext, 'tools/list', {})
        return listed.result!.tools!.map((tool) => tool.name)
      }

      assert.lengthOf(await names(), 13)
      assert.notInclude(await names(), 'google-ads__update_campaign_budget')

      mcp.builtinWriteEnabled = true
      await mcp.save()
      assert.lengthOf(await names(), 28)
    } finally {
      google.restore()
    }
  })

  test('asks before the tools that commit money, until the admin decides otherwise', async ({
    client,
    assert,
  }) => {
    const google = mockGoogleAds()
    try {
      const admin = await createAdmin()
      const mcp = await createGoogleAdsMcp(admin.id, { writeEnabled: true })

      const page = await client.get(`/mcps/${mcp.id}/tools`).loginAs(admin).withInertia()
      const { tools } = page.inertiaProps as { tools: Array<{ name: string; mode: string }> }
      assert.sameMembers(
        tools.filter((tool) => tool.mode === 'ask').map((tool) => tool.name),
        ['create_campaign', 'update_campaign', 'set_campaign_status', 'update_campaign_budget']
      )
    } finally {
      google.restore()
    }
  })

  test('describes a budget change from what Google says, not from what the agent says', async ({
    client,
    assert,
  }) => {
    const google = mockGoogleAds()
    try {
      const admin = await createAdmin()
      const mcp = await writableMcp(admin.id, ['update_campaign_budget'])
      const { plaintext } = await createAccessToken(admin.id)
      // The agent meant 2.50 and wrote 250.
      const call = { customer_id: '123-456-7890', campaign_id: '111', daily_budget: 250 }

      const held = await callTool(client, plaintext, 'update_campaign_budget', call)
      assert.include(
        held.text,
        'Approval required: update_campaign_budget on Google Ads was not run.'
      )

      const request = await ApprovalRequest.query().where('mcp_id', mcp.id).firstOrFail()
      assert.deepEqual(await ApprovalService.summary(request), {
        interpreted: true,
        title: 'Change the daily budget of the campaign "Spring sale" from €2.50 to €250.00',
        details: [
          { label: 'Account', value: 'Acme Shoes (123-456-7890)' },
          { label: 'Campaign', value: 'Spring sale (enabled)' },
          { label: 'Daily budget', value: '€250.00 a day', before: '€2.50 a day' },
          { label: 'Most it can cost in a month', value: '€7,600.00', before: '€76.00' },
        ],
        warnings: [
          'The new budget is 100 times the current one.',
          'The campaign is live: the new budget applies at once.',
        ],
        toolDescription: null,
      })

      // Google checked the change without making it.
      const budgetChange = [
        {
          campaignBudgetOperation: {
            update: {
              resourceName: `customers/${GOOGLE_ADS_CUSTOMER}/campaignBudgets/222`,
              amountMicros: '250000000',
            },
            updateMask: 'amount_micros',
          },
        },
      ]
      assert.deepEqual(google.validations(), [budgetChange])
      assert.lengthOf(google.mutations(), 0)

      await ApprovalService.decide(request, 'approve', admin)
      const approved = await callTool(client, plaintext, 'update_campaign_budget', call)
      assert.deepEqual(approved.data, {
        campaign_id: '111',
        name: 'Spring sale',
        daily_budget: 250,
        previous_daily_budget: 2.5,
        currency: 'EUR',
      })
      assert.deepEqual(google.mutations(), [budgetChange])
    } finally {
      google.restore()
    }
  })

  test('does not ask anyone to approve what Google would refuse', async ({ client, assert }) => {
    const google = mockGoogleAds(({ url, json }) => {
      if (url.pathname.endsWith('/googleAds:mutate') && json.validateOnly) {
        return googleAdsFailure([
          {
            errorCode: { campaignBudgetError: 'MONEY_AMOUNT_TOO_LARGE' },
            message: 'The amount is too large.',
            fields: ['mutate_operations', 'campaign_budget_operation', 'update', 'amount_micros'],
          },
        ])
      }
    })
    try {
      const admin = await createAdmin()
      await writableMcp(admin.id, ['update_campaign_budget'])
      const { plaintext } = await createAccessToken(admin.id)

      const refused = await callTool(client, plaintext, 'update_campaign_budget', {
        customer_id: GOOGLE_ADS_CUSTOMER,
        campaign_id: '111',
        daily_budget: 900000,
      })

      assert.isTrue(refused.isError)
      assert.equal(
        refused.text,
        'Google Ads refused the request: The amount is too large. (MONEY_AMOUNT_TOO_LARGE) at mutate_operations.campaign_budget_operation.update.amount_micros'
      )
      assert.lengthOf(await ApprovalRequest.all(), 0)
    } finally {
      google.restore()
    }
  })

  test('creates a paused campaign with its own budget in one request', async ({
    client,
    assert,
  }) => {
    const google = mockGoogleAds(({ json }) => {
      if (/FROM geo_target_constant/.test(json?.query ?? '')) {
        return googleJson({
          results: [{ geoTargetConstant: { id: '2250', canonicalName: 'France', name: 'France' } }],
        })
      }
    })
    try {
      const admin = await createAdmin()
      await writableMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)

      const { data } = await callTool(client, plaintext, 'create_campaign', {
        customer_id: GOOGLE_ADS_CUSTOMER,
        name: 'Autumn sale',
        channel: 'SEARCH',
        daily_budget: 12.5,
        bidding_strategy: 'MAXIMIZE_CLICKS',
        max_cpc: 1.2,
        location_ids: [2250],
        start_date: '2026-11-01',
        end_date: '2026-11-30',
      })

      assert.equal(data.campaign_id, '9001')
      assert.equal(data.status, 'PAUSED')
      const customer = `customers/${GOOGLE_ADS_CUSTOMER}`
      assert.deepEqual(google.mutations(), [
        [
          {
            campaignBudgetOperation: {
              create: {
                resourceName: `${customer}/campaignBudgets/-1`,
                amountMicros: '12500000',
                deliveryMethod: 'STANDARD',
                explicitlyShared: false,
              },
            },
          },
          {
            campaignOperation: {
              create: {
                resourceName: `${customer}/campaigns/-2`,
                name: 'Autumn sale',
                status: 'PAUSED',
                advertisingChannelType: 'SEARCH',
                campaignBudget: `${customer}/campaignBudgets/-1`,
                networkSettings: {
                  targetGoogleSearch: true,
                  targetSearchNetwork: false,
                  targetContentNetwork: false,
                  targetPartnerSearchNetwork: false,
                },
                targetSpend: { cpcBidCeilingMicros: '1200000' },
                startDateTime: '2026-11-01 00:00:00',
                endDateTime: '2026-11-30 23:59:59',
                containsEuPoliticalAdvertising: 'DOES_NOT_CONTAIN_EU_POLITICAL_ADVERTISING',
              },
            },
          },
          {
            campaignCriterionOperation: {
              create: {
                campaign: `${customer}/campaigns/-2`,
                location: { geoTargetConstant: 'geoTargetConstants/2250' },
              },
            },
          },
        ],
      ])
    } finally {
      google.restore()
    }
  })

  test('refuses settings that do not go together before calling Google', async ({
    client,
    assert,
  }) => {
    const google = mockGoogleAds()
    try {
      const admin = await createAdmin()
      await writableMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)
      const campaign = {
        customer_id: GOOGLE_ADS_CUSTOMER,
        name: 'Autumn sale',
        channel: 'SEARCH',
        daily_budget: 12.5,
        bidding_strategy: 'MAXIMIZE_CLICKS',
      }
      const create = (overrides: Record<string, unknown>) =>
        callTool(client, plaintext, 'create_campaign', { ...campaign, ...overrides })

      const target = await create({ target_cpa: 20 })
      assert.equal(target.text, 'target_cpa does not go with MAXIMIZE_CLICKS, which takes max_cpc')

      const languages = await create({ languages: ['fr'] })
      assert.include(languages.text, 'Google no longer takes languages for Search campaigns')

      const networks = await create({ channel: 'DISPLAY', search_partners: true })
      assert.equal(
        networks.text,
        'search_partners and display_network are for Search campaigns only'
      )

      const budget = await create({ daily_budget: 0 })
      assert.equal(budget.text, 'daily_budget must be a number between 0.01 and 1000000')
      assert.lengthOf(google.requests, 0)
    } finally {
      google.restore()
    }
  })

  test('describes enabling a campaign by the budget it frees', async ({ client, assert }) => {
    const paused = {
      ...googleAdsFixtures.campaign,
      campaign: { ...googleAdsFixtures.campaign.campaign, status: 'PAUSED' },
    }
    const google = mockGoogleAds(({ json }) => {
      if (/ FROM campaign /.test(json?.query ?? '')) return googleJson({ results: [paused] })
    })
    try {
      const admin = await createAdmin()
      const mcp = await writableMcp(admin.id, ['set_campaign_status'])
      const { plaintext } = await createAccessToken(admin.id)

      await callTool(client, plaintext, 'set_campaign_status', {
        customer_id: GOOGLE_ADS_CUSTOMER,
        campaign_id: 111,
        status: 'ENABLED',
      })

      const request = await ApprovalRequest.query().where('mcp_id', mcp.id).firstOrFail()
      const summary = await ApprovalService.summary(request)
      assert.equal(summary?.title, 'Enable the campaign "Spring sale", which can spend €2.50 a day')
      assert.deepInclude(summary?.details, {
        label: 'Status',
        value: 'Enabled',
        before: 'Paused',
      })
    } finally {
      google.restore()
    }
  })

  test('removes a location by what the campaign says it is, never an exclusion', async ({
    client,
    assert,
  }) => {
    const criterion = (id: string, geo: string, negative: boolean) => ({
      campaignCriterion: {
        resourceName: `customers/${GOOGLE_ADS_CUSTOMER}/campaignCriteria/111~${id}`,
        criterionId: id,
        type: 'LOCATION',
        negative,
        location: { geoTargetConstant: `geoTargetConstants/${geo}` },
      },
    })
    const google = mockGoogleAds(({ json }) => {
      const query: string = json?.query ?? ''
      if (/FROM geo_target_constant/.test(query)) {
        return googleJson({
          results: [
            { geoTargetConstant: { id: '2250', canonicalName: 'France' } },
            { geoTargetConstant: { id: '2056', canonicalName: 'Belgium' } },
          ].filter(({ geoTargetConstant }) => query.includes(geoTargetConstant.id)),
        })
      }
      if (/FROM campaign_criterion/.test(query)) {
        return googleJson({
          results: [criterion('2250', '2250', true), criterion('2056', '2056', false)],
        })
      }
    })
    try {
      const admin = await createAdmin()
      await writableMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)
      const targeting = { customer_id: GOOGLE_ADS_CUSTOMER, campaign_id: '111' }

      // The campaign excludes France: taking that away would open France to its ads.
      const exclusion = await callTool(client, plaintext, 'update_campaign_targeting', {
        ...targeting,
        remove_location_ids: ['2250'],
      })
      assert.isTrue(exclusion.isError)
      assert.include(exclusion.text, 'The campaign 111 excludes France')
      assert.include(exclusion.text, 'pass its criterion ID 2250 in remove_criterion_ids')
      assert.lengthOf(google.mutations(), 0)

      const removed = await callTool(client, plaintext, 'update_campaign_targeting', {
        ...targeting,
        remove_location_ids: ['2056'],
      })
      assert.deepEqual(removed.data, { campaign_id: '111', added: 0, removed: 1 })
      assert.deepEqual(google.mutations(), [
        [
          {
            campaignCriterionOperation: {
              remove: `customers/${GOOGLE_ADS_CUSTOMER}/campaignCriteria/111~2056`,
            },
          },
        ],
      ])
    } finally {
      google.restore()
    }
  })

  test('adds keywords to an ad group and returns their IDs', async ({ client, assert }) => {
    const google = mockGoogleAds()
    try {
      const admin = await createAdmin()
      await writableMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)

      const { data } = await callTool(client, plaintext, 'add_keywords', {
        customer_id: GOOGLE_ADS_CUSTOMER,
        ad_group_id: '333',
        keywords: [
          { text: ' running shoes ', match_type: 'PHRASE', max_cpc: 0.8 },
          { text: 'trail shoes', match_type: 'EXACT' },
        ],
      })

      assert.deepEqual(data.keywords, [
        { criterion_id: '9000', text: 'running shoes', match_type: 'PHRASE' },
        { criterion_id: '9001', text: 'trail shoes', match_type: 'EXACT' },
      ])
      const adGroup = `customers/${GOOGLE_ADS_CUSTOMER}/adGroups/333`
      assert.deepEqual(google.mutations(), [
        [
          {
            adGroupCriterionOperation: {
              create: {
                adGroup,
                keyword: { text: 'running shoes', matchType: 'PHRASE' },
                status: 'ENABLED',
                cpcBidMicros: '800000',
              },
            },
          },
          {
            adGroupCriterionOperation: {
              create: {
                adGroup,
                keyword: { text: 'trail shoes', matchType: 'EXACT' },
                status: 'ENABLED',
              },
            },
          },
        ],
      ])

      const unmatched = await callTool(client, plaintext, 'add_keywords', {
        customer_id: GOOGLE_ADS_CUSTOMER,
        ad_group_id: '333',
        keywords: [{ text: 'running shoes' }],
      })
      assert.include(unmatched.text, 'keywords must be a list of 1 to 100 keywords such as')
    } finally {
      google.restore()
    }
  })

  test('creates a search ad from its headlines and descriptions', async ({ client, assert }) => {
    const google = mockGoogleAds()
    try {
      const admin = await createAdmin()
      await writableMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)
      const ad = {
        customer_id: GOOGLE_ADS_CUSTOMER,
        ad_group_id: '333',
        headlines: ['Running shoes', 'Free delivery', 'Shop the spring sale'],
        descriptions: ['Light shoes for every road.', 'Returns are free for 30 days.'],
        final_url: 'https://acme.example/shoes',
        path1: 'shoes',
      }

      const { data } = await callTool(client, plaintext, 'create_responsive_search_ad', ad)
      assert.deepEqual(data, { ad_id: '9000', ad_group_id: '333', status: 'ENABLED' })
      assert.deepEqual(google.mutations()[0][0].adGroupAdOperation.create.ad, {
        finalUrls: ['https://acme.example/shoes'],
        responsiveSearchAd: {
          headlines: [
            { text: 'Running shoes' },
            { text: 'Free delivery' },
            { text: 'Shop the spring sale' },
          ],
          descriptions: [
            { text: 'Light shoes for every road.' },
            { text: 'Returns are free for 30 days.' },
          ],
          path1: 'shoes',
        },
      })

      const short = await callTool(client, plaintext, 'create_responsive_search_ad', {
        ...ad,
        headlines: ['Running shoes', 'A headline that runs well past the thirty characters'],
      })
      assert.equal(
        short.text,
        'headlines must be a list of 3 to 15 headlines of at most 30 characters each'
      )
    } finally {
      google.restore()
    }
  })

  test('turns an uploaded image into an asset, and says what it can be used as', async ({
    client,
    assert,
  }) => {
    const google = mockGoogleAds()
    try {
      const admin = await createAdmin()
      const mcp = await writableMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)

      const link = await callTool(client, plaintext, 'create_image_upload_link', {
        filename: 'banner.png',
      })
      assert.match(link.data.url, /^http:\/\/localhost:3333\/uploads\/\d+\//)
      assert.equal(link.data.max_bytes, 5_242_880)

      const upload = await client.put(
        new URL(link.data.url).pathname + new URL(link.data.url).search
      )
      // Nothing was sent: the link is the one a file goes to.
      upload.assertStatus(400)

      const image = png(1200, 628)
      await saveBuiltinUpload(
        mcp.id,
        { id: link.data.upload_id, filename: 'banner.png', maxBytes: 5_242_880 },
        chunks(image)
      )
      const { data } = await callTool(client, plaintext, 'create_image_asset', {
        customer_id: GOOGLE_ADS_CUSTOMER,
        upload_id: link.data.upload_id,
        name: 'Spring banner',
      })

      assert.deepEqual(data, {
        asset_id: '9000',
        name: 'Spring banner',
        width: 1200,
        height: 628,
        shape: 'landscape',
        large_enough: true,
      })
      assert.deepEqual(google.mutations(), [
        [
          {
            assetOperation: {
              create: {
                name: 'Spring banner',
                type: 'IMAGE',
                imageAsset: { data: image.toString('base64') },
              },
            },
          },
        ],
      ])

      const missing = await callTool(client, plaintext, 'create_image_asset', {
        customer_id: GOOGLE_ADS_CUSTOMER,
        upload_id: '11111111-2222-4333-8444-555555555555',
        name: 'Nothing',
      })
      assert.include(missing.text, 'No file is uploaded as "11111111-2222-4333-8444-555555555555"')
    } finally {
      google.restore()
    }
  }).teardown(async () => {
    const { removeBuiltinUploads } = await import('#services/builtin/upload_store')
    const mcps = await Mcp.all()
    await Promise.all(mcps.map((mcp) => removeBuiltinUploads(mcp.id)))
  })

  test('only puts images of the right shape in a display ad', async ({ client, assert }) => {
    const image = (id: string, width: number, height: number) => ({
      asset: {
        resourceName: `customers/${GOOGLE_ADS_CUSTOMER}/assets/${id}`,
        id,
        name: `Image ${id}`,
        type: 'IMAGE',
        imageAsset: { fullSize: { widthPixels: String(width), heightPixels: String(height) } },
      },
    })
    const google = mockGoogleAds(({ json }) => {
      if (/ FROM asset /.test(json?.query ?? '')) {
        return googleJson({ results: [image('71', 1200, 628), image('72', 600, 600)] })
      }
    })
    try {
      const admin = await createAdmin()
      await writableMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)
      const ad = {
        customer_id: GOOGLE_ADS_CUSTOMER,
        ad_group_id: '333',
        marketing_image_asset_ids: ['71'],
        square_marketing_image_asset_ids: ['72'],
        headlines: ['Running shoes'],
        long_headline: 'Light running shoes for every road',
        descriptions: ['Returns are free for 30 days.'],
        business_name: 'Acme Shoes',
        final_url: 'https://acme.example/shoes',
      }

      const created = await callTool(client, plaintext, 'create_responsive_display_ad', ad)
      assert.equal(created.data.ad_id, '9000')
      const display = google.mutations()[0][0].adGroupAdOperation.create.ad.responsiveDisplayAd
      assert.deepEqual(display.marketingImages, [
        { asset: `customers/${GOOGLE_ADS_CUSTOMER}/assets/71` },
      ])
      assert.deepEqual(display.longHeadline, { text: 'Light running shoes for every road' })

      const swapped = await callTool(client, plaintext, 'create_responsive_display_ad', {
        ...ad,
        marketing_image_asset_ids: ['72'],
        square_marketing_image_asset_ids: ['71'],
      })
      assert.equal(
        swapped.text,
        'marketing_image_asset_ids takes landscape (1.91:1) images of at least 600×314 pixels, and the image asset 72 is 600×600'
      )
      assert.lengthOf(google.mutations(), 1)
    } finally {
      google.restore()
    }
  })
})
