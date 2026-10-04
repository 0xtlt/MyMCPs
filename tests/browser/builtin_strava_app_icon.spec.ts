import { readFile } from 'node:fs/promises'
import { test } from '@japa/runner'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin } from '#tests/helpers/factories'

/** Decode an image in the page and read two of its pixels as RGBA. */
async function readPixels(dataUrl: string) {
  const browser = globalThis as unknown as {
    Image: new () => { src: string; decode: () => Promise<void> }
    document: {
      createElement: (tag: 'canvas') => {
        width: number
        height: number
        getContext: (kind: '2d') => {
          drawImage: (image: unknown, x: number, y: number) => void
          getImageData: (x: number, y: number, w: number, h: number) => { data: Iterable<number> }
        }
      }
    }
  }
  const image = new browser.Image()
  image.src = dataUrl
  await image.decode()

  const canvas = browser.document.createElement('canvas')
  canvas.width = 512
  canvas.height = 512
  const context = canvas.getContext('2d')
  context.drawImage(image, 0, 0)
  const pixel = (x: number, y: number) => Array.from(context.getImageData(x, y, 1, 1).data)
  return { corner: pixel(2, 2), stroke: pixel(127, 253) }
}

test.group('Built-in Strava application icon', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('downloads an opaque square PNG that is ready to upload to Strava', async ({
    assert,
    browserContext,
    visit,
  }) => {
    const admin = await createAdmin()
    await browserContext.loginAs(admin)
    const page = await visit('/mcps')

    await page.getByRole('button', { name: 'Add MCP' }).click()
    const gallery = page.getByRole('dialog', { name: 'Add an MCP' })
    await gallery.getByRole('textbox', { name: 'Search templates' }).fill('Strava')
    await gallery.getByRole('button', { name: 'Set up Strava' }).click()

    const setup = page.getByRole('dialog', { name: 'Set up Strava' })
    const [download] = await Promise.all([
      page.waitForEvent('download'),
      setup.getByRole('button', { name: 'Download app icon' }).click(),
    ])

    assert.equal(download.suggestedFilename(), 'mymcps-app-icon.png')
    const png = await readFile(await download.path())
    // PNG signature, then the IHDR chunk with the width and height.
    assert.equal(png.subarray(0, 8).toString('hex'), '89504e470d0a1a0a')
    assert.equal(png.readUInt32BE(16), 512)
    assert.equal(png.readUInt32BE(20), 512)

    // The logo is transparent. The icon must be the mark on solid white.
    const pixels = await page.evaluate(
      readPixels,
      `data:image/png;base64,${png.toString('base64')}`
    )
    assert.deepEqual(pixels.corner, [255, 255, 255, 255])
    assert.equal(pixels.stroke[3], 255)
    assert.isBelow(pixels.stroke[0], 100)

    // Downloading must not submit or close the setup form.
    assert.equal(await setup.getByRole('textbox', { name: 'Client ID' }).count(), 1)
    assert.equal(new URL(page.url()).pathname, '/mcps')
  })
})
