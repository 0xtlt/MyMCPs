import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  protected tableName = 'mcp_call_logs'

  async up() {
    this.schema.alterTable(this.tableName, (table) => {
      table.string('caller_ip', 64).nullable()
    })
  }

  async down() {
    this.schema.alterTable(this.tableName, (table) => {
      table.dropColumn('caller_ip')
    })
  }
}
