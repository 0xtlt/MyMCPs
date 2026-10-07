import { test } from '@japa/runner'
import {
  customerLabel,
  customerNumber,
  fromMicros,
  money,
  toMicros,
} from '#services/builtin/google_ads/format'
import { imageInfo, imageShape } from '#services/builtin/google_ads/images'
import { toolInput } from '#services/builtin/tool_input'
import {
  addKeywordsValidator,
  createCampaignValidator,
  updateCampaignTargetingValidator,
} from '#validators/builtin_google_ads'

function png(width: number, height: number) {
  const bytes = Buffer.alloc(33)
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]).copy(bytes)
  bytes.writeUInt32BE(13, 8)
  bytes.write('IHDR', 12, 'latin1')
  bytes.writeUInt32BE(width, 16)
  bytes.writeUInt32BE(height, 20)
  return bytes
}

/** The start of a JPEG file: an application segment, then the frame header that carries the size. */
function jpeg(width: number, height: number) {
  const app0 = Buffer.from([0xff, 0xe0, 0x00, 0x04, 0x4a, 0x46])
  const frame = Buffer.alloc(11)
  frame.writeUInt16BE(0xffc2, 0)
  frame.writeUInt16BE(9, 2)
  frame.writeUInt8(8, 4)
  frame.writeUInt16BE(height, 5)
  frame.writeUInt16BE(width, 7)
  return Buffer.concat([Buffer.from([0xff, 0xd8]), app0, frame])
}

function gif(width: number, height: number) {
  const bytes = Buffer.alloc(13)
  bytes.write('GIF89a', 0, 'latin1')
  bytes.writeUInt16LE(width, 6)
  bytes.writeUInt16LE(height, 8)
  return bytes
}

test.group('Google Ads: accounts and money', () => {
  test('writes account IDs the way the API takes them and the way people read them', ({
    assert,
  }) => {
    assert.equal(customerNumber('123-456-7890'), '1234567890')
    assert.equal(customerNumber('1234567890'), '1234567890')
    assert.equal(customerLabel('1234567890'), '123-456-7890')
  })

  test('converts amounts to micros without the errors of binary fractions', ({ assert }) => {
    assert.equal(toMicros(250), '250000000')
    assert.equal(toMicros(2.5), '2500000')
    assert.equal(toMicros(0.07), '70000')
    // 1.1 * 1,000,000 is 1100000.0000000002 in floating point.
    assert.equal(toMicros(1.1), '1100000')
    assert.equal(toMicros(19.999), '20000000')

    assert.equal(fromMicros('2500000'), 2.5)
    assert.equal(fromMicros(0), 0)
    assert.isUndefined(fromMicros(undefined))
    assert.isUndefined(fromMicros(''))
  })

  test('says amounts in the currency of the account', ({ assert }) => {
    assert.equal(money(250, 'EUR'), '€250.00')
    assert.equal(money(2.5, 'USD'), '$2.50')
    assert.equal(money(7600, 'EUR'), '€7,600.00')
    assert.equal(money(1500, 'JPY'), '¥1,500')
    assert.equal(money(12, 'not a currency'), '12.00 not a currency')
  })
})

test.group('Google Ads: images', () => {
  test('reads the size of PNG, JPEG, and GIF files from their first bytes', ({ assert }) => {
    assert.deepEqual(imageInfo(png(1200, 628)), { format: 'png', width: 1200, height: 628 })
    assert.deepEqual(imageInfo(jpeg(600, 600)), { format: 'jpeg', width: 600, height: 600 })
    assert.deepEqual(imageInfo(gif(512, 128)), { format: 'gif', width: 512, height: 128 })
  })

  test('takes nothing else for an image', ({ assert }) => {
    assert.isNull(imageInfo(Buffer.from('<svg xmlns="http://www.w3.org/2000/svg"/>')))
    assert.isNull(imageInfo(Buffer.from('%PDF-1.4')))
    assert.isNull(imageInfo(Buffer.alloc(0)))
    assert.isNull(imageInfo(png(0, 628)))
    // A JPEG cut before its frame header has no size to read.
    assert.isNull(imageInfo(jpeg(600, 600).subarray(0, 10)))
  })

  test('names the shapes Google Ads uses, within 1% of their ratio', ({ assert }) => {
    assert.equal(imageShape({ width: 1200, height: 628 })?.name, 'landscape')
    assert.equal(imageShape({ width: 600, height: 314 })?.name, 'landscape')
    assert.equal(imageShape({ width: 300, height: 300 })?.name, 'square')
    assert.equal(imageShape({ width: 512, height: 128 })?.name, 'wide_logo')
    assert.equal(imageShape({ width: 960, height: 1200 })?.name, 'portrait')
    assert.isNull(imageShape({ width: 800, height: 600 }))
    assert.isNull(imageShape({ width: 1200, height: 600 }))
  })
})

test.group('Google Ads: arguments', () => {
  const campaign = {
    customer_id: '123-456-7890',
    name: 'Autumn sale',
    channel: 'SEARCH',
    daily_budget: 12.5,
    bidding_strategy: 'MAXIMIZE_CLICKS',
  }

  test('takes identifiers as numbers or text, and keeps them as text', async ({ assert }) => {
    const input = await toolInput(createCampaignValidator, {
      ...campaign,
      customer_id: 1234567890,
      location_ids: [2250, '21167'],
      languages: 'fr',
    })

    assert.equal(input.customer_id, '1234567890')
    assert.deepEqual(input.location_ids, ['2250', '21167'])
    // A single value is a list of one.
    assert.deepEqual(input.languages, ['fr'])
  })

  test('writes language codes the way Google Ads does', async ({ assert }) => {
    const input = await toolInput(updateCampaignTargetingValidator, {
      customer_id: '1234567890',
      campaign_id: '111',
      add_languages: ['FR', 'pt-br', 'zh_cn'],
    })
    assert.deepEqual(input.add_languages, ['fr', 'pt_BR', 'zh_CN'])

    await assert.rejects(
      () =>
        toolInput(updateCampaignTargetingValidator, {
          customer_id: '1234567890',
          campaign_id: '111',
          add_languages: ['french'],
        }),
      'add_languages must be a list of 1 to 30 language codes such as en, fr, or pt_BR'
    )
  })

  test('refuses dates that do not exist and budgets out of range', async ({ assert }) => {
    await assert.rejects(
      () => toolInput(createCampaignValidator, { ...campaign, start_date: '2026-02-30' }),
      'start_date must be a date such as 2026-01-31'
    )
    await assert.rejects(
      () => toolInput(createCampaignValidator, { ...campaign, start_date: '31/01/2026' }),
      'start_date must be a date such as 2026-01-31'
    )
    await assert.rejects(
      () => toolInput(createCampaignValidator, { ...campaign, daily_budget: 2_000_000 }),
      'daily_budget must be a number between 0.01 and 1000000'
    )
    await assert.rejects(
      () => toolInput(createCampaignValidator, { ...campaign, customer_id: '12345' }),
      'customer_id must be a Google Ads account ID such as 123-456-7890'
    )
  })

  test('wants a match type for every keyword', async ({ assert }) => {
    const keywords = { customer_id: '1234567890', ad_group_id: '333' }
    const input = await toolInput(addKeywordsValidator, {
      ...keywords,
      keywords: [{ text: ' trail shoes ', match_type: 'EXACT', max_cpc: '0.8' }],
    })
    assert.deepEqual(input.keywords, [{ text: 'trail shoes', matchType: 'EXACT', maxCpc: 0.8 }])

    for (const wrong of [
      [{ text: 'trail shoes' }],
      [{ text: 'trail shoes', match_type: 'LOOSE' }],
      [{ text: 'x'.repeat(81), match_type: 'EXACT' }],
      [{ text: 'trail shoes', match_type: 'EXACT', max_cpc: 0 }],
      ['trail shoes'],
      [],
    ]) {
      await assert.rejects(
        () => toolInput(addKeywordsValidator, { ...keywords, keywords: wrong }),
        /^keywords must be a list of 1 to 100 keywords such as/
      )
    }
  })
})
