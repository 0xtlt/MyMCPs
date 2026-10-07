import { ApprovalRequestSchema } from '#database/schema'
import { belongsTo } from '@adonisjs/lucid/orm'
import type { BelongsTo } from '@adonisjs/lucid/types/relations'
import { DateTime } from 'luxon'
import AccessToken from '#models/access_token'
import Mcp from '#models/mcp'
import User from '#models/user'

/** What a person decided. `pending` until they have. */
export type ApprovalDecision = 'pending' | 'approved' | 'denied'

/**
 * Where a request stands for whoever reads it: `used` once the agent ran the
 * approved call, and `expired` when nobody decided in time or the approved
 * call was never run.
 */
export type ApprovalState = 'pending' | 'approved' | 'used' | 'denied' | 'expired'

/**
 * A tool call an agent made that waits for a person, or has been decided. It
 * keeps the call exactly as the agent made it: an approval is for that call
 * and no other.
 */
export default class ApprovalRequest extends ApprovalRequestSchema {
  declare status: ApprovalDecision

  @belongsTo(() => Mcp)
  declare mcp: BelongsTo<typeof Mcp>

  @belongsTo(() => AccessToken)
  declare accessToken: BelongsTo<typeof AccessToken>

  @belongsTo(() => User, { foreignKey: 'decidedBy' })
  declare decider: BelongsTo<typeof User>

  get isExpired() {
    return this.expiresAt <= DateTime.utc()
  }

  get state(): ApprovalState {
    if (this.status === 'denied') return 'denied'
    if (this.status === 'approved' && this.consumedAt) return 'used'
    return this.isExpired ? 'expired' : this.status
  }
}
