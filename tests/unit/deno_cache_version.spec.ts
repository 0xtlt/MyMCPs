import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from '@japa/runner'
import {
  buildDenoCacheReloadArgs,
  readCachedNpmPackageVersion,
} from '#services/upstream/deno_runner'

async function fakeNpmCache(options: { npmPackage: string; versions: string[]; latest?: string }) {
  const denoDir = await mkdtemp(join(tmpdir(), 'mymcps-deno-cache-'))
  const packageDir = join(denoDir, 'npm', 'registry.npmjs.org', ...options.npmPackage.split('/'))
  await mkdir(packageDir, { recursive: true })
  for (const version of options.versions) {
    await mkdir(join(packageDir, version), { recursive: true })
  }
  if (options.latest) {
    await writeFile(
      join(packageDir, 'registry.json'),
      JSON.stringify({ 'dist-tags': { latest: options.latest } })
    )
  }
  return denoDir
}

test.group('Deno npm cache version', (group) => {
  let previousDenoDir: string | undefined

  group.each.setup(() => {
    previousDenoDir = process.env.DENO_DIR
  })

  group.each.teardown(async () => {
    if (previousDenoDir === undefined) {
      delete process.env.DENO_DIR
    } else {
      process.env.DENO_DIR = previousDenoDir
    }
  })

  test('reads dist-tags.latest when the MCP tracks latest', async ({ assert }) => {
    const denoDir = await fakeNpmCache({
      npmPackage: '@shopify/dev-mcp',
      versions: ['1.14.4', '1.13.0'],
      latest: '1.14.4',
    })
    process.env.DENO_DIR = denoDir

    try {
      assert.equal(readCachedNpmPackageVersion('@shopify/dev-mcp', 'latest'), '1.14.4')
      assert.equal(readCachedNpmPackageVersion('@shopify/dev-mcp', null), '1.14.4')
      assert.equal(readCachedNpmPackageVersion('@shopify/dev-mcp', ''), '1.14.4')
    } finally {
      await rm(denoDir, { recursive: true, force: true })
    }
  })

  test('returns a pinned version only when that folder is cached', async ({ assert }) => {
    const denoDir = await fakeNpmCache({
      npmPackage: 'mongodb-mcp-server',
      versions: ['2.0.0'],
      latest: '2.1.0',
    })
    process.env.DENO_DIR = denoDir

    try {
      assert.equal(readCachedNpmPackageVersion('mongodb-mcp-server', '2.0.0'), '2.0.0')
      assert.isNull(readCachedNpmPackageVersion('mongodb-mcp-server', '9.9.9'))
    } finally {
      await rm(denoDir, { recursive: true, force: true })
    }
  })

  test('returns null when the package is not in the Deno cache', ({ assert }) => {
    process.env.DENO_DIR = join(tmpdir(), 'mymcps-missing-deno-cache')
    assert.isNull(readCachedNpmPackageVersion('@example/missing-mcp', 'latest'))
  })
})

test.group('Deno npm cache reload args', () => {
  test('ignores the host package.json node_modules mode', ({ assert }) => {
    assert.deepEqual(buildDenoCacheReloadArgs('@shopify/dev-mcp'), [
      'cache',
      '--reload',
      '--quiet',
      '--node-modules-dir=none',
      'npm:@shopify/dev-mcp@latest',
    ])
  })

  test('rejects an empty package name', ({ assert }) => {
    assert.throws(() => buildDenoCacheReloadArgs('  '), 'npm MCP is missing a package name')
  })
})
