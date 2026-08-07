import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  async up() {
    this.schema.createTable('oauth_clients', (table) => {
      table.increments('id').notNullable()
      table.string('client_id', 80).notNullable().unique()
      table.string('client_secret_hash', 64).nullable()
      table.string('client_secret_prefix', 20).nullable()
      table.timestamp('client_secret_expires_at').nullable()
      table.string('client_name', 120).notNullable()
      table.text('redirect_uris').notNullable()
      table.string('token_endpoint_auth_method', 32).notNullable()
      table.text('grant_types').notNullable()
      table.text('response_types').notNullable()
      table.string('scope', 500).notNullable()
      table.timestamp('created_at').notNullable()
      table.timestamp('updated_at').nullable()
    })

    this.schema.createTable('oauth_authorization_codes', (table) => {
      table.increments('id').notNullable()
      table.string('code_hash', 64).notNullable().unique()
      table
        .integer('oauth_client_id')
        .unsigned()
        .notNullable()
        .references('id')
        .inTable('oauth_clients')
        .onDelete('CASCADE')
      table
        .integer('user_id')
        .unsigned()
        .notNullable()
        .references('id')
        .inTable('users')
        .onDelete('CASCADE')
      table.string('redirect_uri', 2048).notNullable()
      table.string('code_challenge', 128).notNullable()
      table.string('scopes', 500).notNullable()
      table.string('resource', 2048).notNullable()
      table.timestamp('expires_at').notNullable()
      table.timestamp('created_at').notNullable()
    })

    this.schema.alterTable('access_tokens', (table) => {
      table.string('source', 16).notNullable().defaultTo('manual')
      table
        .integer('oauth_client_id')
        .unsigned()
        .nullable()
        .references('id')
        .inTable('oauth_clients')
        .onDelete('SET NULL')
      table.string('oauth_scopes', 500).nullable()
      table.string('oauth_resource', 2048).nullable()
      table.string('oauth_refresh_token_hash', 64).nullable().unique()
      table.string('oauth_refresh_token_prefix', 20).nullable()
      table.timestamp('oauth_refresh_expires_at').nullable()
    })
  }

  async down() {
    this.schema.alterTable('access_tokens', (table) => {
      table.dropColumn('source')
      table.dropColumn('oauth_client_id')
      table.dropColumn('oauth_scopes')
      table.dropColumn('oauth_resource')
      table.dropColumn('oauth_refresh_token_hash')
      table.dropColumn('oauth_refresh_token_prefix')
      table.dropColumn('oauth_refresh_expires_at')
    })
    this.schema.dropTable('oauth_authorization_codes')
    this.schema.dropTable('oauth_clients')
  }
}
