import { rm } from 'node:fs/promises'
import app from '@adonisjs/core/services/app'
import db from '@adonisjs/lucid/services/db'
import { MigrationRunner } from '@adonisjs/lucid/migration'

const testDatabasePath = app.tmpPath('test.sqlite3')

export async function prepareTestDatabase() {
  await db.manager.closeAll()

  await Promise.all([
    rm(testDatabasePath, { force: true }),
    rm(`${testDatabasePath}-shm`, { force: true }),
    rm(`${testDatabasePath}-wal`, { force: true }),
  ])

  const migrator = new MigrationRunner(db, app, {
    direction: 'up',
    connectionName: 'sqlite',
    disableLocks: true,
  })

  try {
    await migrator.run()
    if (migrator.error) {
      throw migrator.error
    }
  } finally {
    await db.manager.closeAll()
  }
}

export async function beginTestTransaction() {
  await db.beginGlobalTransaction()
}

export async function rollbackTestTransaction() {
  if (db.connectionGlobalTransactions.has('sqlite')) {
    await db.rollbackGlobalTransaction()
  }
}

export async function closeTestDatabase() {
  await rollbackTestTransaction()
  await db.manager.closeAll()
}
