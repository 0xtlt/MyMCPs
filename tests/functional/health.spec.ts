import { test } from '@japa/runner'

test.group('health checks', () => {
  test('returns liveness without authentication or setup', async ({ client }) => {
    const response = await client.get('/health')

    response.assertStatus(200)
    response.assertBody({ status: 'ok' })
  })
})
