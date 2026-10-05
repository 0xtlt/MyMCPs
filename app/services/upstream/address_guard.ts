import { lookup } from 'node:dns/promises'
import { BlockList, isIP } from 'node:net'

/**
 * Networks a URL taken from a remote document must not lead into: unspecified,
 * private, CGNAT, loopback, link-local (cloud metadata lives at
 * 169.254.169.254), and the special-purpose blocks no public OAuth provider is
 * hosted on. Documentation ranges are left out: nothing listens there.
 */
const RESTRICTED_IPV4_SUBNETS: ReadonlyArray<readonly [string, number]> = [
  ['0.0.0.0', 8],
  ['10.0.0.0', 8],
  ['100.64.0.0', 10],
  ['127.0.0.0', 8],
  ['169.254.0.0', 16],
  ['172.16.0.0', 12],
  ['192.0.0.0', 24],
  ['192.168.0.0', 16],
  ['198.18.0.0', 15],
  ['224.0.0.0', 4],
  ['240.0.0.0', 4],
]

/** Unspecified, loopback, local-use NAT64, unique-local, link-local, site-local, multicast. */
const RESTRICTED_IPV6_SUBNETS: ReadonlyArray<readonly [string, number]> = [
  ['::', 128],
  ['::1', 128],
  ['64:ff9b:1::', 48],
  ['fc00::', 7],
  ['fe80::', 10],
  ['fec0::', 10],
  ['ff00::', 8],
]

function ipv4AsHextets(address: string) {
  const [a, b, c, d] = address.split('.').map(Number)
  return `${((a << 8) | b).toString(16)}:${((c << 8) | d).toString(16)}`
}

const restricted = new BlockList()
for (const [network, prefix] of RESTRICTED_IPV4_SUBNETS) {
  restricted.addSubnet(network, prefix, 'ipv4')
  // IPv6 forms that carry an IPv4 destination: IPv4-compatible, NAT64 and
  // 6to4, which a translator or tunnel delivers to the IPv4 address.
  // BlockList itself matches IPv4-mapped addresses against the IPv4 rules.
  const hextets = ipv4AsHextets(network)
  restricted.addSubnet(`::${hextets}`, 96 + prefix, 'ipv6')
  restricted.addSubnet(`64:ff9b::${hextets}`, 96 + prefix, 'ipv6')
  restricted.addSubnet(`2002:${hextets}::`, 16 + prefix, 'ipv6')
}
for (const [network, prefix] of RESTRICTED_IPV6_SUBNETS) {
  restricted.addSubnet(network, prefix, 'ipv6')
}

/** Whether an IP address lies in a network the gateway must not be sent into. */
export function isRestrictedAddress(address: string) {
  // getaddrinfo reports link-local addresses with their zone, `fe80::1%en0`.
  const unscoped = address.split('%', 1)[0]
  const family = isIP(unscoped)
  if (family === 0) {
    return true
  }
  return restricted.check(unscoped, family === 6 ? 'ipv6' : 'ipv4')
}

async function systemLookup(hostname: string) {
  const entries = await lookup(hostname, { all: true })
  return entries.map((entry) => entry.address)
}

/**
 * Name resolution used by the guard. It goes through getaddrinfo like the HTTP
 * client, so the hosts file and `localhost` names count. Replaceable in tests.
 */
export const addressGuardRuntime = {
  lookup: systemLookup,
}

export function resetAddressGuardRuntime() {
  addressGuardRuntime.lookup = systemLookup
}

/**
 * Whether the host of a parsed URL is, or resolves to, a restricted address.
 * `URL` has already turned every IPv4 notation (decimal, octal, hex, short
 * forms) into dotted decimal and compressed IPv6 literals inside brackets.
 */
export async function resolvesToRestrictedAddress(hostname: string) {
  const literal = hostname.startsWith('[') ? hostname.slice(1, -1) : hostname
  if (isIP(literal) !== 0) {
    return isRestrictedAddress(literal)
  }

  let addresses: string[]
  try {
    addresses = await addressGuardRuntime.lookup(hostname)
  } catch {
    // A name that does not resolve cannot be connected to either.
    return false
  }
  return addresses.some(isRestrictedAddress)
}

/** A discovered endpoint leads into a network the MCP itself is not part of. */
export class RestrictedEndpointError extends Error {
  constructor(label: string, hostname: string) {
    super(
      `${label} host "${hostname}" is a loopback, private or link-local address, which a remote MCP may not send MyMCPs to`
    )
    this.name = 'RestrictedEndpointError'
  }
}

/**
 * Check for URLs that come from documents the MCP or its OAuth provider serves
 * (authorization server, token, registration and authorization endpoints),
 * which would otherwise let a remote MCP aim gateway requests at the
 * instance's own network.
 *
 * Operators may point an MCP at a private address. When the MCP's own host is
 * one, its provider may be too, and nothing is refused.
 *
 * The check resolves the name before the request is made and the HTTP client
 * resolves it again to connect, so a DNS answer that changes in between (DNS
 * rebinding) is not caught. Closing that window needs the check at connect
 * time, in the socket lookup of a dedicated HTTP dispatcher.
 */
export function discoveredEndpointGuard(mcpUrl: URL) {
  let mcpIsRestricted: Promise<boolean> | undefined
  const verdicts = new Map<string, Promise<boolean>>()

  return async function assertDiscoveredEndpointAllowed(target: URL | string, label: string) {
    const { hostname } = typeof target === 'string' ? new URL(target) : target
    if (hostname === mcpUrl.hostname) {
      return
    }

    mcpIsRestricted ??= resolvesToRestrictedAddress(mcpUrl.hostname)
    if (await mcpIsRestricted) {
      return
    }

    let verdict = verdicts.get(hostname)
    if (!verdict) {
      verdict = resolvesToRestrictedAddress(hostname)
      verdicts.set(hostname, verdict)
    }
    if (await verdict) {
      throw new RestrictedEndpointError(label, hostname)
    }
  }
}

export type DiscoveredEndpointGuard = ReturnType<typeof discoveredEndpointGuard>
