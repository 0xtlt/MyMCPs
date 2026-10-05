import { test } from '@japa/runner'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

test.group('hardening: Inertia page embedding', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('a full page load still mounts when a stored string looks like script markup', async ({
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    const name = 'Hostile <!--<script> name'
    await createMcp(admin.id, { name, slug: 'hostile' })

    await browserContext.loginAs(admin)
    const page = await visit('/mcps')

    // The page stays blank when the browser cannot find the mount element.
    await page.getByRole('button', { name: 'Add MCP' }).waitFor()
    await page.getByText(name).first().waitFor()
  })
})
