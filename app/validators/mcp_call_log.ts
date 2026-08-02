import vine from '@vinejs/vine'

export const logsQueryValidator = vine.create({
  range: vine.enum(['24h', '7d', '30d', 'all'] as const).optional(),
  outcome: vine.enum(['success', 'error'] as const).optional(),
  mcp: vine.string().trim().maxLength(120).optional(),
  token: vine.string().trim().maxLength(16).optional(),
  page: vine.number().withoutDecimals().positive().optional(),
  logId: vine.number().withoutDecimals().positive().optional(),
})

export const analyticsQueryValidator = vine.create({
  range: vine.enum(['24h', '7d', '30d'] as const).optional(),
})
