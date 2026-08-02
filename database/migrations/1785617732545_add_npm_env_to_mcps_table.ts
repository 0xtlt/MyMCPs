import { BaseSchema } from '@adonisjs/lucid/schema'

export default class extends BaseSchema {
  protected tableName = 'mcps'

  async up() {
    this.schema.alterTable(this.tableName, (table) => {
      table.text('npm_env').nullable()
    })
  }

  async down() {
    this.schema.alterTable(this.tableName, (table) => {
      table.dropColumn('npm_env')
    })
  }
}
