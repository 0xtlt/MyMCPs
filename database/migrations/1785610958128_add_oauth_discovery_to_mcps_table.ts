import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  protected tableName = 'mcps'

  async up() {
    this.schema.alterTable(this.tableName, (table) => {
      table.string('oauth_issuer', 2048).nullable()
      table.string('oauth_resource', 2048).nullable()
      table.string('oauth_redirect_uri', 2048).nullable()
      table.string('oauth_client_auth_method', 64).nullable()
      table.string('oauth_token_type', 64).nullable()
    })
  }

  async down() {
    this.schema.alterTable(this.tableName, (table) => {
      table.dropColumn('oauth_issuer')
      table.dropColumn('oauth_resource')
      table.dropColumn('oauth_redirect_uri')
      table.dropColumn('oauth_client_auth_method')
      table.dropColumn('oauth_token_type')
    })
  }
}
