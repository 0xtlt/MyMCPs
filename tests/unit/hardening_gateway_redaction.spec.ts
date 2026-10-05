import { test } from '@japa/runner'
import { sanitizeDiagnostic } from '#services/security_redaction'

const ONE_MEGABYTE = 1024 * 1024

/**
 * Shapes that keep a pattern scanning without ever completing a match.
 * `a.` and `a-` runs took minutes per megabyte before keys were anchored.
 */
const HOSTILE_UNITS = [
  'a.',
  'a-',
  'a ',
  '"',
  '"a',
  'Bearer ',
  'Bearer error ',
  '://a:',
  '://a:b',
  'token="',
  "token='x ",
  'token="a\\" ',
]

test.group('hardening: diagnostic redaction', () => {
  test('sanitizes a one megabyte hostile diagnostic without blocking the process', ({ assert }) => {
    for (const unit of HOSTILE_UNITS) {
      const hostile = unit.repeat(Math.ceil(ONE_MEGABYTE / unit.length))
      const startedAt = performance.now()
      const sanitized = sanitizeDiagnostic(new Error(hostile), 500, ['configured-secret'])
      const elapsed = performance.now() - startedAt

      assert.isAtMost(sanitized!.length, 500)
      assert.isBelow(elapsed, 250, `sanitizing ${JSON.stringify(unit)} runs took too long`)
    }
  }).timeout(30_000)

  test('redacts a known secret wherever it appears before the text is cut', ({ assert }) => {
    const secret = 'opaque-value-without-a-credential-shape'
    const filler = 'x'.repeat(480)

    const sanitized = sanitizeDiagnostic(`${filler} ${secret} trailing text`, 500, [secret])!

    assert.include(sanitized, '[REDACTED]')
    assert.notInclude(sanitized, secret.slice(0, 8))
  })

  test('redacts credentials that the scan limit cut short', ({ assert }) => {
    // A long redacted value pulls the end of the scanned text into the result.
    const padding = `token=${'t'.repeat(8 * 1024 - 60)}`
    const overflow = 'y'.repeat(1024)

    const quoted = sanitizeDiagnostic(
      `${padding} "password":"correct horse battery staple and more words ${overflow}"`,
      500
    )!
    assert.notInclude(quoted, 'horse')
    assert.notInclude(quoted, 'battery')

    const url = sanitizeDiagnostic(
      `${padding} https://user:url-password-${'z'.repeat(60)}${overflow}@example.test/mcp`,
      500
    )!
    assert.notInclude(url, 'url-password')
    assert.include(url, 'https://[REDACTED]')
  })

  test('keeps host and port readable at the end of an untruncated diagnostic', ({ assert }) => {
    assert.equal(
      sanitizeDiagnostic('connect ECONNREFUSED http://127.0.0.1:9999'),
      'connect ECONNREFUSED http://127.0.0.1:9999'
    )
    assert.equal(
      sanitizeDiagnostic('Unexpected token: "abc def'),
      'Unexpected token: [REDACTED] def'
    )
  })
})
