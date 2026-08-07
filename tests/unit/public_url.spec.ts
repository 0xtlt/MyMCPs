import { test } from '@japa/runner'
import { normalizePublicAppUrl, validatePublicAppUrl } from '#services/public_url'

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

  test('requires HTTPS except for explicitly allowed loopback origins', ({ assert }) => {
    assert.equal(validatePublicAppUrl('https://mcp.example.com', false), 'https://mcp.example.com')
    assert.equal(validatePublicAppUrl('http://localhost:3333', true), 'http://localhost:3333')
    assert.throws(() => validatePublicAppUrl('http://mcp.example.com', false), /HTTPS origin/)
    assert.throws(() => validatePublicAppUrl('http://localhost:3333', false), /HTTPS origin/)
    assert.throws(() => validatePublicAppUrl('https://mcp.example.com/base', false), /HTTPS origin/)
  })
})
