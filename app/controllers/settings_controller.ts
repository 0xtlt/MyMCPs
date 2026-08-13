import {
  updateEmailValidator,
  updateMcpLoggingValidator,
  updatePasswordValidator,
} from '#validators/user'
import type { HttpContext } from '@adonisjs/core/http'
import McpCallLogService from '#services/mcp_call_log_service'
import { DEFAULT_MCP_AUTO_UPDATE_CRON } from '#services/mcp_auto_update_cron'
import { resyncMcpAutoUpdateScheduler } from '#services/mcp_auto_update_scheduler'
import User from '#models/user'

export default class SettingsController {
  async index({ inertia, auth }: HttpContext) {
    const mcpLogging = auth.user!.isAdmin ? await McpCallLogService.settings() : null
    return inertia.render('settings/index', {
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

    await user.validatePassword(payload.currentPassword)
    user.email = payload.email
    await user.save()

    session.flash('success', 'Email updated')
    return response.redirect().toRoute('settings.index')
  }

  async updatePassword({ request, auth, response, session }: HttpContext) {
    const user = auth.user!
    const payload = await request.validateUsing(updatePasswordValidator)

    await user.validatePassword(payload.currentPassword)
    user.password = payload.newPassword
    await user.save()

    const rememberTokens = await User.rememberMeTokens.all(user)
    await Promise.all(
      rememberTokens.map((token) => User.rememberMeTokens.delete(user, token.identifier))
    )

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
}
