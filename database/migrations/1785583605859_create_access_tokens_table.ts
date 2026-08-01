import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  protected tableName = 'access_tokens'

  async up() {
    this.schema.createTable(this.tableName, (table) => {
      table.increments('id').notNullable()
      table.string('name', 120).notNullable()
      table.string('token_prefix', 16).notNullable()
      table.string('token_hash', 64).notNullable().unique()
      table.string('scope_mode', 16).notNullable().defaultTo('all')
      table.timestamp('expires_at').nullable()
      table.timestamp('revoked_at').nullable()
      table.timestamp('last_used_at').nullable()
      table
        .integer('created_by')
        .unsigned()
        .notNullable()
        .references('id')
        .inTable('users')
        .onDelete('CASCADE')
      table.timestamp('created_at').notNullable()
      table.timestamp('updated_at').nullable()
    })
  }

  async down() {
    this.schema.dropTable(this.tableName)
  }
}
