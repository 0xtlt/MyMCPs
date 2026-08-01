import { BaseSchema } from '@adonisjs/lucid/schema'

/**
 * Repair installs that already ran the role migration with defaultTo('member')
 * and were left with users but no admin.
 */
export default class extends BaseSchema {
  protected tableName = 'users'

  async up() {
    this.defer(async (db) => {
      const admin = await db.from(this.tableName).where('role', 'admin').first()
      if (admin) {
        return
      }

      const firstUser = await db.from(this.tableName).orderBy('id', 'asc').first()
      if (!firstUser) {
        return
      }

      await db.from(this.tableName).where('id', firstUser.id).update({ role: 'admin' })
    })
  }

  async down() {
    // Irreversible data repair
  }
}
