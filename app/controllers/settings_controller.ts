import {
  updateEmailValidator,
  updateMcpLoggingValidator,
  updatePasswordValidator,
} from '#validators/user'
import type { HttpContext } from '@adonisjs/core/http'
import McpCallLogService from '#services/mcp_call_log_service'

export default class SettingsController {
  async index({ inertia, auth }: HttpContext) {
    const mcpLogging = auth.user!.isAdmin ? await McpCallLogService.settings() : null
    return inertia.render('settings/index', {
      mcpLogging: mcpLogging
        ? {
            level: mcpLogging.mcpLogLevel,
            retentionDays: mcpLogging.mcpLogRetentionDays,
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

    session.flash('success', 'Password updated')
    return response.redirect().toRoute('settings.index')
  }

  async updateMcpLogging({ request, auth, response, session }: HttpContext) {
    const payload = await request.validateUsing(updateMcpLoggingValidator)
    const settings = await McpCallLogService.settings()
    settings.mcpLogLevel = payload.mcpLogLevel
    settings.mcpLogRetentionDays = payload.mcpLogRetentionDays
    settings.updatedBy = auth.user!.id
    await settings.save()
    await McpCallLogService.pruneExpired({ force: true })

    session.flash('success', 'MCP logging settings updated')
    return response.redirect().toRoute('settings.index')
  }
}
