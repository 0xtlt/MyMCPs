import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  protected tableName = 'users'

  async up() {
    this.schema.alterTable(this.tableName, (table) => {
      table.string('role', 32).notNullable().defaultTo('member')
    })

    /**
     * Pre-role installs treated every local user as an operator.
     * Promote them to admin so upgrades are never left without one.
     */
    this.defer(async (db) => {
      await db.from(this.tableName).update({ role: 'admin' })
    })
  }

  async down() {
    this.schema.alterTable(this.tableName, (table) => {
      table.dropColumn('role')
    })
  }
}
