// @ts-check

import assert from 'node:assert/strict'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { test } from 'node:test'
import { bumpVersion, nightlyVersion, prepareRelease, updateChangelog } from './prepare-release.mjs'

test('bumps stable semantic versions', () => {
  assert.equal(bumpVersion('1.2.3', 'patch'), '1.2.4')
  assert.equal(bumpVersion('1.2.3', 'minor'), '1.3.0')
  assert.equal(bumpVersion('1.2.3', 'major'), '2.0.0')
})

test('rejects invalid versions and bump types', () => {
  assert.throws(() => bumpVersion('1.2.3-beta.1', 'patch'), /stable semantic version/)
  assert.throws(() => bumpVersion('1.2.3', 'banana'), /major, minor, or patch/)
})

test('creates valid nightly versions even for numeric hashes with a leading zero', () => {
  assert.equal(nightlyVersion('1.2.3', '20260808', '0123456'), '1.2.4-nightly.20260808.g0123456')
  assert.throws(() => nightlyVersion('1.2.3', '2026-08-08', 'abcdef0'), /YYYYMMDD/)
  assert.throws(() => nightlyVersion('1.2.3', '20260808', 'ABCDEF0'), /lowercase Git SHA/)
})

test('adds a release to an existing Changed section', () => {
  const changelog = `# Changelog

Intro.

## 2026-08-08

### Changed

- Existing change.
`

  const updated = updateChangelog(changelog, {
    date: '2026-08-08',
    releaseUrl: 'https://github.com/acme/project/releases/tag/v1.2.4',
    version: '1.2.4',
  })

  assert.match(updated, /### Changed\n\n- Released version \[1\.2\.4\].*\n\n- Existing change\./)
})

test('adds a Changed section to an existing date', () => {
  const changelog = `# Changelog

## 2026-08-08

### Fixed

- Existing fix.
`

  const updated = updateChangelog(changelog, {
    date: '2026-08-08',
    releaseUrl: 'https://github.com/acme/project/releases/tag/v1.2.4',
    version: '1.2.4',
  })

  assert.match(updated, /## 2026-08-08\n\n### Changed\n\n- Released version/)
  assert.match(updated, /### Fixed\n\n- Existing fix\./)
})

test('creates a new newest date section and remains idempotent', () => {
  const changelog = `# Changelog

Intro.

## 2026-08-07

### Added

- Existing feature.
`
  const release = {
    date: '2026-08-08',
    releaseUrl: 'https://github.com/acme/project/releases/tag/v1.2.4',
    version: '1.2.4',
  }
  const updated = updateChangelog(changelog, release)

  assert.ok(updated.indexOf('## 2026-08-08') < updated.indexOf('## 2026-08-07'))
  assert.equal(updateChangelog(updated, release), updated)
})

test('updates the package and changelog together', async (context) => {
  const rootDirectory = await mkdtemp(path.join(tmpdir(), 'mymcps-release-'))
  context.after(() => rm(rootDirectory, { force: true, recursive: true }))

  await writeFile(
    path.join(rootDirectory, 'package.json'),
    `${JSON.stringify({ name: 'app', version: '1.2.3' }, null, 2)}\n`
  )
  await writeFile(path.join(rootDirectory, 'CHANGELOG.md'), '# Changelog\n')

  const result = await prepareRelease({
    bump: 'minor',
    date: '2026-08-08',
    repository: 'acme/project',
    rootDirectory,
  })

  const packageJson = JSON.parse(await readFile(path.join(rootDirectory, 'package.json'), 'utf8'))
  const changelog = await readFile(path.join(rootDirectory, 'CHANGELOG.md'), 'utf8')

  assert.deepEqual(result, {
    releaseUrl: 'https://github.com/acme/project/releases/tag/v1.3.0',
    tag: 'v1.3.0',
    version: '1.3.0',
  })
  assert.equal(packageJson.version, '1.3.0')
  assert.match(changelog, /## 2026-08-08\n\n### Changed\n\n- Released version/)
})
