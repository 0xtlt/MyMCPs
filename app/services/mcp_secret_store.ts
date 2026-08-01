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
      // Corrupt or rotated APP_KEY ciphertext — treat as missing secret.
      return null
    }
  }

  static hasSecret(value: string | null | undefined) {
    return Boolean(value)
  }
}
