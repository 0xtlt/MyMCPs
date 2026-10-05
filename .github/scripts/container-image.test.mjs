// @ts-check

import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

const execFileAsync = promisify(execFile)
const repositoryRoot = new URL('../../', import.meta.url)

/** @param {string} name */
async function readRepositoryFile(name) {
  return readFile(new URL(name, repositoryRoot), 'utf8')
}

/**
 * Images pulled from a registry, in order. A FROM that continues an earlier
 * stage is not a base image.
 *
 * @param {string} dockerfile
 */
function baseImages(dockerfile) {
  /** @type {Set<string>} */
  const stages = new Set()
  /** @type {string[]} */
  const images = []

  for (const line of dockerfile.split('\n')) {
    const match = /^FROM\s+(\S+)(?:\s+AS\s+(\S+))?\s*$/i.exec(line)
    if (!match) continue
    if (!stages.has(match[1])) images.push(match[1])
    if (match[2]) stages.add(match[2])
  }

  return images
}

test('every base image is pinned by tag and digest in its FROM line', async () => {
  const images = baseImages(await readRepositoryFile('Dockerfile'))

  assert.ok(images.length > 0)
  for (const image of images) {
    // Dependabot can only update a tag and digest it reads in the FROM line
    // itself, so neither may come from an ARG.
    assert.match(image, /^[a-z0-9./-]+:[A-Za-z0-9._-]+@sha256:[0-9a-f]{64}$/)
  }
})

test('the build and runtime stages use the same Node image', async () => {
  const images = baseImages(await readRepositoryFile('Dockerfile')).filter((image) =>
    image.startsWith('node:')
  )

  // better-sqlite3 is compiled in the build stage and copied into the runtime stage.
  assert.equal(images.length, 2)
  assert.equal(images[0], images[1])
})

test('DENO_VERSION matches the pinned Deno image', async () => {
  const dockerfile = await readRepositoryFile('Dockerfile')
  const image = /^FROM denoland\/deno:bin-(\d+\.\d+\.\d+)@sha256:[0-9a-f]{64} AS deno$/m.exec(
    dockerfile
  )
  const variable = /^\s*DENO_VERSION=(\S+)/m.exec(dockerfile)

  assert.ok(image, 'expected a digest-pinned denoland/deno:bin-<version> stage')
  assert.ok(variable, 'expected the runtime stage to set DENO_VERSION')
  assert.equal(
    variable[1],
    image[1],
    'DENO_VERSION must be updated together with the denoland/deno image tag'
  )
})

test('Dependabot updates the Dockerfile base images', async () => {
  const dependabot = await readRepositoryFile('.github/dependabot.yml')

  assert.match(dependabot, /^ {2}- package-ecosystem: docker\n {4}directory: \/$/m)
})

test('the build context leaves out secrets, Git data, and coding-agent state', async () => {
  const ignored = (await readRepositoryFile('.dockerignore')).split('\n')

  for (const entry of ['.env', '.env.*', '.git', '.claude', '.codex', '.cursor']) {
    assert.ok(ignored.includes(entry), `expected .dockerignore to list ${entry}`)
  }
})

test(
  'the entrypoint keeps what it and the server create private to the app user',
  { skip: process.platform === 'win32' },
  async (t) => {
    const workDirectory = await mkdtemp(path.join(tmpdir(), 'mymcps-entrypoint-'))
    t.after(() => rm(workDirectory, { recursive: true, force: true }))

    // Stands in for `node ace migration:run`, which creates the SQLite database.
    const binDirectory = path.join(workDirectory, 'bin')
    await mkdir(binDirectory)
    await writeFile(path.join(binDirectory, 'node'), '#!/bin/sh\n: > tmp/db.sqlite3\n', {
      mode: 0o755,
    })

    const entrypoint = fileURLToPath(new URL('docker-entrypoint.sh', repositoryRoot))
    const { stdout } = await execFileAsync('sh', [entrypoint, 'sh', '-c', 'umask'], {
      cwd: workDirectory,
      // A supplied key is the path that used to keep the default umask.
      env: { PATH: `${binDirectory}${path.delimiter}${process.env.PATH}`, APP_KEY: 'supplied' },
    })

    /** @param {string} name */
    const mode = async (name) => (await stat(path.join(workDirectory, name))).mode & 0o777

    // The command the entrypoint hands over to (the server, then its Deno
    // children) inherits the umask.
    assert.match(stdout.trim(), /^0?077$/)
    assert.equal(await mode('tmp/db.sqlite3'), 0o600)
    assert.equal(await mode('tmp/mcp-sandboxes'), 0o700)
    assert.equal(await mode('tmp/deno-cache'), 0o700)
  }
)
