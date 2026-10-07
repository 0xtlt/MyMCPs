import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  protected tableName = 'mcps'

  async up() {
    this.schema.alterTable(this.tableName, (table) => {
      // JSON object of tool name to "ask" or "auto". A tool it does not name follows its default.
      table.text('tool_approvals').nullable()
      // Encrypted JSON object of what a built-in MCP needs beyond its sign-in.
      table.text('builtin_settings').nullable()
    })
  }

  async down() {
    this.schema.alterTable(this.tableName, (table) => {
      table.dropColumn('tool_approvals')
      table.dropColumn('builtin_settings')
    })
  }
}
