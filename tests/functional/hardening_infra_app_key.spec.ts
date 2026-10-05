import { spawn } from 'node:child_process'
import { readFile } from 'node:fs/promises'
import { test } from '@japa/runner'
import app from '@adonisjs/core/services/app'

async function publishedAppKey(file: string) {
  const match = /^\s*APP_KEY=(\S+)/m.exec(await readFile(app.makePath(file), 'utf8'))
  if (!match) throw new Error(`${file} no longer assigns APP_KEY`)
  return match[1]
}

/**
 * Boots the real HTTP entrypoint as a production server. PORT=0 keeps a
 * regression (a server that does start) off any port another process uses.
 */
function bootProductionServer(appKey: string) {
  return new Promise<{ code: number | null; output: string }>((resolve, reject) => {
    const child = spawn(process.execPath, ['--import=@poppinss/ts-exec', 'bin/server.ts'], {
      cwd: app.makePath(),
      env: {
        ...process.env,
        NODE_ENV: 'production',
        HOST: '127.0.0.1',
        PORT: '0',
        LOG_LEVEL: 'info',
        SESSION_DRIVER: 'cookie',
        APP_URL: 'https://mymcps.example.test',
        APP_KEY: appKey,
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    })

    let output = ''
    child.stdout.on('data', (chunk) => (output += chunk))
    child.stderr.on('data', (chunk) => (output += chunk))

    // A server that boots never exits on its own.
    const deadline = setTimeout(() => child.kill('SIGKILL'), 20_000)
    child.on('error', reject)
    child.on('close', (code) => {
      clearTimeout(deadline)
      resolve({ code, output })
    })
  })
}

test.group('hardening: production boot with a published APP_KEY', () => {
  for (const file of ['Dockerfile', '.env.test']) {
    test(`refuses to start the HTTP server with the APP_KEY from ${file}`, async ({ assert }) => {
      const { code, output } = await bootProductionServer(await publishedAppKey(file))

      assert.equal(code, 1, output)
      assert.include(output, 'Refusing to start: APP_KEY is a placeholder')
      assert.notInclude(output, 'started HTTP server')
    })
  }
})
