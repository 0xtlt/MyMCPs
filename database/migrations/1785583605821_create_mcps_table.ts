import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  protected tableName = 'mcps'

  async up() {
    this.schema.createTable(this.tableName, (table) => {
      table.increments('id').notNullable()
      table.string('name', 120).notNullable()
      table.string('slug', 120).notNullable().unique()
      table.string('description', 500).nullable()
      table.string('transport', 16).notNullable()
      table.string('http_url', 2048).nullable()
      table.string('npm_package', 254).nullable()
      table.string('npm_version', 64).nullable()
      table.text('npm_args').nullable()
      table.string('auth_type', 16).notNullable().defaultTo('none')
      table.text('auth_bearer').nullable()
      table.string('auth_header_name', 120).nullable()
      table.text('auth_header_value').nullable()
      table.string('oauth_authorize_url', 2048).nullable()
      table.string('oauth_token_url', 2048).nullable()
      table.string('oauth_scopes', 500).nullable()
      table.string('oauth_client_id', 254).nullable()
      table.text('oauth_client_secret').nullable()
      table.text('oauth_access_token').nullable()
      table.text('oauth_refresh_token').nullable()
      table.timestamp('oauth_token_expires_at').nullable()
      table.string('status', 16).notNullable().defaultTo('draft')
      table.text('last_error').nullable()
      table.boolean('enabled').notNullable().defaultTo(true)
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
