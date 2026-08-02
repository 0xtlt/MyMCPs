import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  async up() {
    this.schema.alterTable('mcp_call_logs', (table) => {
      table.text('response').nullable()
      table.boolean('response_captured').notNullable().defaultTo(false)
    })
  }

  async down() {
    this.schema.alterTable('mcp_call_logs', (table) => {
      table.dropColumn('response_captured')
      table.dropColumn('response')
    })
  }
}
