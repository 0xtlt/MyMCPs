import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  async up() {
    this.schema.createTable('oauth_refresh_token_history', (table) => {
      table.increments('id').notNullable()
      table
        .integer('access_token_id')
        .unsigned()
        .notNullable()
        .references('id')
        .inTable('access_tokens')
        .onDelete('CASCADE')
      table.string('token_hash', 64).notNullable().unique()
      table.timestamp('invalidated_at').notNullable()
      table.index(['access_token_id'])
    })
  }

  async down() {
    this.schema.dropTable('oauth_refresh_token_history')
  }
}
