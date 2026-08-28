import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  async up() {
    this.schema.createTable('mcp_debug_sessions', (table) => {
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
      table.string('status', 16).notNullable().defaultTo('active')
      table
        .integer('created_by')
        .unsigned()
        .nullable()
        .references('id')
        .inTable('users')
        .onDelete('SET NULL')
      table.timestamp('started_at').notNullable()
      table.timestamp('paused_at').nullable()
      table.timestamp('ended_at').nullable()
      table.integer('paused_duration_ms').unsigned().notNullable().defaultTo(0)
      table.integer('state_version').unsigned().notNullable().defaultTo(0)
      table.timestamp('created_at').notNullable()
      table.timestamp('updated_at').nullable()

      table.index(['access_token_id', 'status'])
      table.index(['started_at'])
    })

    this.schema.alterTable('mcp_call_logs', (table) => {
      table
        .integer('debug_session_id')
        .unsigned()
        .nullable()
        .references('id')
        .inTable('mcp_debug_sessions')
        .onDelete('SET NULL')
      table.integer('arguments_bytes').unsigned().notNullable().defaultTo(0)
      table.integer('response_bytes').unsigned().notNullable().defaultTo(0)
      table.boolean('arguments_redacted').notNullable().defaultTo(false)
      table.boolean('response_redacted').notNullable().defaultTo(false)
      table.timestamp('started_at').nullable()
      table.integer('debug_session_elapsed_ms').unsigned().nullable()

      table.index(['debug_session_id', 'started_at'])
    })

    this.defer(async (db) => {
      await db.rawQuery(`
        CREATE UNIQUE INDEX mcp_debug_sessions_one_open_per_token
        ON mcp_debug_sessions (access_token_id)
        WHERE status IN ('active', 'paused')
      `)
    })
  }

  async down() {
    this.schema.alterTable('mcp_call_logs', (table) => {
      table.dropIndex(['debug_session_id', 'started_at'])
      table.dropColumn('debug_session_elapsed_ms')
      table.dropColumn('started_at')
      table.dropColumn('response_redacted')
      table.dropColumn('arguments_redacted')
      table.dropColumn('response_bytes')
      table.dropColumn('arguments_bytes')
      table.dropColumn('debug_session_id')
    })
    this.schema.dropTable('mcp_debug_sessions')
  }
}
