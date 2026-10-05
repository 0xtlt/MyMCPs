import { randomBytes } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import { test } from '@japa/runner'
import { Secret } from '@adonisjs/core/helpers'
import app from '@adonisjs/core/services/app'
import type { ApplicationService } from '@adonisjs/core/types'
import env from '#start/env'
import AppKeyGuardProvider, {
  assertPrivateAppKey,
  isPublishedAppKey,
} from '#providers/app_key_guard_provider'

/**
 * Reads the APP_KEY a repository file assigns, so the guard cannot drift from
 * the values that are actually published.
 */
async function publishedAppKey(file: string) {
  const match = /^\s*APP_KEY=(\S+)/m.exec(await readFile(app.makePath(file), 'utf8'))
  if (!match) throw new Error(`${file} no longer assigns APP_KEY`)
  return match[1]
}

test.group('hardening: published APP_KEY guard', () => {
  test('recognizes the Docker build placeholder and the committed test key', async ({ assert }) => {
    const buildKey = await publishedAppKey('Dockerfile')
    const testKey = await publishedAppKey('.env.test')

    assert.isTrue(isPublishedAppKey(buildKey))
    assert.isTrue(isPublishedAppKey(testKey))
    assert.isTrue(isPublishedAppKey(` ${buildKey}\n`))
    assert.throws(
      () => assertPrivateAppKey(buildKey),
      /Refusing to start: APP_KEY is a placeholder/
    )
    assert.throws(() => assertPrivateAppKey(testKey), /Refusing to start: APP_KEY is a placeholder/)
  })

  test('accepts a generated key', ({ assert }) => {
    const generated = `base64:${randomBytes(32).toString('base64')}`

    assert.isFalse(isPublishedAppKey(generated))
    assert.doesNotThrow(() => assertPrivateAppKey(generated))
  })

  test('only checks the configured key in production', async ({ assert, cleanup }) => {
    const configured = env.get('APP_KEY')
    const exported = process.env.APP_KEY
    cleanup(() => {
      env.set('APP_KEY', configured)
      // Env.set() also writes the redacted secret to process.env, which the
      // child processes later specs spawn would inherit.
      if (exported === undefined) delete process.env.APP_KEY
      else process.env.APP_KEY = exported
    })
    env.set('APP_KEY', new Secret(await publishedAppKey('Dockerfile')))

    const outsideProduction = new AppKeyGuardProvider({ inProduction: false } as ApplicationService)
    await assert.doesNotReject(() => outsideProduction.boot())

    const inProduction = new AppKeyGuardProvider({ inProduction: true } as ApplicationService)
    await assert.rejects(() => inProduction.boot(), /Refusing to start: APP_KEY is a placeholder/)

    env.set('APP_KEY', new Secret(`base64:${randomBytes(32).toString('base64')}`))
    await assert.doesNotReject(() => inProduction.boot())
  })
})
