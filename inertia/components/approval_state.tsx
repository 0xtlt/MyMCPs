import { Token } from '@astryxdesign/core/Token'

/** Where an approval request stands: `used` once the agent ran the approved call. */
export type ApprovalState = 'pending' | 'approved' | 'used' | 'denied' | 'expired'

/** An approval request as the pages list it. */
export type ApprovalRow = {
  id: string
  state: ApprovalState
  toolName: string
  /** What MyMCPs read in the call. `null` when it can no longer be decrypted. */
  title: string | null
  interpreted: boolean
  mcp: { id: number; name: string; slug: string }
  accessToken: { name: string; prefix: string }
  createdAt: string
  expiresAt: string
  decidedAt: string | null
  decidedBy: string | null
  consumedAt: string | null
}

const states: Record<ApprovalState, { label: string; color: 'yellow' | 'green' | 'red' | 'gray' }> =
  {
    pending: { label: 'Waiting', color: 'yellow' },
    approved: { label: 'Approved', color: 'green' },
    used: { label: 'Approved and run', color: 'green' },
    denied: { label: 'Denied', color: 'red' },
    expired: { label: 'Expired', color: 'gray' },
  }

export function ApprovalStateToken({ state }: { state: ApprovalState }) {
  return <Token label={states[state].label} color={states[state].color} size="sm" />
}
