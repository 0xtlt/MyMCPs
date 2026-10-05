import { test } from '@japa/runner'
import { serializeInertiaPage } from '#providers/inertia_page_provider'

test.group('hardening: Inertia page serializer', () => {
  test('escapes markup characters and parses back to the same data', ({ assert }) => {
    const page = {
      'component': 'logs/index',
      'props': {
        tool: '<!--<script>alert(1)</script>-->',
        entity: '&lt;/script&gt; & more',
        separators: 'line\u2028paragraph\u2029end',
        // A backslash right before an escaped character must stay a backslash.
        backslash: 'C:\\<dir>\\',
        url: 'https://example.test/a/b',
      },
      '<key>': ['<', '>', '&'],
    }

    const serialized = serializeInertiaPage(page)

    assert.notMatch(serialized, /[<>&\u2028\u2029]/)
    assert.include(serialized, '\\u003c!--\\u003cscript\\u003e')
    assert.deepEqual(JSON.parse(serialized), page)
  })

  test('serializes a missing page as an empty object', ({ assert }) => {
    assert.equal(serializeInertiaPage(undefined), '{}')
  })
})
