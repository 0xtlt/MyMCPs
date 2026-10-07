import { test } from '@japa/runner'
import limiter from '@adonisjs/limiter/services/main'
import { DateTime } from 'luxon'
import ApprovalRequest from '#models/approval_request'
import type Mcp from '#models/mcp'
import McpCallLog from '#models/mcp_call_log'
import User from '#models/user'
import ApprovalService, { argumentsHash } from '#services/approvals/approval_service'
import { APPROVAL_NOTE } from '#services/approvals/policy'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAccessToken, createAdmin, createMcp, createMember } from '#tests/helpers/factories'
import { gatewayRpc, mockHttpMcp, resultText } from '#tests/helpers/gateway'
import { createStravaMcp, mockStrava } from '#tests/helpers/strava'

const ORIGIN = 'https://crm.example'
const TOOLS = [
  { name: 'list_contacts', description: 'List the contacts of the account.' },
  { name: 'delete_contact', description: 'Delete a contact for good.' },
]

/** An MCP reached over HTTP whose `delete_contact` asks for approval. */
async function crmMcp(createdBy: number) {
  const mcp = await createMcp(createdBy, { name: 'CRM', slug: 'crm', httpUrl: `${ORIGIN}/mcp` })
  mcp.toolApprovals = JSON.stringify({ delete_contact: 'ask' })
  await mcp.save()
  return mcp
}

function linkIn(text: string) {
  return text.match(/http:\/\/localhost:3333\/approvals\/([A-Za-z0-9_-]{32})/)
}

async function decide(mcp: Mcp, decision: 'approve' | 'deny', userId: number) {
  const request = await ApprovalRequest.query()
    .where('mcp_id', mcp.id)
    .orderBy('id', 'desc')
    .first()
  const user = await User.findOrFail(userId)
  return ApprovalService.decide(request!, decision, user)
}

test.group('Tool approvals: gateway', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('holds a tool that asks and hands the agent a link instead of running it', async ({
    client,
    assert,
  }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const mcp = await crmMcp(admin.id)
      const { plaintext, token } = await createAccessToken(admin.id, { name: 'Claude' })

      const response = await gatewayRpc(client, plaintext, 'tools/call', {
        name: 'crm__delete_contact',
        arguments: { id: 42, reason: 'duplicate' },
      })

      assert.isTrue(response.result?.isError)
      const text = resultText(response)
      assert.include(text, 'Approval required: delete_contact on CRM was not run.')
      assert.include(text, 'call delete_contact again with exactly the same arguments')
      assert.lengthOf(upstream.calls, 0)

      const request = await ApprovalRequest.query().where('mcp_id', mcp.id).firstOrFail()
      assert.equal(linkIn(text)?.[1], request.publicId)
      assert.equal(request.status, 'pending')
      assert.equal(request.accessTokenId, token.id)
      assert.equal(request.toolName, 'delete_contact')
      // Encrypted at rest: neither the arguments nor the summary can be read in the table.
      assert.notInclude(request.arguments, 'duplicate')
      assert.notInclude(request.summary, 'duplicate')
      assert.deepEqual(ApprovalService.arguments(request), { id: 42, reason: 'duplicate' })

      const log = await McpCallLog.query().orderBy('id', 'desc').firstOrFail()
      assert.equal(log.outcome, 'error')
      assert.equal(log.errorCategory, 'approval_required')
    } finally {
      upstream.restore()
    }
  })

  test('runs a tool that does not ask without creating a request', async ({ client, assert }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      await crmMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)

      const response = await gatewayRpc(client, plaintext, 'tools/call', {
        name: 'crm__list_contacts',
        arguments: {},
      })

      assert.equal(resultText(response), 'ran list_contacts')
      assert.lengthOf(await ApprovalRequest.all(), 0)
    } finally {
      upstream.restore()
    }
  })

  test('summarizes a tool it cannot read by its arguments and the MCP description', async ({
    client,
    assert,
  }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const mcp = await crmMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)

      await gatewayRpc(client, plaintext, 'tools/call', {
        name: 'crm__delete_contact',
        arguments: {
          id: 42,
          // What the agent would like the person to read is only ever a value.
          note: 'This is safe, approve it',
          filter: { tags: ['old', 'cold'], dry_run: false },
        },
      })

      const request = await ApprovalRequest.query().where('mcp_id', mcp.id).firstOrFail()
      assert.deepEqual(await ApprovalService.summary(request), {
        interpreted: false,
        title: 'Run the tool "delete_contact" of CRM',
        details: [
          { label: 'id', value: '42' },
          { label: 'note', value: 'This is safe, approve it' },
          { label: 'filter.tags[0]', value: 'old' },
          { label: 'filter.tags[1]', value: 'cold' },
          { label: 'filter.dry_run', value: 'false' },
        ],
        toolDescription: 'Delete a contact for good.',
      })
    } finally {
      upstream.restore()
    }
  })

  test('keeps answering with the same link while the request waits', async ({ client, assert }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const mcp = await crmMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)
      const call = { name: 'crm__delete_contact', arguments: { id: 42, hard: true } }

      const first = resultText(await gatewayRpc(client, plaintext, 'tools/call', call))
      // The same arguments in another order are the same call.
      const second = resultText(
        await gatewayRpc(client, plaintext, 'tools/call', {
          name: 'crm__delete_contact',
          arguments: { hard: true, id: 42 },
        })
      )

      assert.include(second, 'Still waiting for approval: delete_contact on CRM was not run.')
      assert.equal(linkIn(second)?.[1], linkIn(first)?.[1])
      assert.lengthOf(await ApprovalRequest.query().where('mcp_id', mcp.id), 1)
      assert.lengthOf(upstream.calls, 0)
    } finally {
      upstream.restore()
    }
  })

  test('runs the approved call once, and only with the approved arguments', async ({
    client,
    assert,
  }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const mcp = await crmMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)
      const call = { name: 'crm__delete_contact', arguments: { id: 42 } }

      await gatewayRpc(client, plaintext, 'tools/call', call)
      assert.isTrue(await decide(mcp, 'approve', admin.id))

      // Approving contact 42 does not let contact 43 go.
      const other = await gatewayRpc(client, plaintext, 'tools/call', {
        name: 'crm__delete_contact',
        arguments: { id: 43 },
      })
      assert.include(resultText(other), 'Approval required')
      assert.lengthOf(upstream.calls, 0)

      const approved = await gatewayRpc(client, plaintext, 'tools/call', call)
      assert.notExists(approved.result?.isError)
      assert.equal(resultText(approved), 'ran delete_contact')
      assert.deepEqual(upstream.calls, [{ name: 'delete_contact', arguments: { id: 42 } }])

      const used = await ApprovalRequest.query()
        .where('arguments_hash', argumentsHash({ id: 42 }))
        .firstOrFail()
      assert.equal(used.state, 'used')

      // The approval is spent: the same call asks again.
      const again = await gatewayRpc(client, plaintext, 'tools/call', call)
      assert.include(resultText(again), 'Approval required')
      assert.lengthOf(upstream.calls, 1)
    } finally {
      upstream.restore()
    }
  })

  test('does not let another access token use an approval', async ({ client, assert }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const mcp = await crmMcp(admin.id)
      const asking = await createAccessToken(admin.id)
      const other = await createAccessToken(admin.id)
      const call = { name: 'crm__delete_contact', arguments: { id: 42 } }

      await gatewayRpc(client, asking.plaintext, 'tools/call', call)
      await decide(mcp, 'approve', admin.id)

      const response = await gatewayRpc(client, other.plaintext, 'tools/call', call)
      assert.include(resultText(response), 'Approval required')
      assert.lengthOf(upstream.calls, 0)
    } finally {
      upstream.restore()
    }
  })

  test('tells the agent once that the call was denied', async ({ client, assert }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const mcp = await crmMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)
      const call = { name: 'crm__delete_contact', arguments: { id: 42 } }

      await gatewayRpc(client, plaintext, 'tools/call', call)
      await decide(mcp, 'deny', admin.id)

      const denied = await gatewayRpc(client, plaintext, 'tools/call', call)
      assert.isTrue(denied.result?.isError)
      assert.include(resultText(denied), 'Denied: a person refused this call to delete_contact')
      const log = await McpCallLog.query().orderBy('id', 'desc').firstOrFail()
      assert.equal(log.errorCategory, 'approval_denied')

      // Asked again, it is a new request for the person to decide.
      const again = await gatewayRpc(client, plaintext, 'tools/call', call)
      assert.include(resultText(again), 'Approval required')
      assert.lengthOf(await ApprovalRequest.query().where('mcp_id', mcp.id), 2)
      assert.lengthOf(upstream.calls, 0)
    } finally {
      upstream.restore()
    }
  })

  test('asks again once an approval has expired', async ({ client, assert }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const mcp = await crmMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)
      const call = { name: 'crm__delete_contact', arguments: { id: 42 } }

      await gatewayRpc(client, plaintext, 'tools/call', call)
      await decide(mcp, 'approve', admin.id)
      const request = await ApprovalRequest.query().where('mcp_id', mcp.id).firstOrFail()
      request.expiresAt = DateTime.utc().minus({ minutes: 1 })
      await request.save()

      const response = await gatewayRpc(client, plaintext, 'tools/call', call)
      assert.include(resultText(response), 'Approval required')
      assert.lengthOf(upstream.calls, 0)
      await request.refresh()
      assert.equal(request.state, 'expired')
    } finally {
      upstream.restore()
    }
  })

  test('holds the calls of the lazy gateway the same way', async ({ client, assert }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const mcp = await crmMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)
      const call = {
        name: 'call_tool',
        arguments: { mcp: 'crm', tool: 'delete_contact', arguments: { id: 42 } },
      }

      const held = await gatewayRpc(client, plaintext, 'tools/call', call, 'lazy')
      assert.include(resultText(held), 'Approval required')
      await decide(mcp, 'approve', admin.id)

      const approved = await gatewayRpc(client, plaintext, 'tools/call', call, 'lazy')
      assert.equal(resultText(approved), 'ran delete_contact')
      assert.deepEqual(upstream.calls, [{ name: 'delete_contact', arguments: { id: 42 } }])
    } finally {
      upstream.restore()
    }
  })

  test('tells agents which tools ask, where they list and search them', async ({
    client,
    assert,
  }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      await crmMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)

      const listed = await gatewayRpc(client, plaintext, 'tools/list', {})
      const descriptions = Object.fromEntries(
        listed.result!.tools!.map((tool) => [tool.name, tool.description])
      )
      assert.equal(descriptions.crm__list_contacts, 'List the contacts of the account.')
      assert.equal(
        descriptions.crm__delete_contact,
        `Delete a contact for good.\n\n${APPROVAL_NOTE}`
      )

      const found = await gatewayRpc(
        client,
        plaintext,
        'tools/call',
        { name: 'tool_search', arguments: { mcp: 'crm', query: 'delete' } },
        'lazy'
      )
      assert.include(resultText(found), 'Needs approval')
    } finally {
      upstream.restore()
    }
  })

  test('refuses a tool the MCP does not have without asking anyone', async ({ client, assert }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const mcp = await crmMcp(admin.id)
      mcp.toolApprovals = JSON.stringify({ drop_database: 'ask' })
      await mcp.save()
      const { plaintext } = await createAccessToken(admin.id)

      const response = await gatewayRpc(client, plaintext, 'tools/call', {
        name: 'crm__drop_database',
        arguments: {},
      })

      assert.isTrue(response.result?.isError)
      assert.equal(resultText(response), 'CRM has no tool named "drop_database"')
      assert.lengthOf(await ApprovalRequest.all(), 0)
    } finally {
      upstream.restore()
    }
  })

  test('stops an access token from piling up requests', async ({ client, assert }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      await crmMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)
      const ask = (id: number) =>
        gatewayRpc(client, plaintext, 'tools/call', {
          name: 'crm__delete_contact',
          arguments: { id },
        })

      for (let id = 1; id <= 20; id += 1) await ask(id)
      const refused = await ask(21)

      assert.include(resultText(refused), '20 calls of this access token already wait for approval')
      assert.lengthOf(await ApprovalRequest.all(), 20)
    } finally {
      upstream.restore()
    }
  })

  test('holds the limit when the calls of an access token arrive at once', async ({ assert }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const mcp = await crmMcp(admin.id)
      const { token } = await createAccessToken(admin.id)

      // Straight to the gate: each call reads what waits, asks the MCP for
      // its tools, then adds its request, and all of them start together.
      const gates = await Promise.all(
        Array.from({ length: 25 }, (_, id) =>
          ApprovalService.gate({
            accessToken: token,
            mcp,
            toolName: 'delete_contact',
            args: { id },
          })
        )
      )

      assert.lengthOf(await ApprovalRequest.all(), 20)
      assert.lengthOf(
        gates.filter((gate) => gate.held && gate.category === 'tool_error'),
        5
      )
    } finally {
      upstream.restore()
    }
  })

  test('asks once for the same call made twice at the same moment', async ({ assert }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const mcp = await crmMcp(admin.id)
      const { token } = await createAccessToken(admin.id)
      const call = { accessToken: token, mcp, toolName: 'delete_contact', args: { id: 42 } }

      const [first, second] = await Promise.all([
        ApprovalService.gate(call),
        ApprovalService.gate(call),
      ])

      // Two requests would be two links to approve, and the call would run twice.
      assert.lengthOf(await ApprovalRequest.all(), 1)
      assert.isTrue(first.held && second.held)
      const text = (gate: typeof first) => {
        const [content] = gate.held ? gate.result.content : []
        return content?.type === 'text' ? content.text : ''
      }
      assert.equal(linkIn(text(first))?.[1], linkIn(text(second))?.[1])
    } finally {
      upstream.restore()
    }
  })

  test('holds every tool when the saved choices cannot be read', async ({ client, assert }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const mcp = await crmMcp(admin.id)
      mcp.toolApprovals = '{"delete_contact":"sometimes"'
      await mcp.save()
      const { plaintext } = await createAccessToken(admin.id)

      const response = await gatewayRpc(client, plaintext, 'tools/call', {
        name: 'crm__list_contacts',
        arguments: {},
      })

      assert.include(resultText(response), 'Approval required')
      assert.lengthOf(upstream.calls, 0)
    } finally {
      upstream.restore()
    }
  })
})

test.group('Tool approvals: built-in MCPs', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('checks the call before asking anyone to approve it', async ({ client, assert }) => {
    const strava = mockStrava()
    try {
      const admin = await createAdmin()
      const mcp = await createStravaMcp(admin.id, { writeEnabled: true })
      mcp.toolApprovals = JSON.stringify({ update_athlete_weight: 'ask' })
      await mcp.save()
      const { plaintext } = await createAccessToken(admin.id)

      const invalid = await gatewayRpc(client, plaintext, 'tools/call', {
        name: 'strava__update_athlete_weight',
        arguments: { weight: 4000 },
      })
      assert.isTrue(invalid.result?.isError)
      assert.equal(resultText(invalid), 'weight must be a number between 20 and 400')
      assert.lengthOf(await ApprovalRequest.all(), 0)

      const held = await gatewayRpc(client, plaintext, 'tools/call', {
        name: 'strava__update_athlete_weight',
        arguments: { weight: 71.5 },
      })
      assert.include(resultText(held), 'Approval required: update_athlete_weight on Strava')
      assert.lengthOf(strava.apiRequests(), 0)

      const request = await ApprovalRequest.query().where('mcp_id', mcp.id).firstOrFail()
      const summary = await ApprovalService.summary(request)
      assert.equal(summary?.title, 'Run the tool "update_athlete_weight" of Strava')
      assert.deepEqual(summary?.details, [{ label: 'weight', value: '71.5' }])
      // Written by MyMCPs itself for its own tool.
      assert.include(summary?.toolDescription, "Set the connected athlete's weight")
    } finally {
      strava.restore()
    }
  })

  test('does not ask for a tool the MCP would refuse anyway', async ({ client, assert }) => {
    const strava = mockStrava()
    try {
      const admin = await createAdmin()
      const mcp = await createStravaMcp(admin.id, { writeEnabled: false })
      mcp.toolApprovals = JSON.stringify({ update_athlete_weight: 'ask' })
      await mcp.save()
      const { plaintext } = await createAccessToken(admin.id)

      const response = await gatewayRpc(client, plaintext, 'tools/call', {
        name: 'strava__update_athlete_weight',
        arguments: { weight: 71.5 },
      })

      assert.include(resultText(response), 'write access is turned off for this MCP')
      assert.lengthOf(await ApprovalRequest.all(), 0)
    } finally {
      strava.restore()
    }
  })
})

test.group('Tool approvals: pages', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(rollbackTestTransaction)

  /** A request waiting for a decision, as a call through the gateway leaves it. */
  async function waitingRequest(client: Parameters<typeof gatewayRpc>[0], adminId: number) {
    const mcp = await crmMcp(adminId)
    const { plaintext } = await createAccessToken(adminId, { name: 'Claude' })
    await gatewayRpc(client, plaintext, 'tools/call', {
      name: 'crm__delete_contact',
      arguments: { id: 42 },
    })
    return ApprovalRequest.query().where('mcp_id', mcp.id).firstOrFail()
  }

  test('sends a guest to sign in and brings them back to the request', async ({
    client,
    assert,
  }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin({ email: 'admin@example.com' })
      const request = await waitingRequest(client, admin.id)

      const guest = await client.get(`/approvals/${request.publicId}`).redirects(0)
      guest.assertStatus(302)
      assert.equal(guest.header('location'), '/login')
      guest.assertSession('approvalReturnTo', `/approvals/${request.publicId}`)

      const signedIn = await client
        .post('/login')
        .withCsrfToken()
        .withSession({ approvalReturnTo: `/approvals/${request.publicId}` })
        .redirects(0)
        .form({ email: 'admin@example.com', password: 'password123' })
      signedIn.assertStatus(302)
      assert.equal(signedIn.header('location'), `/approvals/${request.publicId}`)
    } finally {
      upstream.restore()
    }
  })

  test('only ever returns from sign-in to an approval request', async ({ client, assert }) => {
    await createAdmin({ email: 'admin@example.com' })

    for (const returnTo of ['https://evil.example/approvals/x', '/approvals/../settings', '//x']) {
      const response = await client
        .post('/login')
        .withCsrfToken()
        .withSession({ approvalReturnTo: returnTo })
        .redirects(0)
        .form({ email: 'admin@example.com', password: 'password123' })
      assert.equal(response.header('location'), '/')
      limiter.clear(['memory'])
    }
  })

  test('shows what MyMCPs read in the call, and the exact arguments', async ({ client }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const request = await waitingRequest(client, admin.id)

      const response = await client
        .get(`/approvals/${request.publicId}`)
        .loginAs(admin)
        .withInertia()

      response.assertStatus(200)
      response.assertInertiaComponent('approvals/show')
      response.assertInertiaPropsContains({
        approval: {
          id: request.publicId,
          state: 'pending',
          toolName: 'delete_contact',
          title: 'Run the tool "delete_contact" of CRM',
          interpreted: false,
          mcp: { name: 'CRM', slug: 'crm' },
          accessToken: { name: 'Claude' },
        },
        summary: {
          interpreted: false,
          details: [{ label: 'id', value: '42' }],
          toolDescription: 'Delete a contact for good.',
        },
        arguments: '{\n  "id": 42\n}',
        runnable: true,
        pendingApprovals: 1,
      })
    } finally {
      upstream.restore()
    }
  })

  test('lets a member decide the calls of their own access tokens, and records it', async ({
    client,
    assert,
  }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const member = await createMember({ fullName: 'Mona Member' })
      const request = await waitingRequest(client, member.id)

      const response = await client
        .post(`/approvals/${request.publicId}`)
        .loginAs(member)
        .withCsrfToken()
        .redirects(0)
        .form({ decision: 'approve' })

      response.assertStatus(302)
      assert.equal(response.header('location'), `/approvals/${request.publicId}`)
      response.assertFlashMessage('success', 'Approved. The agent can now run this call, once.')
      await request.refresh()
      assert.equal(request.status, 'approved')
      assert.equal(request.decidedBy, member.id)
      assert.isTrue(request.expiresAt > DateTime.utc().plus({ hours: 23 }))

      // An administrator reads every request.
      const page = await client.get(`/approvals/${request.publicId}`).loginAs(admin).withInertia()
      page.assertInertiaPropsContains({
        approval: { state: 'approved', decidedBy: 'Mona Member' },
        pendingApprovals: 0,
      })
    } finally {
      upstream.restore()
    }
  })

  test('keeps the calls of an access token from the other members', async ({ client, assert }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const other = await createMember()
      const request = await waitingRequest(client, admin.id)

      // The same answer as for a request that does not exist.
      const page = await client.get(`/approvals/${request.publicId}`).loginAs(other).redirects(0)
      page.assertStatus(302)
      assert.equal(page.header('location'), '/approvals')

      const decision = await client
        .post(`/approvals/${request.publicId}`)
        .loginAs(other)
        .withCsrfToken()
        .redirects(0)
        .form({ decision: 'approve' })
      assert.include(
        decision.flashMessage('error'),
        'belongs to the access token of another member'
      )
      await request.refresh()
      assert.equal(request.status, 'pending')

      const list = await client.get('/approvals').loginAs(other).withInertia()
      list.assertInertiaPropsContains({ waiting: [], past: [], pendingApprovals: 0 })
      const all = await client.get('/approvals').loginAs(admin).withInertia()
      all.assertInertiaPropsContains({
        waiting: [{ id: request.publicId }],
        pendingApprovals: 1,
      })
    } finally {
      upstream.restore()
    }
  })

  test('refuses a decision from a guest, a second decision, and a late one', async ({
    client,
    assert,
  }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const request = await waitingRequest(client, admin.id)
      const post = (decision: string) =>
        client
          .post(`/approvals/${request.publicId}`)
          .loginAs(admin)
          .withCsrfToken()
          .redirects(0)
          .form({ decision })

      const guest = await client
        .post(`/approvals/${request.publicId}`)
        .withCsrfToken()
        .redirects(0)
        .form({ decision: 'approve' })
      assert.equal(guest.header('location'), '/login')
      await request.refresh()
      assert.equal(request.status, 'pending')

      const unknown = await post('maybe')
      assert.property(unknown.flashMessage('inputErrorsBag'), 'decision')
      await request.refresh()
      assert.equal(request.status, 'pending')

      const denied = await post('deny')
      denied.assertFlashMessage('success', 'Denied. The agent is told the call was refused.')
      const changed = await post('approve')
      changed.assertFlashMessage('error', 'This request has expired or was already decided')
      await request.refresh()
      assert.equal(request.status, 'denied')

      const late = await waitingRequestFor(client, admin.id, 43)
      late.expiresAt = DateTime.utc().minus({ minutes: 1 })
      await late.save()
      const expired = await client
        .post(`/approvals/${late.publicId}`)
        .loginAs(admin)
        .withCsrfToken()
        .redirects(0)
        .form({ decision: 'approve' })
      expired.assertFlashMessage('error', 'This request has expired or was already decided')
      await late.refresh()
      assert.equal(late.status, 'pending')
    } finally {
      upstream.restore()
    }
  })

  /** Another request on the MCP `waitingRequest` created. */
  async function waitingRequestFor(
    client: Parameters<typeof gatewayRpc>[0],
    adminId: number,
    id: number
  ) {
    const { plaintext } = await createAccessToken(adminId)
    await gatewayRpc(client, plaintext, 'tools/call', {
      name: 'crm__delete_contact',
      arguments: { id },
    })
    return ApprovalRequest.query().orderBy('id', 'desc').firstOrFail()
  }

  test('lists what waits apart from what was decided', async ({ client }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const first = await waitingRequest(client, admin.id)
      const second = await waitingRequestFor(client, admin.id, 43)
      await ApprovalService.decide(first, 'deny', admin)

      const response = await client.get('/approvals').loginAs(admin).withInertia()

      response.assertInertiaComponent('approvals/index')
      response.assertInertiaPropsContains({
        waiting: [{ id: second.publicId, state: 'pending' }],
        past: [{ id: first.publicId, state: 'denied' }],
      })
    } finally {
      upstream.restore()
    }
  })

  test('answers an unknown or malformed link with the list', async ({ client, assert }) => {
    const admin = await createAdmin()

    for (const id of ['A'.repeat(32), 'short', '..%2Fsettings']) {
      const response = await client.get(`/approvals/${id}`).loginAs(admin).redirects(0)
      response.assertStatus(302)
      assert.equal(response.header('location'), '/approvals')
    }
  })
})

test.group('Tool approvals: choosing the tools that ask', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('lists the tools of a connected MCP with what is saved for them', async ({ client }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const mcp = await crmMcp(admin.id)

      const response = await client.get(`/mcps/${mcp.id}/tools`).loginAs(admin).withInertia()

      response.assertInertiaComponent('mcps/tools')
      response.assertInertiaPropsContains({
        mcp: { id: mcp.id, name: 'CRM', slug: 'crm', isBuiltin: false },
        tools: [
          {
            name: 'list_contacts',
            description: 'List the contacts of the account.',
            mode: 'auto',
            defaultMode: 'auto',
            isListed: true,
          },
          {
            name: 'delete_contact',
            description: 'Delete a contact for good.',
            mode: 'ask',
            defaultMode: 'auto',
            isListed: true,
          },
        ],
        listError: null,
        savedUnreadable: false,
      })
    } finally {
      upstream.restore()
    }
  })

  test('saves the choices that differ from the defaults', async ({ client, assert }) => {
    const upstream = mockHttpMcp(ORIGIN, TOOLS)
    try {
      const admin = await createAdmin()
      const mcp = await crmMcp(admin.id)

      const response = await client
        .put(`/mcps/${mcp.id}/tools`)
        .loginAs(admin)
        .withCsrfToken()
        .redirects(0)
        .json({
          tools: [
            { name: 'list_contacts', mode: 'ask' },
            { name: 'delete_contact', mode: 'auto' },
          ],
        })

      response.assertStatus(302)
      response.assertFlashMessage('success', 'Tool approvals saved')
      await mcp.refresh()
      assert.deepEqual(JSON.parse(mcp.toolApprovals!), { list_contacts: 'ask' })

      const unknown = await client
        .put(`/mcps/${mcp.id}/tools`)
        .loginAs(admin)
        .withCsrfToken()
        .redirects(0)
        .json({ tools: [{ name: 'list_contacts', mode: 'never' }] })
      assert.deepEqual(Object.keys(unknown.flashMessage('inputErrorsBag')), ['tools.0.mode'])
      await mcp.refresh()
      assert.deepEqual(JSON.parse(mcp.toolApprovals!), { list_contacts: 'ask' })
    } finally {
      upstream.restore()
    }
  })

  test('keeps the saved choices in view when the MCP cannot be reached', async ({ client }) => {
    const admin = await createAdmin()
    const mcp = await crmMcp(admin.id)
    const originalFetch = globalThis.fetch
    globalThis.fetch = async () => {
      throw new Error('connect ECONNREFUSED')
    }
    try {
      const response = await client.get(`/mcps/${mcp.id}/tools`).loginAs(admin).withInertia()

      response.assertInertiaPropsContains({
        tools: [{ name: 'delete_contact', description: null, mode: 'ask', isListed: false }],
      })
    } finally {
      globalThis.fetch = originalFetch
    }
  })

  test('lists the tools of a built-in MCP before its account is connected', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const mcp = await createStravaMcp(admin.id, { connected: false })

    const response = await client.get(`/mcps/${mcp.id}/tools`).loginAs(admin).withInertia()

    const { tools } = response.inertiaProps as { tools: Array<{ name: string; mode: string }> }
    assert.lengthOf(tools, 21)
    assert.includeMembers(
      tools.map((tool) => tool.name),
      ['get_athlete', 'update_athlete_weight']
    )
    assert.isTrue(tools.every((tool) => tool.mode === 'auto'))
  })
})
