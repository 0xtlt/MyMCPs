import logger from '@adonisjs/core/services/logger'
import { Cron } from 'croner'
import InstanceSetting from '#models/instance_setting'
import { DEFAULT_MCP_AUTO_UPDATE_CRON, isValidFiveFieldCron } from '#services/mcp_auto_update_cron'
import { updateLatestTrackingMcps } from '#services/mcp_npm_update_service'

/**
 * In-process Croner job for instance-wide Deno npm MCP auto-updates.
 *
 * MyMCPs is a single Node process with SQLite, so this runs inside the web
 * server rather than a separate worker. The provider starts it on boot and
 * settings saves call `resyncMcpAutoUpdateScheduler()` so cron/enablement
 * changes take effect without a restart.
 *
 * The job only reloads npm MCPs that already track `latest`. Pinned versions
 * and HTTP MCPs are never touched. Expressions are 5-field cron in UTC
 * (`0 2 * * *` = 02:00 UTC). Tests skip scheduling (`NODE_ENV=test`).
 */
let job: Cron | null = null

function shouldSchedule() {
  return process.env.NODE_ENV !== 'test'
}

function stopJob() {
  job?.stop()
  job = null
}

/**
 * Replace the running job with whatever instance settings currently say.
 * Stops the job when auto-update is off or the cron expression is invalid.
 */
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

  // `protect` skips a tick when the previous run is still reloading caches.
  job = new Cron(cron, { timezone: 'UTC', protect: true, mode: '5-part' }, async () => {
    logger.info('Running scheduled npm MCP updates')
    const result = await updateLatestTrackingMcps()
    logger.info(result, 'Scheduled npm MCP updates finished')
  })
}

/**
 * Start (or refresh) the job from current instance settings. Called once
 * when the web provider becomes ready.
 */
export async function startMcpAutoUpdateScheduler() {
  await resyncMcpAutoUpdateScheduler()
}

/**
 * Stop the job on process shutdown so Croner timers do not outlive the app.
 */
export function stopMcpAutoUpdateScheduler() {
  stopJob()
}
