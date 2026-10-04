import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  protected tableName = 'mcps'

  async up() {
    this.schema.alterTable(this.tableName, (table) => {
      table.string('builtin_username', 254).nullable()
      table.text('builtin_password').nullable()
      table.text('builtin_permissions').nullable()
      table.text('builtin_aliases').nullable()
    })
  }

  async down() {
    this.schema.alterTable(this.tableName, (table) => {
      table.dropColumn('builtin_username')
      table.dropColumn('builtin_password')
      table.dropColumn('builtin_permissions')
      table.dropColumn('builtin_aliases')
    })
  }
}
