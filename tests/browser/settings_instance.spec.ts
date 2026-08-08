import { test } from '@japa/runner'
import InstanceSetting from '#models/instance_setting'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'

test.group('instance settings browser flow', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('changes the default MCP tool mode from the Settings page', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    await browserContext.loginAs(admin)
    const page = await visit('/settings')

    const eager = page.getByRole('radio', { name: 'Eager' })
    const lazy = page.getByRole('radio', { name: 'Lazy' })
    assert.isTrue(await eager.isChecked())

    await lazy.click()
    await page.getByRole('button', { name: 'Save instance settings' }).click()
    await page.getByLabel('Notifications').getByText('Instance settings updated').waitFor()

    assert.isTrue(await lazy.isChecked())
    const settings = await InstanceSetting.current()
    assert.equal(settings.gatewayToolMode, 'lazy')
  })
})
