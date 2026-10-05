import { test } from '@japa/runner'
import { createMcpValidator } from '#validators/mcp'
import { flashedRecordIdValidator, flashedTextValidator } from '#validators/session'

const http = { name: 'Docs', transport: 'http', authType: 'auto' }

test.group('validators: MCP endpoint URL', () => {
  test('accepts an HTTP(S) endpoint')
    .with([
      'https://mcp.example.com/mcp',
      'http://127.0.0.1:9999/mcp',
      'https://mcp.example.com/mcp?key=value',
      'https://user:secret@mcp.example.com/mcp',
    ])
    .run(async ({ assert }, httpUrl) => {
      const [error, payload] = await createMcpValidator.tryValidate({ ...http, httpUrl })
      assert.isNull(error)
      assert.equal(payload!.httpUrl, httpUrl)
    })

  test('refuses an endpoint that is not plain HTTP(S)')
    .with([
      { httpUrl: 'ftp://mcp.example.com/mcp', message: 'MCP URL must use HTTP or HTTPS' },
      { httpUrl: 'mcp.example.com/mcp', message: 'MCP URL must be a valid URL' },
      {
        httpUrl: 'https://mcp.example.com/mcp#tools',
        message: 'MCP URL must not include a fragment',
      },
    ])
    .run(async ({ assert }, { httpUrl, message }) => {
      const [error] = await createMcpValidator.tryValidate({ ...http, httpUrl })
      assert.deepEqual(error!.messages, [{ field: 'httpUrl', message, rule: 'mcpEndpointUrl' }])
    })

  test('leaves the field alone for another transport', async ({ assert }) => {
    const [error] = await createMcpValidator.tryValidate({
      name: 'Package',
      transport: 'npm',
      npmPackage: '@example/mcp',
      authType: 'auto',
      httpUrl: 'ftp://mcp.example.com/mcp',
    })
    assert.isNull(error)
  })
})

test.group('validators: flashed values', () => {
  test('a flashed record id is a whole number written by the server')
    .with([
      { value: 12, valid: true },
      { value: '12', valid: false },
      { value: 1.5, valid: false },
      { value: null, valid: false },
      { value: undefined, valid: false },
      { value: { id: 12 }, valid: false },
    ])
    .run(async ({ assert }, { value, valid }) => {
      const [error, id] = await flashedRecordIdValidator.tryValidate(value)
      assert.equal(error === null, valid)
      assert.equal(id, valid ? value : null)
    })

  test('a flashed text is a string')
    .with([
      { value: 'mcp_live_token', valid: true },
      { value: 12, valid: false },
      { value: ['mcp_live_token'], valid: false },
      { value: undefined, valid: false },
    ])
    .run(async ({ assert }, { value, valid }) => {
      const [error, text] = await flashedTextValidator.tryValidate(value)
      assert.equal(error === null, valid)
      assert.equal(text, valid ? value : null)
    })
})
