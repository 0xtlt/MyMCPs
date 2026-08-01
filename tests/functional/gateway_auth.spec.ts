import { test } from '@japa/runner'
import {
  beginTestTransaction,
  rollbackTestTransaction,
} from '#tests/helpers/database'

test.group('gateway bearer authentication', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('rejects requests without a bearer token', async ({ client }) => {
    const response = await client.get('/mcp')

    response.assertStatus(401)
    response.assertBody({
      error: 'unauthorized',
      message: 'Missing Bearer access token',
    })
  })

  test('rejects an empty bearer token', async ({ client }) => {
    const response = await client.get('/mcp').header('authorization', 'Bearer ')

    response.assertStatus(401)
    response.assertBody({
      error: 'unauthorized',
      message: 'Empty Bearer access token',
    })
  })

  test('rejects invalid, expired, and revoked token values', async ({ client, assert }) => {
    const values = ['not-a-token', 'mcp_expired', 'mcp_revoked']

    for (const value of values) {
      const response = await client.get('/mcp').bearerToken(value)

      response.assertStatus(401)
      assert.equal(response.body().message, 'Invalid, expired, or revoked access token')
    }
  })
})
