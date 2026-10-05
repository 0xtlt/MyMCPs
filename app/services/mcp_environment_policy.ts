/**
 * Environment variable names an operator may not set on an npm MCP.
 *
 * The variables of an npm MCP are handed to the `deno` process, not only to
 * the package it runs. These names are read by the binary lookup, the dynamic
 * loader, libc or the Deno runtime before or outside Deno's permission
 * checks, so they would steer the sandbox itself. Ordinary application
 * settings (API keys, `*_URL`, `NODE_ENV`, proxies, `NPM_CONFIG_REGISTRY`)
 * stay allowed.
 */

type ReservedNames = {
  names?: readonly string[]
  prefixes?: readonly string[]
  reason: (name: string) => string
}

const RESERVED: readonly ReservedNames[] = [
  {
    // Set by the gateway for every npm MCP. PATHEXT is the Windows half of PATH.
    names: ['HOME', 'TMPDIR', 'NO_COLOR', 'PATH', 'PATHEXT'],
    reason: (name) => `"${name}" is set by MyMCPs for the sandbox and cannot be changed`,
  },
  {
    // DENO_DIR, DENO_V8_FLAGS, DENO_CERT, DENO_TLS_CA_STORE, DENO_AUTH_TOKENS...
    prefixes: ['DENO_'],
    // Deno does not act on these today, but they address the runtime rather
    // than the package and Node compatibility keeps growing.
    names: ['NODE_OPTIONS', 'NODE_PATH'],
    reason: (name) =>
      `"${name}" configures the runtime that sandboxes the package, not the package, and cannot be set`,
  },
  {
    // ld.so (LD_PRELOAD, LD_LIBRARY_PATH, LD_AUDIT...) and dyld
    // (DYLD_INSERT_LIBRARIES...) load code into the process. MALLOC covers the
    // glibc `MALLOC_*` tunables and the macOS `Malloc*` switches, which can
    // also write log files.
    prefixes: ['LD_', 'DYLD_', 'MALLOC'],
    // What glibc itself strips from privileged programs (unsecvars.h), plus
    // the musl counterpart of LOCPATH: they redirect files libc loads or the
    // resolver it uses.
    names: [
      'GCONV_PATH',
      'GETCONF_DIR',
      'GLIBC_TUNABLES',
      'HOSTALIASES',
      'LOCALDOMAIN',
      'LOCPATH',
      'MUSL_LOCPATH',
      'NIS_PATH',
      'NLSPATH',
      'RESOLV_HOST_CONF',
      'RES_OPTIONS',
      'TZDIR',
    ],
    reason: (name) =>
      `"${name}" changes how the sandbox process itself is loaded and cannot be set`,
  },
]

/**
 * Why a name is refused, or null when the package may receive it. Names are
 * compared ignoring case because Windows does, and nothing legitimate depends
 * on a lower-case spelling of these.
 */
export function reservedEnvironmentNameReason(name: string): string | null {
  const upper = name.toUpperCase()
  for (const group of RESERVED) {
    if (
      group.names?.includes(upper) ||
      group.prefixes?.some((prefix) => upper.startsWith(prefix))
    ) {
      return group.reason(name)
    }
  }
  return null
}

export function isReservedEnvironmentName(name: string) {
  return reservedEnvironmentNameReason(name) !== null
}
