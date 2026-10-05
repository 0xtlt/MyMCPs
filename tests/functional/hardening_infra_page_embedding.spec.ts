import { test } from '@japa/runner'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

/**
 * What the Inertia client looks up on a full page load: the page-data script,
 * immediately followed by the element it mounts into.
 */
const PAGE_DATA = /<script data-page="app" type="application\/json">([\s\S]*?)<\/script>([\s\S]*)/

test.group('hardening: Inertia page embedding', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('a stored string cannot break out of the page-data script on a full page load', async ({
    client,
    assert,
  }) => {
    const admin = await createAdmin()
    // "<!--<script" moves an HTML parser into the double-escaped script state,
    // where the next "</script>" no longer closes the element.
    const name = 'Hostile <!--<script> name'
    const description = 'closes </script> early & keeps U+2028 \u2028 and U+2029 \u2029 intact'
    const mcp = await createMcp(admin.id, { name, slug: 'hostile' })
    mcp.description = description
    await mcp.save()

    const response = await client.get('/mcps').loginAs(admin)

    response.assertStatus(200)
    assert.include(response.header('content-type'), 'text/html')

    const match = PAGE_DATA.exec(response.text())
    assert.isNotNull(match, 'expected the page-data script the Inertia client reads')
    const [, pageData, afterPageData] = match!

    // Without a raw "<" the tokenizer can only leave the script at its real end tag.
    assert.notMatch(pageData, /[<>&\u2028\u2029]/)
    assert.match(afterPageData, /^\s*<div id="app"><\/div>/)

    const page = JSON.parse(pageData)
    const embedded = (
      page.props.mcps as { slug: string; name: string; description: string }[]
    ).find((candidate) => candidate.slug === 'hostile')
    assert.equal(embedded?.name, name)
    assert.equal(embedded?.description, description)
  })
})
