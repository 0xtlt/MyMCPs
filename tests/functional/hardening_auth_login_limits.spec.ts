import { test } from '@japa/runner'
import limiter from '@adonisjs/limiter/services/main'
import type { ApiClient } from '@japa/api-client'
import { createAdmin, createMember } from '#tests/helpers/factories'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'

/**
 * The test server trusts its loopback peer, so the forwarded address is the
 * client address the limiter sees.
 */
function attempt(client: ApiClient, from: string, email: string, password: string) {
  return client
    .post('/login')
    .header('x-forwarded-for', from)
    .withCsrfToken()
    .redirects(0)
    .form({ email, password })
}

test.group('hardening: login rate limits', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.setup(() => limiter.clear(['memory']))
  group.each.teardown(rollbackTestTransaction)

  test('a successful login on another account does not reset the guesses on one', async ({
    client,
  }) => {
    await createAdmin({ email: 'admin@example.com' })
    await createMember({ email: 'member@example.com' })

    for (let guess = 0; guess < 5; guess++) {
      const wrong = await attempt(client, '198.51.100.10', 'admin@example.com', 'wrong-password')
      wrong.assertStatus(302)
      const own = await attempt(client, '198.51.100.10', 'member@example.com', 'password123')
      own.assertStatus(302)
    }

    const limited = await attempt(client, '198.51.100.10', 'admin@example.com', 'password123')
    limited.assertStatus(429)
    limited.assertHeader('retry-after')
    limited.assertTextIncludes('Too many requests')
  })

  test('a successful login clears the guesses on its own account', async ({ client }) => {
    await createAdmin({ email: 'admin@example.com' })

    for (let round = 0; round < 2; round++) {
      for (let guess = 0; guess < 4; guess++) {
        await attempt(client, '198.51.100.11', 'admin@example.com', 'wrong-password')
      }
      const own = await attempt(client, '198.51.100.11', 'admin@example.com', 'password123')
      own.assertStatus(302)
      own.assertCookie('remember_web')
    }
  })

  test('counts an account under any letter case of its email', async ({ client }) => {
    await createAdmin({ email: 'admin@example.com' })

    const spellings = ['admin@example.com', 'ADMIN@example.com', 'Admin@Example.COM']
    for (let guess = 0; guess < 5; guess++) {
      const wrong = await attempt(client, '198.51.100.12', spellings[guess % 3], 'wrong-password')
      wrong.assertStatus(302)
    }

    const limited = await attempt(client, '198.51.100.12', 'aDmin@example.com', 'wrong-password')
    limited.assertStatus(429)
  })

  test('interleaved successful logins do not reset the address-wide budget', async ({
    client,
    assert,
  }) => {
    await createMember({ email: 'member@example.com' })

    for (let guess = 0; guess < 30; guess++) {
      const wrong = await attempt(
        client,
        '198.51.100.13',
        `target-${guess}@example.com`,
        'wrong-password'
      )
      assert.notEqual(wrong.status(), 429)

      if (guess % 3 === 0) {
        const own = await attempt(client, '198.51.100.13', 'member@example.com', 'password123')
        assert.notEqual(own.status(), 429)
      }
    }

    const limited = await attempt(client, '198.51.100.13', 'fresh@example.com', 'wrong-password')
    limited.assertStatus(429)
    limited.assertHeader('retry-after')

    const elsewhere = await attempt(client, '198.51.100.14', 'fresh@example.com', 'wrong-password')
    assert.notEqual(elsewhere.status(), 429)
  })

  test('caps a parallel burst of wrong passwords on one account', async ({ client, assert }) => {
    await createAdmin({ email: 'admin@example.com' })

    const responses = await Promise.all(
      Array.from({ length: 20 }, () =>
        attempt(client, '198.51.100.15', 'admin@example.com', 'wrong-password')
      )
    )

    assert.lengthOf(
      responses.filter((response) => response.status() !== 429),
      5
    )
  })

  test('caps a parallel burst of wrong passwords across accounts', async ({ client, assert }) => {
    await createAdmin()

    const responses = await Promise.all(
      Array.from({ length: 40 }, (_, guess) =>
        attempt(client, '198.51.100.16', `target-${guess}@example.com`, 'wrong-password')
      )
    )

    assert.lengthOf(
      responses.filter((response) => response.status() !== 429),
      30
    )
  })

  test('counts every address of an IPv6 /64 as one client', async ({ client, assert }) => {
    await createAdmin({ email: 'admin@example.com' })

    for (let guess = 0; guess < 5; guess++) {
      const wrong = await attempt(
        client,
        `2001:db8:12:34:${guess + 1}::1`,
        'admin@example.com',
        'wrong-password'
      )
      assert.notEqual(wrong.status(), 429)
    }

    const sameNetwork = await attempt(
      client,
      '2001:db8:12:34:ffff::9',
      'admin@example.com',
      'wrong-password'
    )
    sameNetwork.assertStatus(429)

    const otherNetwork = await attempt(
      client,
      '2001:db8:12:35::1',
      'admin@example.com',
      'wrong-password'
    )
    assert.notEqual(otherNetwork.status(), 429)
  })

  test('counts by socket address when the forwarded value is not an address', async ({
    client,
    assert,
  }) => {
    await createAdmin({ email: 'admin@example.com' })

    for (let guess = 0; guess < 5; guess++) {
      const wrong = await attempt(client, `spoofed-${guess}`, 'admin@example.com', 'wrong-password')
      assert.notEqual(wrong.status(), 429)
    }

    const limited = await attempt(client, 'spoofed-again', 'admin@example.com', 'wrong-password')
    limited.assertStatus(429)
  })
})
