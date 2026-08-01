import type User from '#models/user'
import { BaseTransformer } from '@adonisjs/core/transformers'

/**
 * Team member row for the invites admin page.
 */
export default class MemberTransformer extends BaseTransformer<User> {
  #currentUserId: number

  constructor(user: User, currentUserId: number) {
    super(user)
    this.#currentUserId = currentUserId
  }

  toObject() {
    return {
      ...this.pick(this.resource, ['id', 'email', 'fullName', 'role', 'createdAt']),
      isCurrentUser: this.resource.id === this.#currentUserId,
    }
  }
}
