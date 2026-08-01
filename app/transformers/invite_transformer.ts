import type Invite from '#models/invite'
import { BaseTransformer } from '@adonisjs/core/transformers'

export default class InviteTransformer extends BaseTransformer<Invite> {
  toObject() {
    return {
      ...this.pick(this.resource, [
        'id',
        'email',
        'role',
        'token',
        'acceptedAt',
        'expiresAt',
        'createdAt',
      ]),
      isUsable: this.resource.isUsable,
    }
  }
}
