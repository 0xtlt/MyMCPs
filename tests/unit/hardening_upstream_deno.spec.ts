import { chmod, mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { isAbsolute, join } from 'node:path'
import { test } from '@japa/runner'
import { errors } from '@vinejs/vine'
import app from '@adonisjs/core/services/app'
import type Mcp from '#models/mcp'
import McpEnvironmentStore from '#services/mcp_environment_store'
import { reservedEnvironmentNameReason } from '#services/mcp_environment_policy'
import {
  buildDenoArgs,
  buildDenoEnvironment,
  denoRuntime,
  listDenoTools,
  locateDenoBinary,
  reloadDenoNpmPackageCache,
  removeMcpSandbox,
  resetDenoRuntime,
  resolveDenoDir,
  sandboxRootFor,
} from '#services/upstream/deno_runner'
import { createMcpValidator } from '#validators/mcp'
import { beginTestTransaction, rollbackTestTransaction } from '#tests/helpers/database'
import { createAdmin, createMcp } from '#tests/helpers/factories'

/**
 * Stands in for the `deno` binary, which CI does not install. As `deno run` it
 * is a minimal MCP stdio server whose only tool describes the process it runs
 * in; as `deno cache` it records the same description and exits.
 */
const FAKE_DENO = `#!${process.execPath}
const fs = require('node:fs')
const names = ['PATH', 'HOME', 'TMPDIR', 'DENO_DIR', 'NO_COLOR', 'API_KEY', 'NPM_CONFIG_REGISTRY',
  'LD_PRELOAD', 'DYLD_INSERT_LIBRARIES', 'DENO_V8_FLAGS', 'NODE_PATH']
const snapshot = {
  argv: process.argv.slice(2),
  cwd: process.cwd(),
  env: Object.fromEntries(names.map((name) => [name, process.env[name] ?? null])),
}
function writeAll(fd, data) {
  let offset = 0
  while (offset < data.length) {
    try {
      offset += fs.writeSync(fd, data, offset)
    } catch (error) {
      if (error.code !== 'EAGAIN') throw error
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 5)
    }
  }
}
if (snapshot.argv[0] === 'cache') {
  fs.writeFileSync('reload.json', JSON.stringify(snapshot))
  process.exit(0)
}
if (process.env.FAKE_BEHAVIOUR === 'refuse-key') {
  writeAll(2, Buffer.from('error: cannot start, the key ' + process.env.API_KEY + ' was refused\\n'))
  process.exit(3)
}
if (process.env.FAKE_BEHAVIOUR === 'flood-then-fail') {
  writeAll(2, Buffer.alloc(256 * 1024, 'x'))
  process.exit(3)
}
if (process.env.FAKE_BEHAVIOUR === 'noisy') {
  writeAll(2, Buffer.alloc(512 * 1024, 'e'))
}
function send(message) {
  process.stdout.write(JSON.stringify(message) + '\\n')
}
let buffer = ''
process.stdin.on('data', (chunk) => {
  buffer += chunk
  for (let end = buffer.indexOf('\\n'); end >= 0; end = buffer.indexOf('\\n')) {
    const line = buffer.slice(0, end).trim()
    buffer = buffer.slice(end + 1)
    if (!line) continue
    const message = JSON.parse(line)
    if (message.method === 'initialize') {
      send({ jsonrpc: '2.0', id: message.id, result: {
        protocolVersion: message.params.protocolVersion,
        capabilities: { tools: {} },
        serverInfo: { name: 'fake-deno', version: '1.0.0' },
      } })
    } else if (message.method === 'tools/list') {
      send({ jsonrpc: '2.0', id: message.id, result: { tools: [
        { name: 'snapshot', description: JSON.stringify(snapshot), inputSchema: { type: 'object' } },
      ] } })
    }
  }
})
process.stdin.on('end', () => process.exit(0))
`

type Snapshot = { argv: string[]; cwd: string; env: Record<string, string | null> }

async function executable(directory: string, name: string, content: string) {
  await mkdir(directory, { recursive: true })
  const path = join(directory, name)
  await writeFile(path, content)
  await chmod(path, 0o755)
  return path
}

async function npmPayload(npmEnv: Array<{ name: string; value: string }>) {
  return createMcpValidator.validate({
    name: 'Environment MCP',
    description: '',
    transport: 'npm',
    httpUrl: '',
    npmPackage: '@example/environment-mcp',
    npmVersion: '',
    npmArgs: '',
    npmEnv,
    authType: 'auto',
    enabled: true,
  })
}

async function snapshotOf(mcp: Mcp): Promise<Snapshot> {
  const [tool] = await listDenoTools(mcp)
  return JSON.parse(tool.description!)
}

test.group('npm MCP environment names', () => {
  test('refuses names that steer the binary lookup, the loader or the Deno runtime', async ({
    assert,
  }) => {
    const refused = [
      'PATH',
      'Path',
      'PATHEXT',
      'HOME',
      'TMPDIR',
      'NO_COLOR',
      'LD_PRELOAD',
      'LD_LIBRARY_PATH',
      'ld_audit',
      'DYLD_INSERT_LIBRARIES',
      'DYLD_LIBRARY_PATH',
      'DENO_DIR',
      'DENO_V8_FLAGS',
      'DENO_CERT',
      'deno_auth_tokens',
      'NODE_OPTIONS',
      'NODE_PATH',
      'GLIBC_TUNABLES',
      'GCONV_PATH',
      'HOSTALIASES',
      'LOCPATH',
      'MALLOC_CHECK_',
      'MallocLogFile',
      'RES_OPTIONS',
    ]
    for (const name of refused) {
      let failure: unknown
      try {
        await npmPayload([{ name, value: 'x' }])
      } catch (error) {
        failure = error
      }
      assert.instanceOf(failure, errors.E_VALIDATION_ERROR, `${name} was accepted`)
      const [message] = (failure as InstanceType<typeof errors.E_VALIDATION_ERROR>).messages
      assert.equal(message.field, 'npmEnv.0.name')
      assert.equal(message.rule, 'npmEnvironmentName')
      assert.include(message.message, `"${name}"`)
      assert.include(message.message, 'cannot be')
    }
  })

  test('keeps ordinary application variables usable', async ({ assert }) => {
    const allowed = [
      'API_KEY',
      'DATABASE_URL',
      'NPM_CONFIG_REGISTRY',
      'NODE_ENV',
      'NODE_EXTRA_CA_CERTS',
      'HTTPS_PROXY',
      'NO_PROXY',
      'TZ',
      'LANG',
      'XDG_CONFIG_HOME',
      // Near misses of the reserved names and prefixes.
      'PATH_PREFIX',
      'HOMEPAGE_URL',
      'LDAP_URL',
      'DENOMINATION',
      'DYLDX',
    ]
    const payload = await npmPayload(allowed.map((name) => ({ name, value: 'value' })))

    assert.deepEqual(
      payload.npmEnv?.map((entry) => entry.name),
      allowed
    )
    for (const name of allowed) {
      assert.isNull(reservedEnvironmentNameReason(name))
    }
  })
})

test.group('Deno binary resolution', (group) => {
  let directory: string

  group.each.setup(async () => {
    directory = await mkdtemp(join(tmpdir(), 'mymcps-deno-binary-'))
  })
  group.each.teardown(async () => {
    await rm(directory, { recursive: true, force: true })
  })

  test('prefers DENO_PATH, then the server PATH, then the usual locations', async ({ assert }) => {
    const configured = await executable(join(directory, 'configured'), 'deno', FAKE_DENO)
    const onPath = await executable(join(directory, 'bin'), 'deno', FAKE_DENO)
    const known = await executable(join(directory, 'known'), 'deno', FAKE_DENO)
    const searchPath = [join(directory, 'empty'), join(directory, 'bin')].join(':')

    assert.equal(locateDenoBinary(configured, searchPath, [known]), configured)
    // A DENO_PATH that does not exist falls back instead of being spawned.
    assert.equal(locateDenoBinary(join(directory, 'missing', 'deno'), searchPath, [known]), onPath)
    assert.equal(locateDenoBinary('', searchPath, [known]), onPath)
    assert.equal(locateDenoBinary('', join(directory, 'empty'), [known]), known)
    assert.equal(locateDenoBinary(undefined, undefined, [known]), known)
  })

  test('never answers with a bare or relative command', async ({ assert }) => {
    await executable(join(directory, 'bin'), 'deno', FAKE_DENO)
    await writeFile(join(directory, 'not-executable'), '')
    await mkdir(join(directory, 'folder', 'deno'), { recursive: true })

    // Relative PATH entries would be searched from the sandbox working directory.
    const relativeOnly = ['', '.', 'bin', 'node_modules/.bin'].join(':')
    assert.throws(() => locateDenoBinary('', relativeOnly, []), /Deno was not found.*DENO_PATH/)
    assert.throws(() => locateDenoBinary('deno', '', []), /Deno was not found/)
    // Neither a plain file without the executable bit nor a directory called deno.
    assert.throws(
      () => locateDenoBinary(join(directory, 'not-executable'), join(directory, 'folder'), []),
      /Deno was not found/
    )

    const found = locateDenoBinary('', `bin:${join(directory, 'bin')}`, [])
    assert.isTrue(isAbsolute(found))
    assert.equal(found, join(directory, 'bin', 'deno'))
  })
})

test.group('Deno sandbox process', (group) => {
  let directory: string
  let previousDenoDir: string | undefined
  const sandboxes: number[] = []

  group.each.setup(async () => {
    await beginTestTransaction()
    directory = await mkdtemp(join(tmpdir(), 'mymcps-deno-sandbox-'))
    previousDenoDir = process.env.DENO_DIR
    process.env.DENO_DIR = join(directory, 'deno-dir')
    const fake = await executable(join(directory, 'bin'), 'deno', FAKE_DENO)
    denoRuntime.binary = () => fake
  })
  group.each.teardown(async () => {
    resetDenoRuntime()
    if (previousDenoDir === undefined) {
      delete process.env.DENO_DIR
    } else {
      process.env.DENO_DIR = previousDenoDir
    }
    await Promise.all(sandboxes.splice(0).map((id) => removeMcpSandbox(id)))
    await rm(directory, { recursive: true, force: true })
    await rollbackTestTransaction()
  })

  async function npmMcp(environment: Array<{ name: string; value: string }> = []) {
    const admin = await createAdmin()
    const mcp = await createMcp(admin.id, { transport: 'npm', npmPackage: '@example/fake-mcp' })
    mcp.npmEnv = McpEnvironmentStore.merge(null, environment)
    await mcp.save()
    sandboxes.push(mcp.id)
    return mcp
  }

  test('drops reserved names saved before they were refused and sets its own last', async ({
    assert,
  }) => {
    // The validator refuses these now; rows saved earlier can still hold them.
    const mcp = await npmMcp([
      { name: 'PATH', value: '/member/bin' },
      { name: 'path', value: '/member/bin' },
      { name: 'HOME', value: '/member/home' },
      { name: 'TMPDIR', value: '/member/tmp' },
      { name: 'NO_COLOR', value: '0' },
      { name: 'DENO_DIR', value: '/member/cache' },
      { name: 'DENO_V8_FLAGS', value: '--allow-natives-syntax' },
      { name: 'LD_PRELOAD', value: '/member/evil.so' },
      { name: 'DYLD_INSERT_LIBRARIES', value: '/member/evil.dylib' },
      { name: 'NODE_OPTIONS', value: '--require=/member/evil.js' },
      { name: 'NODE_PATH', value: '/member/modules' },
      { name: 'GLIBC_TUNABLES', value: 'glibc.malloc.check=3' },
      { name: 'API_KEY', value: 'kept-secret' },
      { name: 'NPM_CONFIG_REGISTRY', value: 'https://registry.example/' },
    ])

    assert.deepEqual(buildDenoEnvironment(mcp, '/safe/sandbox'), {
      API_KEY: 'kept-secret',
      NPM_CONFIG_REGISTRY: 'https://registry.example/',
      PATH: process.env.PATH ?? '',
      HOME: '/safe/sandbox',
      TMPDIR: '/safe/sandbox',
      DENO_DIR: join(directory, 'deno-dir'),
      NO_COLOR: '1',
    })
  })

  test('starts the resolved binary with the gateway environment and one explicit cache', async ({
    assert,
  }) => {
    const mcp = await npmMcp([
      { name: 'PATH', value: join(directory, 'member-bin') },
      { name: 'LD_PRELOAD', value: '/member/evil.so' },
      { name: 'DYLD_INSERT_LIBRARIES', value: '/member/evil.dylib' },
      { name: 'DENO_DIR', value: join(directory, 'member-cache') },
      { name: 'DENO_V8_FLAGS', value: '--allow-natives-syntax' },
      { name: 'NODE_PATH', value: '/member/modules' },
      { name: 'API_KEY', value: 'kept-secret' },
    ])
    const sandbox = sandboxRootFor(mcp.id)

    const snapshot = await snapshotOf(mcp)

    assert.deepEqual(snapshot.env, {
      PATH: process.env.PATH ?? '',
      HOME: sandbox,
      TMPDIR: sandbox,
      DENO_DIR: join(directory, 'deno-dir'),
      NO_COLOR: '1',
      API_KEY: 'kept-secret',
      NPM_CONFIG_REGISTRY: null,
      LD_PRELOAD: null,
      DYLD_INSERT_LIBRARIES: null,
      DENO_V8_FLAGS: null,
      NODE_PATH: null,
    })
    // The package may read the cache it runs from, and write only its sandbox.
    assert.include(snapshot.argv, `--allow-read=${sandbox},${join(directory, 'deno-dir')}`)
    assert.include(snapshot.argv, `--allow-write=${sandbox}`)
    assert.isFalse(isWithinDirectory(join(directory, 'deno-dir'), sandbox))
  }).timeout(10_000)

  test('reloads the cache the MCP process reads, with the same environment', async ({ assert }) => {
    const mcp = await npmMcp([
      { name: 'NPM_CONFIG_REGISTRY', value: 'https://registry.example/' },
      { name: 'DENO_DIR', value: join(directory, 'member-cache') },
    ])
    const sandbox = sandboxRootFor(mcp.id)

    await reloadDenoNpmPackageCache(mcp)
    const reload: Snapshot = JSON.parse(await readFile(join(sandbox, 'reload.json'), 'utf8'))
    const started = await snapshotOf(mcp)

    assert.deepEqual(reload.argv, [
      'cache',
      '--reload',
      '--quiet',
      '--node-modules-dir=none',
      '--no-lock',
      'npm:@example/fake-mcp@latest',
    ])
    assert.equal(reload.env.DENO_DIR, join(directory, 'deno-dir'))
    assert.equal(reload.env.DENO_DIR, resolveDenoDir())
    assert.deepEqual(reload.env, started.env)
    assert.equal(reload.cwd, started.cwd)
  }).timeout(10_000)

  test('refuses a cache directory that exposes the app or that a package could write', async ({
    assert,
  }) => {
    const mcp = await npmMcp()

    process.env.DENO_DIR = app.makePath()
    assert.throws(() => buildDenoArgs(mcp, sandboxRootFor(mcp.id)), /must not contain/)
    assert.throws(() => buildDenoEnvironment(mcp, sandboxRootFor(mcp.id)), /must not contain/)

    process.env.DENO_DIR = join(sandboxRootFor(mcp.id), 'cache')
    assert.throws(() => buildDenoArgs(mcp, sandboxRootFor(mcp.id)), /inside an MCP sandbox/)

    // The Docker layout: next to the sandboxes, inside the app's tmp directory.
    process.env.DENO_DIR = app.tmpPath('deno-cache')
    assert.doesNotThrow(() => buildDenoArgs(mcp, sandboxRootFor(mcp.id)))
  })

  test('removes the cache Deno used to keep inside the sandbox', async ({ assert }) => {
    const mcp = await npmMcp()
    const sandbox = sandboxRootFor(mcp.id)
    const legacy = [join(sandbox, '.cache', 'deno'), join(sandbox, 'Library', 'Caches', 'deno')]
    for (const cache of legacy) {
      await mkdir(join(cache, 'npm', 'registry.npmjs.org'), { recursive: true })
    }
    await writeFile(join(sandbox, 'state.json'), '{}')

    await listDenoTools(mcp)

    for (const cache of legacy) {
      await assert.rejects(() => stat(cache))
    }
    // Only the cache goes: what the package itself stored stays.
    assert.equal(await readFile(join(sandbox, 'state.json'), 'utf8'), '{}')
  }).timeout(10_000)

  test('keeps reading stderr so a chatty package cannot block itself', async ({ assert }) => {
    // 512 KiB before the first MCP message: more than a pipe and its stream buffer hold.
    const mcp = await npmMcp([{ name: 'FAKE_BEHAVIOUR', value: 'noisy' }])

    const tools = await listDenoTools(mcp)

    assert.deepEqual(
      tools.map((tool) => tool.name),
      ['snapshot']
    )
  }).timeout(10_000)

  test('reports what a failed start wrote to stderr, without its secrets', async ({ assert }) => {
    // A secret over two lines, like a PEM key, has to be redacted before lines are joined.
    const mcp = await npmMcp([
      { name: 'FAKE_BEHAVIOUR', value: 'refuse-key' },
      { name: 'API_KEY', value: 'super-secret-api-key\nsecond-line-of-key' },
    ])

    let failure: unknown
    try {
      await listDenoTools(mcp)
    } catch (error) {
      failure = error
    }

    assert.instanceOf(failure, Error)
    const message = (failure as Error).message
    assert.include(message, 'Failed to start Deno npm MCP "@example/fake-mcp"')
    assert.include(message, 'Output: error: cannot start, the key [REDACTED] was refused')
    assert.notInclude(message, 'super-secret-api-key')
    assert.notInclude(message, 'second-line-of-key')
    assert.notInclude(message, 'Is Deno installed?')
  }).timeout(10_000)

  test('does not quote stderr it could not keep whole', async ({ assert }) => {
    const mcp = await npmMcp([{ name: 'FAKE_BEHAVIOUR', value: 'flood-then-fail' }])

    let failure: unknown
    try {
      await listDenoTools(mcp)
    } catch (error) {
      failure = error
    }

    const message = (failure as Error).message
    assert.include(message, 'Output exceeded 32 KiB and is not shown.')
    assert.notInclude(message, 'xxxx')
    assert.isBelow(message.length, 400)
  }).timeout(10_000)

  test('deletes the sandbox of an MCP on request', async ({ assert }) => {
    const mcp = await npmMcp()
    const sandbox = sandboxRootFor(mcp.id)
    await mkdir(join(sandbox, 'nested'), { recursive: true })
    await writeFile(join(sandbox, 'nested', 'token.json'), '{}')

    await removeMcpSandbox(mcp.id)
    await assert.rejects(() => stat(sandbox))

    // Nothing to delete, and nothing outside the sandboxes for an id that is not one.
    await removeMcpSandbox(mcp.id)
    await removeMcpSandbox(Number.NaN)
    await removeMcpSandbox(-1)
    const root = await stat(app.tmpPath('mcp-sandboxes'))
    assert.isTrue(root.isDirectory())
  })
})

function isWithinDirectory(path: string, directory: string) {
  return path === directory || path.startsWith(`${directory}/`)
}

test.group('Deno cache directory', (group) => {
  const saved = { DENO_DIR: process.env.DENO_DIR, XDG_CACHE_HOME: process.env.XDG_CACHE_HOME }

  group.each.teardown(() => {
    for (const [name, value] of Object.entries(saved)) {
      if (value === undefined) {
        delete process.env[name]
      } else {
        process.env[name] = value
      }
    }
  })

  test('is DENO_DIR, else where Deno itself would cache for the server user', ({ assert }) => {
    process.env.DENO_DIR = 'relative/deno-cache'
    assert.equal(resolveDenoDir(), join(process.cwd(), 'relative', 'deno-cache'))

    delete process.env.DENO_DIR
    process.env.XDG_CACHE_HOME = '/var/cache/server'
    assert.equal(resolveDenoDir(), '/var/cache/server/deno')

    delete process.env.XDG_CACHE_HOME
    assert.isTrue(isAbsolute(resolveDenoDir()))
    assert.match(resolveDenoDir(), /deno$/)
  }).skip(process.platform === 'win32', 'XDG_CACHE_HOME is not used on Windows')
})
