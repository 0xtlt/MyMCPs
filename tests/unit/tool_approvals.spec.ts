import { test } from '@japa/runner'
import Mcp from '#models/mcp'
import { argumentsHash } from '#services/approvals/approval_service'
import {
  assignToolApprovals,
  defaultToolApproval,
  savedToolApprovals,
  toolApprovalMode,
} from '#services/approvals/policy'
import { argumentDetails, argumentsSummary } from '#services/approvals/summary'

function mcp(fields: Partial<Mcp> = {}) {
  return Object.assign(new Mcp(), { name: 'CRM', transport: 'http', ...fields })
}

test.group('Tool approvals: which tools ask', () => {
  test('lets every tool run until the admin says otherwise', async ({ assert }) => {
    assert.equal(await toolApprovalMode(mcp(), 'delete_contact'), 'auto')
    assert.equal(
      await toolApprovalMode(mcp({ toolApprovals: '{"delete_contact":"ask"}' }), 'delete_contact'),
      'ask'
    )
    assert.equal(
      await toolApprovalMode(mcp({ toolApprovals: '{"delete_contact":"ask"}' }), 'list_contacts'),
      'auto'
    )
  })

  test('asks by default for the built-in tools that commit money', async ({ assert }) => {
    const googleAds = mcp({ transport: 'builtin', builtinKey: 'google-ads' })

    assert.equal(defaultToolApproval(googleAds, 'update_campaign_budget'), 'ask')
    assert.equal(defaultToolApproval(googleAds, 'list_campaigns'), 'auto')
    assert.equal(await toolApprovalMode(googleAds, 'update_campaign_budget'), 'ask')

    googleAds.toolApprovals = '{"update_campaign_budget":"auto"}'
    assert.equal(await toolApprovalMode(googleAds, 'update_campaign_budget'), 'auto')
    // A tool the MCP does not have, or that is named like a property of every object.
    assert.equal(await toolApprovalMode(googleAds, 'constructor'), 'auto')
  })

  test('asks for every tool when what is saved cannot be read', async ({ assert }) => {
    for (const unreadable of ['{', '[]', '"ask"', '{"delete_contact":"sometimes"}']) {
      const broken = mcp({ toolApprovals: unreadable })
      assert.isNull(await savedToolApprovals(broken), unreadable)
      assert.equal(await toolApprovalMode(broken, 'list_contacts'), 'ask', unreadable)
    }
  })

  test('saves the choices that differ from the defaults, and nothing else', ({ assert }) => {
    const googleAds = mcp({ transport: 'builtin', builtinKey: 'google-ads' })
    assignToolApprovals(googleAds, [
      { name: 'update_campaign_budget', mode: 'ask' },
      { name: 'set_campaign_status', mode: 'auto' },
      { name: 'add_keywords', mode: 'ask' },
      { name: 'list_campaigns', mode: 'auto' },
    ])
    assert.deepEqual(JSON.parse(googleAds.toolApprovals!), {
      set_campaign_status: 'auto',
      add_keywords: 'ask',
    })

    assignToolApprovals(googleAds, [{ name: 'update_campaign_budget', mode: 'ask' }])
    assert.isNull(googleAds.toolApprovals)
  })
})

test.group('Tool approvals: reading a call', () => {
  test('identifies arguments whatever the order of their keys', ({ assert }) => {
    const hash = argumentsHash({ id: 42, filter: { tags: ['a', 'b'], dry_run: false } })

    assert.equal(hash, argumentsHash({ filter: { dry_run: false, tags: ['a', 'b'] }, id: 42 }))
    assert.equal(argumentsHash(undefined), argumentsHash({}))
    // Another value, another type, or another order in a list is another call.
    assert.notEqual(hash, argumentsHash({ id: 43, filter: { tags: ['a', 'b'], dry_run: false } }))
    assert.notEqual(hash, argumentsHash({ id: '42', filter: { tags: ['a', 'b'], dry_run: false } }))
    assert.notEqual(hash, argumentsHash({ id: 42, filter: { tags: ['b', 'a'], dry_run: false } }))
  })

  test('puts every value on a row named by where it is', ({ assert }) => {
    const { details, hidden } = argumentDetails({
      amount: 250,
      note: '',
      nothing: null,
      tags: [],
      options: {},
      items: [{ name: 'a' }, { name: 'b', flags: [true] }],
    })

    assert.equal(hidden, 0)
    assert.deepEqual(details, [
      { label: 'amount', value: '250' },
      { label: 'note', value: '(empty text)' },
      { label: 'nothing', value: 'null' },
      { label: 'tags', value: '(empty list)' },
      { label: 'options', value: '(empty object)' },
      { label: 'items[0].name', value: 'a' },
      { label: 'items[1].name', value: 'b' },
      { label: 'items[1].flags[0]', value: 'true' },
    ])
  })

  test('cuts what is too long to read, and says so', ({ assert }) => {
    const many = Object.fromEntries(Array.from({ length: 75 }, (_, index) => [`field${index}`, 1]))
    const summary = argumentsSummary(
      mcp(),
      'import',
      { ...many, body: 'x'.repeat(1500) },
      ' Imports. '
    )

    assert.equal(summary.title, 'Run the tool "import" of CRM')
    assert.isFalse(summary.interpreted)
    assert.equal(summary.toolDescription, 'Imports.')
    assert.lengthOf(summary.details, 60)
    assert.deepEqual(summary.warnings, [
      'Only the first 60 values are listed, and 16 more are not. Read the exact arguments before you decide.',
    ])

    const [long] = argumentDetails({ body: 'x'.repeat(1500) }).details
    assert.equal(long.value, `${'x'.repeat(1000)}… (500 more characters)`)
  })
})
