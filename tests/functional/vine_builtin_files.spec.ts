import { test } from '@japa/runner'
import { signedUrlFor } from '@adonisjs/core/services/url_builder'
import limiter from '@adonisjs/limiter/services/main'
import { BUILTIN_FILE_PURPOSE, builtinFileUrl } from '#services/builtin/file_link'
import env from '#start/env'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'
import {
  createIcloudMailMcp,
  ICLOUD_MAIL_ATTACHMENT,
  mockIcloudMail,
} from '#tests/helpers/icloud_mail'
import { builtinFileValidator } from '#validators/builtin_files'

const attachment = { mailbox: 'INBOX', uid: 11, part: '2' }

function encoded(reference: unknown) {
  return Buffer.from(JSON.stringify(reference)).toString('base64url')
}

/** A link with our signature, to parameters `builtinFileUrl` would never write. */
function signedLink(id: string | number, reference: string) {
  return signedUrlFor(
    'builtin.file',
    { id, reference },
    { expiresIn: 60_000, purpose: BUILTIN_FILE_PURPOSE, prefixUrl: env.get('APP_URL') }
  )
}

/**
 * Links name APP_URL, which need not be where the test server listens. The
 * signature covers the path and the query, so the same link works on both.
 */
function fetchLink(url: string) {
  const served = new URL(url)
  served.host = `${env.get('HOST')}:${env.get('PORT')}`
  return fetch(served, { signal: AbortSignal.timeout(10_000) })
}

test.group('Built-in file links: validator', () => {
  test('reads the MCP and what the tool put in the link', async ({ assert }) => {
    assert.deepEqual(
      await builtinFileValidator.validate({
        params: { id: '12', reference: encoded(attachment) },
        signature: 'ignored',
      }),
      { params: { id: 12, reference: attachment } }
    )
    // A reference is whatever the provider put there: its content is not checked here.
    assert.deepEqual(
      await builtinFileValidator.validate({ params: { id: '1', reference: encoded(['x', 1]) } }),
      { params: { id: 1, reference: ['x', 1] } }
    )
  })

  test('refuses an MCP that is not a positive whole number', async ({ assert }) => {
    for (const id of ['abc', '0', '-1', '1.5', '12abc', undefined]) {
      const [malformed] = await builtinFileValidator.tryValidate({
        params: { id, reference: encoded(attachment) },
      })
      assert.isNotNull(malformed, String(id))
    }
  })

  test('refuses a reference that does not decode', async ({ assert }) => {
    const references = ['not-a-reference', encoded(attachment).slice(0, -3), '%%%', undefined]
    for (const reference of references) {
      const [malformed] = await builtinFileValidator.tryValidate({ params: { id: '1', reference } })
      assert.isNotNull(malformed, String(reference))
    }
    const [missing] = await builtinFileValidator.tryValidate({})
    assert.isNotNull(missing)
  })
})

test.group('Built-in file links: validated parameters', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(() => limiter.clear(['memory']))
  group.each.teardown(rollbackTestTransaction)

  test('answers a signed link to malformed parameters like a file that is gone', async ({
    assert,
  }) => {
    const icloud = mockIcloudMail()
    try {
      const admin = await createAdmin()
      const mcp = await createIcloudMailMcp(admin.id)

      for (const link of [
        signedLink(mcp.id, 'not-a-reference'),
        signedLink(mcp.id, encoded({ mailbox: 'INBOX', uid: 'x', part: '2' })),
        signedLink(mcp.id, encoded('INBOX/11/2')),
        signedLink('abc', encoded(attachment)),
        signedLink(`${mcp.id}.5`, encoded(attachment)),
      ]) {
        const response = await fetchLink(link)
        assert.equal(response.status, 404, link)
        assert.equal(await response.text(), 'This file is no longer available.')
        assert.equal(response.headers.get('content-type'), 'text/plain; charset=utf-8')
      }
      // None of them reached the account.
      assert.lengthOf(icloud.signIns, 0)

      const download = await fetchLink(builtinFileUrl(mcp.id, attachment, 60_000))
      assert.equal(download.status, 200)
      assert.equal(Buffer.from(await download.arrayBuffer()).toString(), ICLOUD_MAIL_ATTACHMENT)
    } finally {
      icloud.restore()
    }
  })

  test('still answers a link without our signature first, whatever its parameters', async ({
    assert,
  }) => {
    const unsigned = new URL(signedLink('abc', 'not-a-reference'))
    unsigned.searchParams.set('signature', 'forged')

    const response = await fetchLink(unsigned.toString())
    assert.equal(response.status, 403)
    assert.equal(await response.text(), 'This link is invalid or has expired.')
  })
})
