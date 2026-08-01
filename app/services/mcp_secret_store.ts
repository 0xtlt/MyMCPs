import encryption from '@adonisjs/core/services/encryption'

/**
 * Encrypt/decrypt MCP secret fields at rest using the app encryption key.
 */
export default class McpSecretStore {
  static encrypt(value: string | null | undefined): string | null {
    if (value === null || value === undefined || value === '') {
      return null
    }
    return encryption.encrypt(value)
  }

  static decrypt(value: string | null | undefined): string | null {
    if (value === null || value === undefined || value === '') {
      return null
    }
    try {
      return encryption.decrypt(value)
    } catch {
      return null
    }
  }

  static mask(value: string | null | undefined): string | null {
    if (!value) {
      return null
    }
    if (value.length <= 4) {
      return '••••'
    }
    return `${'•'.repeat(Math.min(12, value.length - 4))}${value.slice(-4)}`
  }

  static hasSecret(value: string | null | undefined) {
    return Boolean(value)
  }
}
