import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  protected tableName = 'approval_requests'

  async up() {
    this.schema.createTable(this.tableName, (table) => {
      table.increments('id').notNullable()
      // Names the request in its link. Random, so a link cannot be guessed from another.
      table.string('public_id', 64).notNullable().unique()
      table
        .integer('mcp_id')
        .unsigned()
        .notNullable()
        .references('id')
        .inTable('mcps')
        .onDelete('CASCADE')
      table
        .integer('access_token_id')
        .unsigned()
        .notNullable()
        .references('id')
        .inTable('access_tokens')
        .onDelete('CASCADE')
      table.string('tool_name', 254).notNullable()
      // The call as the agent made it, and what MyMCPs read in it. Both encrypted.
      table.text('arguments').notNullable()
      table.string('arguments_hash', 64).notNullable()
      table.text('summary').notNullable()
      table.string('status', 16).notNullable().defaultTo('pending')
      table
        .integer('decided_by')
        .unsigned()
        .nullable()
        .references('id')
        .inTable('users')
        .onDelete('SET NULL')
      table.timestamp('decided_at').nullable()
      // When the agent ran the approved call, or was told about the refusal.
      table.timestamp('consumed_at').nullable()
      table.timestamp('expires_at').notNullable()
      table.timestamp('created_at').notNullable()
      table.timestamp('updated_at').nullable()

      table.index(['access_token_id', 'mcp_id', 'tool_name', 'arguments_hash'])
      table.index(['status', 'expires_at'])
    })
  }

  async down() {
    this.schema.dropTable(this.tableName)
  }
}
