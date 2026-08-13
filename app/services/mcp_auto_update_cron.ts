import { Cron } from 'croner'

export const DEFAULT_MCP_AUTO_UPDATE_CRON = '0 2 * * *'

/**
 * True when `value` is a 5-field cron expression Croner can parse.
 */
export function isValidFiveFieldCron(value: string) {
  const trimmed = value.trim()
  if (!/^(\S+\s+){4}\S+$/.test(trimmed)) {
    return false
  }

  try {
    const job = new Cron(trimmed, {
      paused: true,
      timezone: 'UTC',
      mode: '5-part',
    })
    job.stop()
    return true
  } catch {
    return false
  }
}
