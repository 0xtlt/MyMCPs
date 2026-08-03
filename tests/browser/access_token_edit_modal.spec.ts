import { test } from '@japa/runner'
import { DateTime } from 'luxon'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAccessToken, createAdmin, createMcp } from '#tests/helpers/factories'

test.group('access token edit modal', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('edits active and expired tokens while keeping revoked tokens immutable', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    await createMcp(admin.id, { name: 'Search MCP' })
    await createAccessToken(admin.id, {
      name: 'Editable token',
      expiresAt: DateTime.utc().plus({ days: 1 }).startOf('minute'),
    })
    await createAccessToken(admin.id, {
      name: 'Expired token',
      expiresAt: DateTime.utc().minus({ days: 1 }),
    })
    const revoked = await createAccessToken(admin.id, { name: 'Revoked token' })
    revoked.token.revokedAt = DateTime.utc()
    await revoked.token.save()

    await browserContext.loginAs(admin)
    const page = await visit('/tokens')

    const editableRow = page.getByRole('row').filter({ hasText: 'Editable token' })
    await editableRow.getByRole('button', { name: 'Edit' }).click()

    const dialog = page.getByRole('dialog', { name: 'Edit Editable token' })
    assert.equal(await dialog.getByLabel('Name').inputValue(), 'Editable token')
    assert.isTrue(await dialog.getByRole('radio', { name: 'All MCPs' }).isChecked())
    assert.isNotEmpty(await dialog.getByRole('combobox').inputValue())

    await dialog.getByLabel('Name').fill('Browser edited token')
    await dialog.getByRole('radio', { name: 'Selected MCPs' }).click()
    await dialog.getByRole('checkbox', { name: /Search MCP/ }).click()
    await dialog.getByRole('button', { name: 'Save changes' }).click()
    await dialog.waitFor({ state: 'hidden' })

    const updatedRow = page.getByRole('row').filter({ hasText: 'Browser edited token' })
    await updatedRow.getByText('1 MCP').waitFor()

    const expiredRow = page.getByRole('row').filter({ hasText: 'Expired token' })
    assert.equal(await expiredRow.getByRole('button', { name: 'Edit' }).count(), 1)
    assert.equal(await expiredRow.getByRole('button', { name: 'Revoke' }).count(), 0)

    const revokedRow = page.getByRole('row').filter({ hasText: 'Revoked token' })
    assert.equal(await revokedRow.getByRole('button', { name: 'Edit' }).count(), 0)

    await expiredRow.getByRole('button', { name: 'Edit' }).click()
    const expiredDialog = page.getByRole('dialog', { name: 'Edit Expired token' })
    await expiredDialog.getByLabel('Name').fill('')
    await expiredDialog.getByRole('button', { name: 'Save changes' }).click()
    await expiredDialog.getByText(/name field/i).waitFor()
    assert.isTrue(await expiredDialog.isVisible())
  })
})
