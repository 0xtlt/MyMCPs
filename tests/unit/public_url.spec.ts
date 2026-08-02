import { test } from '@japa/runner'
import { normalizePublicAppUrl } from '#services/public_url'

test.group('public app URL', () => {
  test('returns null when APP_URL is missing', ({ assert }) => {
    assert.isNull(normalizePublicAppUrl(undefined))
  })

  test('keeps a valid configured origin', ({ assert }) => {
    assert.equal(normalizePublicAppUrl('https://mcp.example.com'), 'https://mcp.example.com')
  })

  test('removes one trailing slash from the configured origin', ({ assert }) => {
    assert.equal(normalizePublicAppUrl('https://mcp.example.com/'), 'https://mcp.example.com')
  })
})
