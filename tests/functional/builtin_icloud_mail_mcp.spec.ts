import { test } from '@japa/runner'
import type { ApiClient } from '@japa/api-client'
import Mcp from '#models/mcp'
import McpCallLog from '#models/mcp_call_log'
import McpCallLogService from '#services/mcp_call_log_service'
import { builtinFileUrl } from '#services/builtin/file_link'
import env from '#start/env'
import McpSecretStore from '#services/mcp_secret_store'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAccessToken, createAdmin } from '#tests/helpers/factories'
import {
  createIcloudMailMcp,
  ICLOUD_MAIL_ATTACHMENT,
  icloudMailSignIn,
  mockIcloudMail,
} from '#tests/helpers/icloud_mail'

type RpcResponse = {
  result?: {
    tools?: Array<{ name: string }>
    content?: Array<{ type: string; text: string }>
    isError?: boolean
  }
}

function parseRpcResponse(response: { text: () => string; body: () => unknown }): RpcResponse {
  const text = response.text().trim()
  const data = text
    .split('\n')
    .find((line) => line.startsWith('data:'))
    ?.slice(5)
    .trim()
  if (data) return JSON.parse(data) as RpcResponse
  return text ? (JSON.parse(text) as RpcResponse) : (response.body() as RpcResponse)
}

async function gatewayRpc(
  client: ApiClient,
  plaintext: string,
  method: string,
  params: Record<string, unknown>
) {
  const response = await client
    .post('/mcp')
    .bearerToken(plaintext)
    .header('accept', 'application/json, text/event-stream')
    .header('X-MyMCPs-Tool-Mode', 'eager')
    .json({ jsonrpc: '2.0', id: 1, method, params })
  response.assertStatus(200)
  return parseRpcResponse(response)
}

/**
 * Links name APP_URL, which need not be where the test server listens. The
 * signature covers the path and the query, so the same link works on both.
 */
function fetchLink(url: string | URL) {
  const served = new URL(url)
  served.host = `${env.get('HOST')}:${env.get('PORT')}`
  return fetch(served)
}

const icloudForm = {
  name: 'iCloud Mail',
  description: 'Personal mail',
  transport: 'builtin',
  builtinKey: 'icloud-mail',
  authType: 'auto',
  builtinUsername: icloudMailSignIn.username,
  builtinPassword: icloudMailSignIn.password,
  builtinPermissions: ['read'],
  enabled: 'on',
}

const SIGN_IN_REJECTED =
  'iCloud Mail rejected the sign-in. Check the iCloud Mail address, create a new app-specific password at account.apple.com, and save it in this MCP in MyMCPs.'

test.group('Built-in iCloud Mail MCP: setup', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('creates the MCP with an encrypted password and checks it by signing in', async ({
    client,
    assert,
  }) => {
    const icloud = mockIcloudMail()
    try {
      const admin = await createAdmin()
      const response = await client
        .post('/mcps')
        .loginAs(admin)
        .withCsrfToken()
        .redirects(0)
        .form(icloudForm)

      response.assertStatus(302)
      response.assertFlashMessage('success', 'MCP created')
      const mcp = await Mcp.findByOrFail('slug', 'icloud-mail')
      assert.equal(mcp.transport, 'builtin')
      assert.equal(mcp.builtinKey, 'icloud-mail')
      assert.equal(mcp.authType, 'auto')
      assert.equal(mcp.builtinUsername, 'thomas@icloud.com')
      assert.notInclude(mcp.builtinPassword!, icloudMailSignIn.password)
      assert.equal(McpSecretStore.decrypt(mcp.builtinPassword), icloudMailSignIn.password)
      assert.equal(mcp.builtinPermissions, 'read')
      assert.isNull(mcp.builtinAliases)
      assert.isFalse(Boolean(mcp.builtinWriteEnabled))
      assert.isNull(mcp.oauthClientId)
      assert.equal(mcp.status, 'ready')
      assert.isNull(mcp.lastError)
      assert.isFalse(Boolean(mcp.oauthRequired))
      assert.isUndefined(response.flashMessage('editingMcpId'))
      assert.deepEqual(icloud.signIns, [icloudMailSignIn])
    } finally {
      icloud.restore()
    }
  })

  test('asks for an address, an app-specific password, and at least one permission', async ({
    client,
    assert,
  }) => {
    const icloud = mockIcloudMail()
    try {
      const admin = await createAdmin()
      const post = (overrides: Record<string, unknown>) =>
        client
          .post('/mcps')
          .loginAs(admin)
          .withCsrfToken()
          .redirects(0)
          .form({ ...icloudForm, ...overrides })
      const passwordHint =
        'Enter an app-specific password, which looks like abcd-efgh-ijkl-mnop. Your Apple Account password does not work here.'

      const missing = await post({
        builtinUsername: '',
        builtinPassword: '',
        builtinPermissions: [],
      })
      assert.deepEqual(missing.flashMessage('inputErrorsBag'), {
        builtinUsername: ['Enter your iCloud Mail address, such as name@icloud.com'],
        builtinPassword: [passwordHint],
        builtinPermissions: ['Allow at least one permission'],
      })

      // The Apple Account password must never be stored.
      const accountPassword = await post({ builtinPassword: 'Correct-Horse-Battery-9' })
      assert.deepEqual(accountPassword.flashMessage('inputErrorsBag'), {
        builtinPassword: [passwordHint],
      })

      const localPart = await post({ builtinUsername: 'thomas' })
      assert.property(localPart.flashMessage('inputErrorsBag'), 'builtinUsername')

      const unknown = await post({ builtinPermissions: ['read', 'admin'] })
      assert.deepEqual(unknown.flashMessage('inputErrorsBag'), {
        builtinPermissions: ['iCloud Mail has no "admin" permission'],
      })

      const alias = await post({ builtinAliases: 'hello@thomas.example, thomas' })
      assert.deepEqual(alias.flashMessage('inputErrorsBag'), {
        builtinAliases: [
          'Enter up to 20 other addresses of this iCloud account, such as alias@icloud.com, separated by commas',
        ],
      })

      assert.lengthOf(await Mcp.all(), 0)
      assert.lengthOf(icloud.signIns, 0)
    } finally {
      icloud.restore()
    }
  })

  test('reopens the dialog with the reason when iCloud rejects the sign-in', async ({
    client,
    assert,
  }) => {
    const icloud = mockIcloudMail({ rejectSignIn: true })
    try {
      const admin = await createAdmin()
      const response = await client
        .post('/mcps')
        .loginAs(admin)
        .withCsrfToken()
        .redirects(0)
        .form(icloudForm)

      const mcp = await Mcp.findByOrFail('slug', 'icloud-mail')
      assert.equal(mcp.status, 'error')
      assert.equal(mcp.lastError, SIGN_IN_REJECTED)
      // Connect cannot repair a password, so the dialog must not offer it.
      assert.isFalse(Boolean(mcp.oauthRequired))
      assert.equal(response.flashMessage('editingMcpId'), mcp.id)
    } finally {
      icloud.restore()
    }
  })

  test('shares the address and permissions, and never the password, with the page', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const mcp = await createIcloudMailMcp(admin.id, {
      permissions: ['read', 'send'],
      aliases: ['hello@thomas.example'],
    })

    const response = await client.get('/mcps').loginAs(admin).withInertia()

    response.assertInertiaPropsContains({
      mcps: [
        {
          id: mcp.id,
          transport: 'builtin',
          builtinKey: 'icloud-mail',
          builtinUsername: 'thomas@icloud.com',
          hasBuiltinPassword: true,
          builtinPermissions: ['read', 'send'],
          builtinAliases: ['hello@thomas.example'],
          builtinWriteGranted: true,
          hasOauthAccessToken: false,
          oauthRequired: false,
        },
      ],
    })
    assert.notInclude(response.text(), icloudMailSignIn.password)
    assert.notInclude(response.text(), mcp.builtinPassword!)
  })

  test('keeps the saved password when none is entered and applies new permissions', async ({
    client,
    assert,
  }) => {
    const icloud = mockIcloudMail()
    try {
      const admin = await createAdmin()
      const mcp = await createIcloudMailMcp(admin.id)
      const put = (overrides: Record<string, unknown>) =>
        client
          .put(`/mcps/${mcp.id}`)
          .loginAs(admin)
          .withCsrfToken()
          .redirects(0)
          .form({ ...icloudForm, builtinPassword: '', ...overrides })

      const widened = await put({ builtinPermissions: ['organize', 'read'] })
      widened.assertFlashMessage('success', 'MCP updated')
      const kept = await Mcp.findOrFail(mcp.id)
      assert.equal(McpSecretStore.decrypt(kept.builtinPassword), icloudMailSignIn.password)
      assert.equal(kept.builtinPermissions, 'read organize')
      assert.equal(kept.status, 'ready')

      await put({ builtinPassword: 'zyxwvutsrqponmlk', builtinPermissions: ['draft'] })
      const replaced = await Mcp.findOrFail(mcp.id)
      assert.equal(McpSecretStore.decrypt(replaced.builtinPassword), 'zyxwvutsrqponmlk')
      assert.equal(replaced.builtinPermissions, 'draft')
      assert.equal(icloud.signIns.at(-1)!.password, 'zyxwvutsrqponmlk')

      // The account's own address is not an alias, and each alias is kept once.
      await put({
        builtinPermissions: ['send'],
        builtinAliases:
          'hello@thomas.example; THOMAS@icloud.com\ntt@icloud.com, Hello@Thomas.example',
      })
      const aliased = await Mcp.findOrFail(mcp.id)
      assert.equal(aliased.builtinAliases, 'hello@thomas.example tt@icloud.com')
      await put({ builtinPermissions: ['draft'], builtinAliases: '' })

      const emptied = await put({ builtinPermissions: [] })
      assert.property(emptied.flashMessage('inputErrorsBag'), 'builtinPermissions')
      const unchanged = await Mcp.findOrFail(mcp.id)
      assert.equal(unchanged.builtinPermissions, 'draft')
      assert.isNull(unchanged.builtinAliases)
    } finally {
      icloud.restore()
    }
  })

  test('has no OAuth flow and forgets the sign-in when the MCP changes kind', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    const mcp = await createIcloudMailMcp(admin.id)

    const start = await client.get(`/mcps/${mcp.id}/oauth/start`).loginAs(admin).redirects(0)
    start.assertStatus(302)
    start.assertFlashMessage('error', 'This MCP does not require OAuth authorization')

    await client.put(`/mcps/${mcp.id}`).loginAs(admin).withCsrfToken().redirects(0).form({
      name: 'iCloud Mail',
      transport: 'http',
      httpUrl: 'http://127.0.0.1:9/mcp',
      authType: 'auto',
      enabled: 'on',
    })
    const changed = await Mcp.findOrFail(mcp.id)
    assert.equal(changed.transport, 'http')
    assert.isNull(changed.builtinKey)
    assert.isNull(changed.builtinUsername)
    assert.isNull(changed.builtinPassword)
    assert.isNull(changed.builtinPermissions)
    assert.isNull(changed.builtinAliases)
  })
})

test.group('Built-in iCloud Mail MCP: gateway', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('exposes the allowed tools through the gateway and refuses the others', async ({
    client,
    assert,
  }) => {
    const icloud = mockIcloudMail()
    try {
      const admin = await createAdmin()
      const mcp = await createIcloudMailMcp(admin.id, { permissions: ['read', 'draft'] })
      const { plaintext } = await createAccessToken(admin.id)

      const listed = await gatewayRpc(client, plaintext, 'tools/list', {})
      assert.deepEqual(
        listed.result!.tools!.map((tool) => tool.name),
        [
          'icloud-mail__list_mailboxes',
          'icloud-mail__list_messages',
          'icloud-mail__get_message',
          'icloud-mail__get_attachment_link',
          'icloud-mail__create_upload_link',
          'icloud-mail__create_draft',
        ]
      )
      assert.lengthOf(icloud.signIns, 0)

      const called = await gatewayRpc(client, plaintext, 'tools/call', {
        name: 'icloud-mail__list_messages',
        arguments: { unread: true },
      })
      assert.isUndefined(called.result!.isError)
      assert.equal(
        JSON.parse(called.result!.content![0].text).messages[0].subject,
        'Lunch on Thursday?'
      )

      const refused = await gatewayRpc(client, plaintext, 'tools/call', {
        name: 'icloud-mail__send_message',
        arguments: { to: ['dave@example.com'], subject: 'Hello', text: 'Hi' },
      })
      assert.isTrue(refused.result!.isError)
      assert.include(refused.result!.content![0].text, 'send_message needs the "send" permission')
      assert.lengthOf(icloud.sent, 0)

      await McpCallLogService.flush()
      const logs = await McpCallLog.query().where('mcp_id', mcp.id).orderBy('id', 'asc')
      assert.deepEqual(
        logs.map((log) => [log.toolName, log.outcome, log.errorCategory]),
        [
          ['list_messages', 'success', null],
          ['send_message', 'error', 'tool_error'],
        ]
      )
    } finally {
      icloud.restore()
    }
  })

  test('reports a revoked password to the agent without leaking it', async ({ client, assert }) => {
    const icloud = mockIcloudMail({ rejectSignIn: true })
    try {
      const admin = await createAdmin()
      await createIcloudMailMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)

      const called = await gatewayRpc(client, plaintext, 'tools/call', {
        name: 'icloud-mail__list_mailboxes',
        arguments: {},
      })
      assert.isTrue(called.result!.isError)
      assert.equal(called.result!.content![0].text, SIGN_IN_REJECTED)
      assert.notInclude(JSON.stringify(called), icloudMailSignIn.password)
    } finally {
      icloud.restore()
    }
  })
})

test.group('Built-in iCloud Mail MCP: attachment links', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  const attachment = { mailbox: 'INBOX', uid: 11, part: '2' }

  async function requestLink(client: ApiClient, permissions = ['read']) {
    const admin = await createAdmin()
    const mcp = await createIcloudMailMcp(admin.id, { permissions })
    const { plaintext } = await createAccessToken(admin.id)
    const called = await gatewayRpc(client, plaintext, 'tools/call', {
      name: 'icloud-mail__get_attachment_link',
      arguments: { uid: 11, part: 2 },
    })
    const [{ text }] = called.result!.content!
    return { mcp, called, text, link: called.result!.isError ? null : JSON.parse(text) }
  }

  test('hands out a temporary link that downloads the attachment without signing in', async ({
    client,
    assert,
  }) => {
    const icloud = mockIcloudMail()
    try {
      const { mcp, called, link } = await requestLink(client)

      assert.isUndefined(called.result!.isError)
      assert.deepEqual(
        { ...link, url: undefined, expires_at: undefined },
        {
          url: undefined,
          expires_at: undefined,
          filename: 'Menu été.pdf',
          content_type: 'application/pdf',
          size: 57000,
        }
      )
      const expiresIn = new Date(link.expires_at).getTime() - Date.now()
      assert.isAbove(expiresIn, 14 * 60_000)
      assert.isAtMost(expiresIn, 15 * 60_000)

      const url = new URL(link.url)
      assert.equal(url.origin, 'http://localhost:3333')
      assert.match(url.pathname, new RegExp(`^/files/${mcp.id}/[\\w-]+$`))
      assert.deepEqual([...url.searchParams.keys()], ['signature'])
      // The link tells an onlooker nothing about the mailbox.
      assert.notInclude(link.url, 'INBOX')
      assert.notInclude(link.url, icloudMailSignIn.username)

      // No cookie, no access token: the signature is the credential.
      const download = await fetchLink(link.url)
      assert.equal(download.status, 200)
      assert.equal(Buffer.from(await download.arrayBuffer()).toString(), ICLOUD_MAIL_ATTACHMENT)
      assert.equal(download.headers.get('content-type'), 'application/pdf')
      assert.equal(
        download.headers.get('content-disposition'),
        `attachment; filename="Menu _t_.pdf"; filename*=UTF-8''Menu%20%C3%A9t%C3%A9.pdf`
      )
      assert.equal(download.headers.get('x-content-type-options'), 'nosniff')
      assert.equal(download.headers.get('content-security-policy'), "sandbox; default-src 'none'")
      assert.equal(download.headers.get('cache-control'), 'private, no-store')

      assert.deepEqual(icloud.downloads.at(-1), { uid: 11, part: '2', maxBytes: 30_000_001 })
      assert.deepEqual(icloud.locks.at(-1), { path: 'INBOX', readOnly: true })
    } finally {
      icloud.restore()
    }
  })

  test('refuses a link that was changed, has expired, or was signed for something else', async ({
    client,
    assert,
  }) => {
    const icloud = mockIcloudMail()
    try {
      const { mcp, link } = await requestLink(client)
      const status = async (url: string | URL) => {
        const response = await fetchLink(url)
        return [response.status, await response.text()]
      }
      const refused = [403, 'This link is invalid or has expired.']
      const sessions = icloud.signIns.length

      const otherMessage = new URL(builtinFileUrl(mcp.id, { ...attachment, uid: 14 }, 60_000))
      const swapped = new URL(link.url)
      swapped.pathname = otherMessage.pathname
      assert.deepEqual(await status(swapped), refused)

      const otherMcp = new URL(link.url)
      otherMcp.pathname = otherMcp.pathname.replace(`/files/${mcp.id}/`, `/files/${mcp.id + 1}/`)
      assert.deepEqual(await status(otherMcp), refused)

      const unsigned = new URL(link.url)
      unsigned.search = ''
      assert.deepEqual(await status(unsigned), refused)

      const extra = new URL(link.url)
      extra.searchParams.set('download', '1')
      assert.deepEqual(await status(extra), refused)

      const expired = builtinFileUrl(mcp.id, attachment, 1)
      await new Promise((resolve) => setTimeout(resolve, 20))
      assert.deepEqual(await status(expired), refused)

      // Nothing above reached iCloud, and the untouched link still works.
      assert.lengthOf(icloud.signIns, sessions)
      const untouched = await fetchLink(link.url)
      assert.equal(untouched.status, 200)
    } finally {
      icloud.restore()
    }
  })

  test('only links to attachments, and only within the allowed time', async ({
    client,
    assert,
  }) => {
    const icloud = mockIcloudMail()
    try {
      const admin = await createAdmin()
      const mcp = await createIcloudMailMcp(admin.id)
      const { plaintext } = await createAccessToken(admin.id)
      const call = async (args: Record<string, unknown>) => {
        const called = await gatewayRpc(client, plaintext, 'tools/call', {
          name: 'icloud-mail__get_attachment_link',
          arguments: args,
        })
        return [called.result!.isError, called.result!.content![0].text]
      }

      // The message text is not a file to hand out.
      assert.deepEqual(await call({ uid: 11, part: '1.1' }), [
        true,
        'Message 11 has no attachment at part "1.1". Its attachments are at parts: 2.',
      ])
      assert.deepEqual(await call({ uid: 12, part: '1' }), [true, 'Message 12 has no attachments.'])
      assert.deepEqual(await call({ uid: 11, part: '../2' }), [
        true,
        'part must be the part of an attachment, such as 2, as returned by get_message',
      ])
      assert.deepEqual(await call({ uid: 11, part: '2', expires_in_minutes: 600 }), [
        true,
        'expires_in_minutes must be an integer between 1 and 60',
      ])

      // A signed link to a part that is not an attachment serves nothing either.
      const body = await fetchLink(builtinFileUrl(mcp.id, { ...attachment, part: '1.1' }, 60_000))
      assert.deepEqual([body.status, await body.text()], [404, 'This file is no longer available.'])
      const garbage = await fetchLink(builtinFileUrl(mcp.id, 'INBOX', 60_000))
      assert.equal(garbage.status, 404)
    } finally {
      icloud.restore()
    }
  })

  test('stops serving a link once the MCP no longer allows it', async ({ client, assert }) => {
    const icloud = mockIcloudMail()
    try {
      const { mcp, link } = await requestLink(client)
      const unavailable = [404, 'This file is no longer available.']
      const status = async () => {
        const response = await fetchLink(link.url)
        return response.ok ? [response.status] : [response.status, await response.text()]
      }
      assert.deepEqual(await status(), [200])

      mcp.builtinPermissions = 'send'
      await mcp.save()
      assert.deepEqual(await status(), unavailable)

      mcp.builtinPermissions = 'read'
      mcp.enabled = false
      await mcp.save()
      assert.deepEqual(await status(), unavailable)

      mcp.enabled = true
      await mcp.save()
      assert.deepEqual(await status(), [200])

      await mcp.delete()
      assert.deepEqual(await status(), unavailable)
    } finally {
      icloud.restore()
    }
  })

  test('needs the read permission and the public address of the instance', async ({
    client,
    assert,
  }) => {
    const icloud = mockIcloudMail()
    const appUrl = env.get('APP_URL')
    try {
      const sendOnly = await requestLink(client, ['send'])
      assert.isTrue(sendOnly.called.result!.isError)
      assert.include(sendOnly.text, 'get_attachment_link needs the "read" permission')

      env.set('APP_URL', undefined)
      const { plaintext } = await createAccessToken(sendOnly.mcp.createdBy)
      sendOnly.mcp.builtinPermissions = 'read'
      await sendOnly.mcp.save()
      const unreachable = await gatewayRpc(client, plaintext, 'tools/call', {
        name: 'icloud-mail__get_attachment_link',
        arguments: { uid: 11, part: '2' },
      })
      assert.isTrue(unreachable.result!.isError)
      assert.equal(
        unreachable.result!.content![0].text,
        'File links need the public address of this MyMCPs instance. An administrator must set APP_URL.'
      )
      assert.lengthOf(icloud.signIns, 0)
    } finally {
      env.set('APP_URL', appUrl)
      icloud.restore()
    }
  })
})
