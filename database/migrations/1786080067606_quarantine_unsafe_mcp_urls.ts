import { BaseSchema } from '@adonisjs/lucid/schema'
import { ensureSafeHttpUrl } from '#services/http_url'

const QUARANTINE_MESSAGE =
  'This legacy MCP URL was cleared because it contained unsafe credentials or URL syntax. Re-enter the endpoint and use encrypted authentication fields for secrets.'

/**
 * Existing installs may contain URL credentials accepted before URL hardening.
 * Disable and clear those endpoints so they cannot be sent or displayed after
 * upgrade. Operators can safely re-enter the endpoint through normal validation.
 */
export default class extends BaseSchema {
  protected tableName = 'mcps'

  async up() {
    this.defer(async (db) => {
      // Diagnostics persisted before secret-aware redaction may already contain
      // upstream-controlled credential text. They cannot be safely repaired in
      // place, so discard them during the security upgrade.
      await db.from(this.tableName).whereNotNull('last_error').update({ last_error: null })
      await db.from('mcp_call_logs').whereNotNull('error_summary').update({ error_summary: null })

      const mcps = await db
        .from(this.tableName)
        .where('transport', 'http')
        .select(['id', 'http_url'])

      const unsafeIds = mcps.flatMap((mcp) => {
        try {
          ensureSafeHttpUrl(String(mcp.http_url ?? ''), 'MCP URL')
          return []
        } catch {
          return [mcp.id]
        }
      })

      if (unsafeIds.length > 0) {
        await db.from(this.tableName).whereIn('id', unsafeIds).update({
          http_url: null,
          enabled: false,
          status: 'draft',
          last_error: QUARANTINE_MESSAGE,
        })
      }
    })
  }

  async down() {
    // Irreversible security remediation: discarded plaintext URL credentials
    // and legacy diagnostics must not be restored by a rollback.
  }
}
