import vine from '@vinejs/vine'

export const debugSessionQueryValidator = vine.create({
  sessionId: vine.number().withoutDecimals().positive().optional(),
  callId: vine.number().withoutDecimals().positive().optional(),
  timeZone: vine.string().trim().maxLength(100).optional(),
})

export const startDebugSessionValidator = vine.create({
  accessTokenId: vine.number().withoutDecimals().positive(),
})

export const updateDebugSessionValidator = vine.create({
  action: vine.enum(['pause', 'resume', 'stop'] as const),
})
