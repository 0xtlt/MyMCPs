import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  async up() {
    this.schema.alterTable('instance_settings', (table) => {
      table.boolean('mcp_auto_update_enabled').notNullable().defaultTo(false)
      table.string('mcp_auto_update_cron', 64).notNullable().defaultTo('0 2 * * *')
    })
  }

  async down() {
    this.schema.alterTable('instance_settings', (table) => {
      table.dropColumn('mcp_auto_update_enabled')
      table.dropColumn('mcp_auto_update_cron')
    })
  }
}
