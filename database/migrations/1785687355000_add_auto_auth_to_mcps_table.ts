import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  protected tableName = 'mcps'

  async up() {
    this.schema.alterTable(this.tableName, (table) => {
      table.boolean('oauth_required').notNullable().defaultTo(false)
    })

    this.defer(async (db) => {
      await db.rawQuery(`
        UPDATE mcps
        SET oauth_required = CASE
          WHEN auth_type = 'oauth' AND oauth_access_token IS NULL THEN 1
          ELSE 0
        END,
        auth_type = CASE
          WHEN auth_type IN ('none', 'oauth') THEN 'auto'
          ELSE auth_type
        END
      `)
    })

    this.schema.alterTable(this.tableName, (table) => {
      table.string('auth_type', 16).notNullable().defaultTo('auto').alter()
    })
  }

  async down() {
    this.defer(async (db) => {
      await db.rawQuery(`
        UPDATE mcps
        SET auth_type = CASE
          WHEN auth_type = 'auto' AND (
            oauth_required = 1 OR
            oauth_access_token IS NOT NULL OR
            oauth_client_id IS NOT NULL
          ) THEN 'oauth'
          WHEN auth_type = 'auto' THEN 'none'
          ELSE auth_type
        END
      `)
    })

    this.schema.alterTable(this.tableName, (table) => {
      table.string('auth_type', 16).notNullable().defaultTo('none').alter()
      table.dropColumn('oauth_required')
    })
  }
}
