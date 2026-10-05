import { test } from '@japa/runner'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'

const PLAINTEXT = 'mcp_history-plaintext-marker'

type BrowserGlobals = {
  history: { state: { page?: unknown } | null }
  sessionStorage: { getItem(key: string): string | null }
}

/**
 * What Inertia stored for the current history entry: whether it is encrypted,
 * everything a script could read from it, and the key that decrypts it.
 */
function historyEntry() {
  const browser = globalThis as unknown as BrowserGlobals
  const page = browser.history.state?.page

  return {
    isEncrypted: page instanceof ArrayBuffer,
    readable: JSON.stringify(page) ?? '',
    key: browser.sessionStorage.getItem('historyKey'),
  }
}

function hasHistoryEntry() {
  return Boolean((globalThis as unknown as BrowserGlobals).history.state?.page)
}

test.group('hardening: browser history', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('keeps a one-time access token unreadable in the history, and gone after logout', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    await browserContext.loginAs(admin)
    await browserContext.setFlashMessages({ createdPlaintext: PLAINTEXT })

    const page = await visit('/tokens')
    await page.getByRole('button', { name: PLAINTEXT }).waitFor()
    await page.waitForFunction(hasHistoryEntry)

    const tokensEntry = await page.evaluate(historyEntry)
    assert.isTrue(tokensEntry.isEncrypted)
    assert.notInclude(tokensEntry.readable, PLAINTEXT)
    assert.isString(tokensEntry.key)

    // A new token opens the install dialog over the page.
    const installDialog = page.getByRole('dialog', { name: 'Install MyMCPs' })
    await installDialog.waitFor()
    await page.keyboard.press('Escape')
    await installDialog.waitFor({ state: 'hidden' })

    await page.getByRole('button', { name: 'Log out' }).click()
    await page.waitForURL((url) => url.pathname === '/login')
    await page.getByLabel('Email').waitFor()
    await page.waitForFunction(hasHistoryEntry)
    const loginEntry = await page.evaluate(historyEntry)
    assert.notEqual(loginEntry.key, tokensEntry.key)

    // Without its key the tokens entry cannot be restored: going back asks the server instead.
    const [reload] = await Promise.all([
      page.waitForResponse((response) => new URL(response.url()).pathname === '/tokens'),
      page.goBack(),
    ])
    assert.equal(reload.status(), 302)
    await page.getByText('Unauthorized access').waitFor()
    await page.getByLabel('Email').waitFor()
    assert.equal(await page.getByText(PLAINTEXT).count(), 0)
  })
})
