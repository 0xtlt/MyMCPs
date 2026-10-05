import type { IncomingMessage } from 'node:http'
import { isIP } from 'node:net'
import { test } from '@japa/runner'
import { defineConfig } from '@adonisjs/core/http'
import { RequestFactory } from '@adonisjs/core/factories/http'
import { http } from '#config/app'
import { rateLimitClientKey, resolveClientIp, resolveTrustProxy } from '#services/client_ip'

/**
 * The IP the HTTP server reports for a request from `socketAddress` carrying
 * the given X-Forwarded-For header.
 */
function requestIp(config: ReturnType<typeof defineConfig>, socketAddress: string, xff?: string) {
  const req = {
    url: '/',
    headers: xff === undefined ? {} : { 'x-forwarded-for': xff },
    socket: { remoteAddress: socketAddress },
  } as unknown as IncomingMessage

  return new RequestFactory().merge({ req, config }).create().ip()
}

function configFor(trustProxy: string | undefined) {
  return defineConfig({ trustProxy: resolveTrustProxy(trustProxy), getIp: http.getIp })
}

test.group('hardening: proxy trust policy', () => {
  test('trusts only loopback proxies by default')
    .with([undefined, '', '  ', 'loopback', 'Loopback'])
    .run(({ assert }, value) => {
      const trusts = resolveTrustProxy(value)

      assert.isFunction(trusts)
      if (typeof trusts !== 'function') return
      assert.isTrue(trusts('127.0.0.1', 0))
      assert.isTrue(trusts('::1', 0))
      assert.isFalse(trusts('10.0.0.5', 0))
      assert.isFalse(trusts('203.0.113.7', 0))
    })

  test('maps booleans to trusting every proxy or none', ({ assert }) => {
    assert.isTrue(resolveTrustProxy('true'))
    assert.isTrue(resolveTrustProxy(' TRUE '))
    assert.isFalse(resolveTrustProxy('false'))
  })

  test('accepts a comma-separated list of range names', ({ assert }) => {
    const trusts = resolveTrustProxy('loopback,uniquelocal')

    assert.isFunction(trusts)
    if (typeof trusts !== 'function') return
    for (const proxy of ['127.0.0.1', '::1', '10.0.1.9', '172.18.0.2', '192.168.1.1', 'fd00::7']) {
      assert.isTrue(trusts(proxy, 0), proxy)
    }
    assert.isTrue(trusts('::ffff:172.18.0.2', 0))
    for (const client of ['203.0.113.7', '172.32.0.1', '169.254.1.1', '2001:db8::1', 'traefik']) {
      assert.isFalse(trusts(client, 0), client)
    }
  })

  test('accepts a list mixing addresses, CIDR ranges and names', ({ assert }) => {
    const trusts = resolveTrustProxy(' 203.0.113.7 , 198.51.100.0/24,linklocal, ')

    assert.isFunction(trusts)
    if (typeof trusts !== 'function') return
    assert.isTrue(trusts('203.0.113.7', 0))
    assert.isTrue(trusts('198.51.100.200', 0))
    assert.isTrue(trusts('169.254.1.1', 0))
    assert.isFalse(trusts('203.0.113.8', 0))
    assert.isFalse(trusts('127.0.0.1', 0))
  })

  test('names the entry it cannot use', ({ assert }) => {
    assert.throws(
      () => resolveTrustProxy('loopback,traefik'),
      /Invalid TRUST_PROXY entry "traefik"/
    )
    assert.throws(() => resolveTrustProxy('true,10.0.0.0/8'), /Invalid TRUST_PROXY entry "true"/)
    assert.throws(() => resolveTrustProxy('10.0.0.0/64'), /Invalid TRUST_PROXY entry/)
  })
})

test.group('hardening: client IP', () => {
  test('keeps a forwarded value only when it is an IP address', ({ assert }) => {
    assert.equal(resolveClientIp('203.0.113.7', '172.18.0.2'), '203.0.113.7')
    assert.equal(resolveClientIp('2001:db8::1', '172.18.0.2'), '2001:db8::1')

    for (const forged of ['', 'unknown', '203.0.113.7:4711', '203.0.113', '<script>', undefined]) {
      assert.equal(resolveClientIp(forged, '172.18.0.2'), '172.18.0.2')
    }
  })

  test('still answers with an address when the socket has none', ({ assert }) => {
    assert.equal(isIP(resolveClientIp('unknown', undefined)), 4)
  })

  test('uses the nearest address a private-network proxy did not vouch for', ({ assert }) => {
    const config = configFor('loopback,uniquelocal')

    assert.equal(requestIp(config, '172.18.0.2', '203.0.113.7'), '203.0.113.7')
    assert.equal(requestIp(config, '172.18.0.2', '198.51.100.1, 203.0.113.7'), '203.0.113.7')
    assert.equal(requestIp(config, '172.18.0.2', 'forged, 203.0.113.7'), '203.0.113.7')
    assert.equal(requestIp(config, '203.0.113.7', '198.51.100.1'), '203.0.113.7')
  })

  test('falls back to the socket address when the forwarded client is not an IP address')
    .with(['true', 'loopback,uniquelocal'])
    .run(({ assert }, trustProxy) => {
      const config = configFor(trustProxy)

      for (const forged of ['forged', '203.0.113.7:4711', "' OR 1=1 --", '10.0.0.999']) {
        assert.equal(requestIp(config, '172.18.0.2', forged), '172.18.0.2', forged)
      }
      assert.equal(requestIp(config, '172.18.0.2'), '172.18.0.2')
    })

  test('the application reports an address for a forged forwarded header', ({ assert }) => {
    assert.equal(requestIp(http, '127.0.0.1', 'forged'), '127.0.0.1')
    assert.equal(requestIp(http, '127.0.0.1', '203.0.113.7'), '203.0.113.7')
  })
})

test.group('hardening: rate-limit client key', () => {
  test('keys IPv4 clients on their address', ({ assert }) => {
    assert.equal(rateLimitClientKey('203.0.113.7'), '203.0.113.7')
    assert.equal(rateLimitClientKey('::ffff:203.0.113.7'), '203.0.113.7')
    assert.equal(rateLimitClientKey('::FFFF:cb00:7107'), '203.0.113.7')
  })

  test('keys IPv6 clients on their /64', ({ assert }) => {
    const key = rateLimitClientKey('2001:db8:12:34::1')

    assert.equal(key, '2001:db8:12:34::/64')
    assert.equal(rateLimitClientKey('2001:0DB8:0012:0034:ffff:ffff:ffff:ffff'), key)
    assert.equal(rateLimitClientKey('2001:db8:12:34:5:6:7:8%eth0'), key)
    assert.notEqual(rateLimitClientKey('2001:db8:12:35::1'), key)
    assert.equal(rateLimitClientKey('::1'), '0:0:0:0::/64')
    assert.equal(rateLimitClientKey('fe80::1'), 'fe80:0:0:0::/64')
  })
})
