import vine from '@vinejs/vine'

export const logsQueryValidator = vine.create({
  range: vine.enum(['24h', '7d', '30d', 'all'] as const).optional(),
  outcome: vine.enum(['success', 'error'] as const).optional(),
  mcp: vine.string().trim().maxLength(120).optional(),
  token: vine.string().trim().maxLength(16).optional(),
  page: vine.number().withoutDecimals().positive().optional(),
  pageSize: vine.number().withoutDecimals().in([10, 25, 50, 100]).optional(),
  logId: vine.number().withoutDecimals().positive().optional(),
  timeZone: vine.string().trim().maxLength(100).optional(),
})

export const analyticsQueryValidator = vine.create({
  range: vine.enum(['24h', '7d', '30d'] as const).optional(),
  timeZone: vine.string().trim().maxLength(100).optional(),
})
