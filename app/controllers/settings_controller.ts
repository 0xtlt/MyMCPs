import {
  updateEmailValidator,
  updateMcpLoggingValidator,
  updatePasswordValidator,
} from '#validators/user'
import { exportBackupValidator } from '#validators/backup'
import type { HttpContext } from '@adonisjs/core/http'
import type { HttpError } from '@adonisjs/core/types/http'
import { createBackupExport } from '#services/backup/export'
import McpCallLogService from '#services/mcp_call_log_service'
import { DEFAULT_MCP_AUTO_UPDATE_CRON } from '#services/mcp_auto_update_cron'
import { resyncMcpAutoUpdateScheduler } from '#services/mcp_auto_update_scheduler'
import { stampSession } from '#services/session_stamp'
import { currentPasswordRateLimiter } from '#start/limiter'
import type User from '#models/user'

/** Marks, for the next page, that the errors of the session are those of the export form. */
const BACKUP_EXPORT_REFUSED = 'backupExportRefused'

/** A client that stops reading must not keep the snapshot of the database on disk for good. */
const STALLED_CLIENT_MS = 60_000

/**
 * Whether a form was refused for what it holds, by a validator or by the
 * check of the current password, whose error is of no class.
 */
function isValidationError(error: unknown): error is HttpError & { messages: unknown[] } {
  return (
    error instanceof Error &&
    'code' in error &&
    error.code === 'E_VALIDATION_ERROR' &&
    'messages' in error &&
    Array.isArray(error.messages)
  )
}

export default class SettingsController {
  async index({ inertia, auth, request, session }: HttpContext) {
    const mcpLogging = auth.user!.isAdmin ? await McpCallLogService.settings() : null
    return inertia.render('settings/index', {
      /**
       * The export answers with a file, which only a form posted by the
       * browser itself can save: that form carries the CSRF token as a field.
       */
      backup: auth.user!.isAdmin
        ? {
            csrfToken: request.csrfToken,
            exportRefused: session.flashMessages.has(BACKUP_EXPORT_REFUSED),
          }
        : null,
      mcpLogging: mcpLogging
        ? {
            gatewayToolMode: mcpLogging.gatewayToolMode,
            level: mcpLogging.mcpLogLevel,
            retentionDays: mcpLogging.mcpLogRetentionDays,
            autoUpdateEnabled: mcpLogging.mcpAutoUpdateEnabled,
            autoUpdateCron: mcpLogging.mcpAutoUpdateCron || DEFAULT_MCP_AUTO_UPDATE_CRON,
          }
        : null,
    })
  }

  async updateEmail({ request, auth, response, session }: HttpContext) {
    const user = auth.user!
    const payload = await request.validateUsing(updateEmailValidator, {
      meta: { userId: user.id },
    })

    await this.confirmCurrentPassword(user, payload.currentPassword)
    user.email = payload.email
    await user.save()

    session.flash('success', 'Email updated')
    return response.redirect().toRoute('settings.index')
  }

  async updatePassword({ request, auth, response, session }: HttpContext) {
    const user = auth.user!
    const payload = await request.validateUsing(updatePasswordValidator)

    await this.confirmCurrentPassword(user, payload.currentPassword)
    await user.changePassword(payload.newPassword)

    // The change retired every session of the account: keep this browser signed in.
    stampSession(session, user)

    session.flash('success', 'Password updated')
    return response.redirect().toRoute('settings.index')
  }

  async updateMcpLogging({ request, auth, response, session }: HttpContext) {
    const payload = await request.validateUsing(updateMcpLoggingValidator)
    const settings = await McpCallLogService.settings()
    settings.gatewayToolMode = payload.gatewayToolMode
    settings.mcpLogLevel = payload.mcpLogLevel
    settings.mcpLogRetentionDays = payload.mcpLogRetentionDays
    settings.mcpAutoUpdateEnabled = payload.mcpAutoUpdateEnabled ?? false
    if (payload.mcpAutoUpdateCron) {
      settings.mcpAutoUpdateCron = payload.mcpAutoUpdateCron
    }
    settings.updatedBy = auth.user!.id
    await settings.save()
    await McpCallLogService.pruneExpired({ force: true })
    await resyncMcpAutoUpdateScheduler()

    session.flash('success', 'Instance settings updated')
    return response.redirect().toRoute('settings.index')
  }

  /**
   * Send the whole instance as one encrypted file: a snapshot of the
   * database with the key its secrets are encrypted with, under a password
   * chosen for this file. It holds every credential of the instance, so the
   * password of the account is asked again.
   */
  async exportBackup({ request, auth, response, session, logger }: HttpContext) {
    const user = auth.user!

    let payload
    try {
      payload = await request.validateUsing(exportBackupValidator)
      await this.confirmCurrentPassword(user, payload.currentPassword)
    } catch (error) {
      if (!isValidationError(error)) throw error
      // The form is posted by the browser, not by the page: the errors go
      // back with a redirect, and the passwords that were typed stay out of
      // the session.
      session.flashValidationErrors(error, false)
      session.flash(BACKUP_EXPORT_REFUSED, true)
      return response.redirect().toRoute('settings.index')
    }

    const backup = await createBackupExport(payload.password)
    // The snapshot goes when the file has been sent, or the client has gone.
    response.onFinish(() => {
      backup.discard().catch((error) => {
        logger.warn({ err: error }, 'Could not delete the snapshot of an exported backup')
      })
    })
    logger.info({ userId: user.id, email: user.email, bytes: backup.size }, 'Backup exported')

    response.header('Content-Type', 'application/octet-stream')
    response.header('Content-Disposition', `attachment; filename="${backup.fileName}"`)
    response.header('Cache-Control', 'no-store')
    response.header('Content-Length', backup.size)
    response.response.setTimeout(STALLED_CLIENT_MS)
    return response.stream(backup.content)
  }

  /**
   * A signed-in browser is no proof of knowing the password: without a
   * budget, whoever holds a hijacked session could guess it here at will.
   */
  private async confirmCurrentPassword(user: User, currentPassword: string) {
    const key = `current-password:${user.id}`

    await currentPasswordRateLimiter.consume(key)
    await user.validatePassword(currentPassword)
    await currentPasswordRateLimiter.delete(key)
  }
}
