import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  async up() {
    this.schema.alterTable('instance_settings', (table) => {
      table.string('gateway_tool_mode', 16).notNullable().defaultTo('eager')
    })
  }

  async down() {
    this.schema.alterTable('instance_settings', (table) => {
      table.dropColumn('gateway_tool_mode')
    })
  }
}
