import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  protected tableName = 'mcps'

  async up() {
    this.schema.alterTable(this.tableName, (table) => {
      table.string('builtin_key', 64).nullable()
    })
  }

  async down() {
    this.schema.alterTable(this.tableName, (table) => {
      table.dropColumn('builtin_key')
    })
  }
}
