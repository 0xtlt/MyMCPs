import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  protected tableName = 'access_token_mcps'

  async up() {
    this.schema.createTable(this.tableName, (table) => {
      table.increments('id').notNullable()
      table
        .integer('access_token_id')
        .unsigned()
        .notNullable()
        .references('id')
        .inTable('access_tokens')
        .onDelete('CASCADE')
      table
        .integer('mcp_id')
        .unsigned()
        .notNullable()
        .references('id')
        .inTable('mcps')
        .onDelete('CASCADE')
      table.unique(['access_token_id', 'mcp_id'])
      table.timestamp('created_at').notNullable()
      table.timestamp('updated_at').nullable()
    })
  }

  async down() {
    this.schema.dropTable(this.tableName)
  }
}
