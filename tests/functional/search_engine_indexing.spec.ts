import { test } from '@japa/runner'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'

test.group('search engine indexing', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('blocks all crawlers through robots.txt', async ({ client }) => {
    const response = await client.get('/robots.txt')

    response.assertStatus(200)
    response.assertTextIncludes('User-agent: *')
    response.assertTextIncludes('Disallow: /')
  })

  test('marks every rendered page as noindex', async ({ client }) => {
    const response = await client.get('/onboarding')

    response.assertStatus(200)
    response.assertTextIncludes(
      '<meta name="robots" content="noindex, nofollow, noarchive, nosnippet" />'
    )
    response.assertTextIncludes(
      '<meta name="googlebot" content="noindex, nofollow, noarchive, nosnippet" />'
    )
  })
})
