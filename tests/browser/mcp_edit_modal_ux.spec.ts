import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from '@japa/runner'
import { mcpNpmUpdateRuntime, resetMcpNpmUpdateRuntime } from '#services/mcp_npm_update_service'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

function getOpacity(element: unknown) {
  const browser = globalThis as unknown as {
    getComputedStyle: (target: unknown) => { opacity: string }
  }
  return browser.getComputedStyle(element).opacity
}

function flashToastIsHitAtItsCenter() {
  const browser = globalThis as unknown as {
    document: {
      querySelector: (selector: string) => {
        getBoundingClientRect: () => { left: number; top: number; width: number; height: number }
        contains: (node: unknown) => boolean
      } | null
      elementFromPoint: (x: number, y: number) => unknown
    }
  }
  const toast = browser.document.querySelector('[data-flash-toast]')
  if (!toast) {
    return false
  }
  const rect = toast.getBoundingClientRect()
  if (rect.width <= 0 || rect.height <= 0) {
    return false
  }
  const topEl = browser.document.elementFromPoint(
    rect.left + rect.width / 2,
    rect.top + rect.height / 2
  )
  return Boolean(topEl && toast.contains(topEl))
}

async function fakeNpmCache(options: { npmPackage: string; versions: string[]; latest?: string }) {
  const denoDir = await mkdtemp(join(tmpdir(), 'mymcps-edit-modal-deno-'))
  const packageDir = join(denoDir, 'npm', 'registry.npmjs.org', ...options.npmPackage.split('/'))
  await mkdir(packageDir, { recursive: true })
  for (const version of options.versions) {
    await mkdir(join(packageDir, version), { recursive: true })
  }
  if (options.latest) {
    await writeFile(
      join(packageDir, 'registry.json'),
      JSON.stringify({ 'dist-tags': { latest: options.latest } })
    )
  }
  return denoDir
}

test.group('MCP edit modal UX', (group) => {
  let previousDenoDir: string | undefined
  let denoDir: string | undefined

  group.each.setup(async () => {
    await beginTestTransaction()
    previousDenoDir = process.env.DENO_DIR
    denoDir = await fakeNpmCache({
      npmPackage: '@shopify/dev-mcp',
      versions: ['1.14.4'],
      latest: '1.14.4',
    })
    process.env.DENO_DIR = denoDir

    mcpNpmUpdateRuntime.reload = async () => {
      await new Promise((resolve) => setTimeout(resolve, 1500))
    }
    mcpNpmUpdateRuntime.probe = async (mcp) => {
      mcp.status = 'ready'
      mcp.lastError = null
      await mcp.save()
      return mcp
    }
  })

  group.each.teardown(async () => {
    resetMcpNpmUpdateRuntime()
    if (previousDenoDir === undefined) {
      delete process.env.DENO_DIR
    } else {
      process.env.DENO_DIR = previousDenoDir
    }
    if (denoDir) {
      await rm(denoDir, { recursive: true, force: true })
      denoDir = undefined
    }
    await rollbackTestTransaction()
  })

  test('aligns npm fields, hides the form while updating, and stacks toasts above the dialog', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    await createMcp(admin.id, {
      name: 'Shopify Dev',
      transport: 'npm',
      npmPackage: '@shopify/dev-mcp',
      npmVersion: 'latest',
    })

    await browserContext.loginAs(admin)
    const page = await visit('/mcps')
    await page.getByRole('button', { name: 'Edit' }).click()

    const dialog = page.getByRole('dialog', { name: 'Edit Shopify Dev' })
    await dialog.getByText('Cached in Deno: 1.14.4').waitFor()

    const versionBox = await dialog.locator('input[name="npmVersion"]').boundingBox()
    const extraArgsBox = await dialog.locator('input[name="npmArgs"]').boundingBox()
    assert.isNotNull(versionBox)
    assert.isNotNull(extraArgsBox)
    assert.isAtMost(Math.abs(versionBox!.y - extraArgsBox!.y), 10)

    const cachedBox = await dialog.getByText('Cached in Deno: 1.14.4').boundingBox()
    assert.isNotNull(cachedBox)
    assert.isAtLeast(cachedBox!.y, versionBox!.y + versionBox!.height - 1)

    await dialog.getByRole('button', { name: 'Update MCP' }).click()
    await dialog.getByRole('status', { name: 'Updating MCP' }).waitFor()

    const formOpacity = await dialog.locator('.dialog-form-updating').evaluate(getOpacity)
    assert.equal(formOpacity, '0')

    await page.getByText('MCP updated to latest').waitFor()
    await page.waitForFunction(flashToastIsHitAtItsCenter)
  })
})
