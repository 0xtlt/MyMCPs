import { test } from '@japa/runner'
import db from '@adonisjs/lucid/services/db'
import { HttpContextFactory } from '@adonisjs/core/factories/http'
import HttpExceptionHandler from '#exceptions/handler'
import { createAdmin } from '#tests/helpers/factories'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'

/**
 * The handler as it behaves in production, where debug mode is off.
 */
class ProductionExceptionHandler extends HttpExceptionHandler {
  protected debug = false
}

async function render(error: unknown, accept: string) {
  const ctx = new HttpContextFactory().create()
  ctx.request.request.headers.accept = accept
  await new ProductionExceptionHandler().handle(error, ctx)

  return { status: ctx.response.getStatus(), body: ctx.response.getBody() }
}

async function queryError(run: () => Promise<unknown>) {
  try {
    await run()
  } catch (error) {
    return error as Error
  }
  throw new Error('Expected the query to fail')
}

test.group('hardening: error disclosure', (group) => {
  group.each.setup(beginTestTransaction)
  group.each.teardown(rollbackTestTransaction)

  test('leaves bound values out of a failed query message', async ({ assert }) => {
    await createAdmin({ email: 'taken@example.com' })

    const error = await queryError(() =>
      db.table('users').insert({
        email: 'taken@example.com',
        password: 'bound-secret-value',
        role: 'member',
        created_at: new Date().toISOString(),
      })
    )

    assert.include(error.message, 'UNIQUE constraint failed')
    assert.include(error.message, 'insert into `users`')
    assert.notInclude(error.message, 'bound-secret-value')
    assert.notInclude(error.message, 'taken@example.com')
  })

  test('answers API clients with a generic message for server errors in production')
    .with([
      { accept: 'application/json', body: { message: 'Internal server error' } },
      {
        accept: 'application/vnd.api+json',
        body: { errors: [{ title: 'Internal server error', status: 500 }] },
      },
    ])
    .run(async ({ assert }, { accept, body }) => {
      const error = await queryError(() => db.from('users').where('email', 'a@b.c').select('nope'))
      assert.include(error.message, 'select `nope` from `users`')

      const response = await render(error, accept)

      assert.equal(response.status, 500)
      assert.deepEqual(response.body, body)
    })

  test('keeps the message of client errors in production', async ({ assert }) => {
    const error = Object.assign(new Error('Row not found'), { status: 404 })

    const response = await render(error, 'application/json')

    assert.equal(response.status, 404)
    assert.deepEqual(response.body, { message: 'Row not found' })
  })
})
