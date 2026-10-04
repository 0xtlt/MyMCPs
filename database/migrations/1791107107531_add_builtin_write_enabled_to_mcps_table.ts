import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  protected tableName = 'mcps'

  async up() {
    this.schema.alterTable(this.tableName, (table) => {
      table.boolean('builtin_write_enabled').notNullable().defaultTo(false)
    })
  }

  async down() {
    this.schema.alterTable(this.tableName, (table) => {
      table.dropColumn('builtin_write_enabled')
    })
  }
}
