import { isIP, isIPv6 } from 'node:net'
import { defineConfig } from '@adonisjs/core/http'

type TrustProxy = (address: string, distance: number) => boolean

/**
 * Returned when not even the socket has a peer address, e.g. after the client
 * disconnected.
 */
const UNKNOWN_IP = '0.0.0.0'

/**
 * proxy-addr belongs to the HTTP server, whose config helper compiles one
 * entry at a time: an IP, a CIDR range, or one of the names loopback,
 * linklocal and uniquelocal.
 */
function compileTrustProxyEntry(entry: string): TrustProxy {
  try {
    return defineConfig({ trustProxy: entry.toLowerCase() }).trustProxy
  } catch {
    throw new Error(
      `Invalid TRUST_PROXY entry "${entry}". Use true, false, or a comma-separated list of proxy IPs, CIDR ranges, and the names loopback, linklocal, uniquelocal.`
    )
  }
}

/**
 * Turn TRUST_PROXY into the HTTP server's proxy trust policy. The default
 * trusts loopback proxies only.
 */
export function resolveTrustProxy(value: string | undefined): boolean | TrustProxy {
  const entries = (value ?? '')
    .split(',')
    .map((entry) => entry.trim())
    .filter(Boolean)

  if (entries.length === 0) {
    entries.push('loopback')
  }

  if (entries.length === 1 && ['true', 'false'].includes(entries[0].toLowerCase())) {
    return entries[0].toLowerCase() === 'true'
  }

  const policies = entries.map(compileTrustProxyEntry)
  return (address, distance) => policies.some((trusts) => trusts(address, distance))
}

/**
 * The address proxy-addr resolves is copied verbatim from X-Forwarded-For
 * whenever every hop after it is trusted, so it can be any string. Fall back
 * to the socket peer unless it is an IP address: limiter keys and call logs
 * rely on `request.ip()` being one.
 */
export function resolveClientIp(forwarded: string | undefined, socketAddress: string | undefined) {
  if (forwarded && isIP(forwarded)) {
    return forwarded
  }
  return socketAddress && isIP(socketAddress) ? socketAddress : UNKNOWN_IP
}

/**
 * The eight groups of an IPv6 address. The URL parser does the validation and
 * yields the canonical form, without zone or dotted quad.
 */
function ipv6Groups(ip: string) {
  const canonical = new URL(`http://[${ip.split('%')[0]}]`).hostname.slice(1, -1)
  const [head, tail] = canonical.split('::')
  const leading = head ? head.split(':') : []
  const trailing = tail ? tail.split(':') : []

  return tail === undefined
    ? leading
    : [...leading, ...Array(8 - leading.length - trailing.length).fill('0'), ...trailing]
}

/**
 * The key a client address is rate-limited under. An IPv6 client usually
 * holds a whole /64 and can use a new address for every request, so IPv6 is
 * keyed on that prefix; IPv4, including its IPv6-mapped form, on the address.
 */
export function rateLimitClientKey(ip: string) {
  if (!isIPv6(ip)) {
    return ip
  }

  const groups = ipv6Groups(ip)
  if (groups.slice(0, 5).every((group) => group === '0') && groups[5] === 'ffff') {
    const mapped = (Number.parseInt(groups[6], 16) << 16) | Number.parseInt(groups[7], 16)
    return [24, 16, 8, 0].map((shift) => (mapped >>> shift) & 0xff).join('.')
  }

  return `${groups.slice(0, 4).join(':')}::/64`
}
