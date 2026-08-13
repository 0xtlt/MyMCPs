import logger from '@adonisjs/core/services/logger'
import { Cron } from 'croner'
import InstanceSetting from '#models/instance_setting'
import { DEFAULT_MCP_AUTO_UPDATE_CRON, isValidFiveFieldCron } from '#services/mcp_auto_update_cron'
import { updateLatestTrackingMcps } from '#services/mcp_npm_update_service'

let job: Cron | null = null

function shouldSchedule() {
  return process.env.NODE_ENV !== 'test'
}

function stopJob() {
  job?.stop()
  job = null
}

export async function resyncMcpAutoUpdateScheduler() {
  stopJob()

  if (!shouldSchedule()) {
    return
  }

  const settings = await InstanceSetting.current()
  if (!settings.mcpAutoUpdateEnabled) {
    return
  }

  const cron = settings.mcpAutoUpdateCron?.trim() || DEFAULT_MCP_AUTO_UPDATE_CRON
  if (!isValidFiveFieldCron(cron)) {
    logger.error({ cron }, 'Ignoring invalid MCP auto-update cron expression')
    return
  }

  job = new Cron(cron, { timezone: 'UTC', protect: true, mode: '5-part' }, async () => {
    logger.info('Running scheduled npm MCP updates')
    const result = await updateLatestTrackingMcps()
    logger.info(result, 'Scheduled npm MCP updates finished')
  })
}

export async function startMcpAutoUpdateScheduler() {
  await resyncMcpAutoUpdateScheduler()
}

export function stopMcpAutoUpdateScheduler() {
  stopJob()
}
