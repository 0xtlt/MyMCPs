import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  async up() {
    this.schema.createTable('instance_settings', (table) => {
      table.increments('id').notNullable()
      table.string('mcp_log_level', 16).notNullable().defaultTo('metadata')
      table.integer('mcp_log_retention_days').unsigned().notNullable().defaultTo(14)
      table
        .integer('updated_by')
        .unsigned()
        .nullable()
        .references('id')
        .inTable('users')
        .onDelete('SET NULL')
      table.timestamp('created_at').notNullable()
      table.timestamp('updated_at').nullable()
    })

    this.schema.createTable('mcp_call_logs', (table) => {
      table.increments('id').notNullable()
      table
        .integer('access_token_id')
        .unsigned()
        .nullable()
        .references('id')
        .inTable('access_tokens')
        .onDelete('SET NULL')
      table.string('access_token_name', 120).notNullable()
      table.string('access_token_prefix', 16).notNullable()
      table
        .integer('mcp_id')
        .unsigned()
        .nullable()
        .references('id')
        .inTable('mcps')
        .onDelete('SET NULL')
      table.string('mcp_name', 120).nullable()
      table.string('mcp_slug', 120).nullable()
      table.string('requested_tool_name', 512).notNullable()
      table.string('tool_name', 254).nullable()
      table.string('outcome', 16).notNullable()
      table.string('error_category', 32).nullable()
      table.string('error_summary', 500).nullable()
      table.text('arguments').nullable()
      table.boolean('arguments_captured').notNullable().defaultTo(false)
      table.integer('duration_ms').unsigned().notNullable()
      table.timestamp('created_at').notNullable()

      table.index(['created_at'])
      table.index(['outcome', 'created_at'])
      table.index(['mcp_slug', 'created_at'])
      table.index(['access_token_prefix', 'created_at'])
    })

    this.defer(async (db) => {
      await db.table('instance_settings').insert({
        id: 1,
        mcp_log_level: 'metadata',
        mcp_log_retention_days: 14,
        created_at: new Date().toISOString().replace('T', ' ').replace('Z', ''),
      })
    })
  }

  async down() {
    this.schema.dropTable('mcp_call_logs')
    this.schema.dropTable('instance_settings')
  }
}
