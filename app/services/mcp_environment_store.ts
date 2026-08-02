import McpSecretStore from '#services/mcp_secret_store'

export type McpEnvironmentInput = {
  name: string
  value: string | null
}

type SecretMap = Record<string, string>

function parseSecretMap(value: string | null | undefined, strict: boolean): SecretMap {
  if (!value) {
    return {}
  }

  try {
    const parsed: unknown = JSON.parse(value)
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      throw new Error('Expected an object')
    }

    const entries = Object.entries(parsed)
    if (entries.some(([, ciphertext]) => typeof ciphertext !== 'string' || !ciphertext)) {
      throw new Error('Expected encrypted string values')
    }
    return Object.fromEntries(entries) as SecretMap
  } catch {
    if (strict) {
      throw new Error('Environment variable configuration is corrupted')
    }
    return {}
  }
}

/**
 * Serializes MCP process environment variables with values encrypted individually.
 */
export default class McpEnvironmentStore {
  static names(value: string | null | undefined) {
    return Object.keys(parseSecretMap(value, false))
  }

  static hasName(value: string | null | undefined, name: string) {
    return Object.hasOwn(parseSecretMap(value, false), name)
  }

  static merge(
    currentValue: string | null | undefined,
    entries: McpEnvironmentInput[]
  ): string | null {
    const current = parseSecretMap(currentValue, false)
    const next: Array<[string, string]> = []

    for (const entry of entries) {
      if (entry.value !== null && entry.value !== '') {
        const encrypted = McpSecretStore.encrypt(entry.value)
        if (encrypted) {
          next.push([entry.name, encrypted])
        }
      } else if (Object.hasOwn(current, entry.name)) {
        next.push([entry.name, current[entry.name]])
      }
    }

    return next.length > 0 ? JSON.stringify(Object.fromEntries(next)) : null
  }

  static decrypt(value: string | null | undefined): Record<string, string> {
    const encrypted = parseSecretMap(value, true)
    const decrypted: Record<string, string> = {}

    for (const [name, ciphertext] of Object.entries(encrypted)) {
      const plaintext = McpSecretStore.decrypt(ciphertext)
      if (plaintext === null) {
        throw new Error(`Environment variable "${name}" could not be decrypted`)
      }
      Object.defineProperty(decrypted, name, {
        value: plaintext,
        enumerable: true,
        configurable: true,
        writable: true,
      })
    }

    return decrypted
  }
}
