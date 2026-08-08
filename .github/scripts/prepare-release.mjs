import { readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const VERSION_PATTERN = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/
const RELEASE_BUMPS = new Set(['major', 'minor', 'patch'])

export function bumpVersion(currentVersion, bump) {
  const match = VERSION_PATTERN.exec(currentVersion)

  if (!match) {
    throw new Error(`Expected a stable semantic version, received "${currentVersion}"`)
  }

  if (!RELEASE_BUMPS.has(bump)) {
    throw new Error(`Expected bump to be major, minor, or patch, received "${bump}"`)
  }

  let major = Number(match[1])
  let minor = Number(match[2])
  let patch = Number(match[3])

  if (bump === 'major') {
    major += 1
    minor = 0
    patch = 0
  } else if (bump === 'minor') {
    minor += 1
    patch = 0
  } else {
    patch += 1
  }

  return `${major}.${minor}.${patch}`
}

export function updateChangelog(changelog, { date, releaseUrl, version }) {
  const versionEntry = `- Released version [${version}](${releaseUrl}).`

  if (changelog.includes(versionEntry)) {
    return changelog
  }

  const dateHeading = `## ${date}`
  const dateStart = changelog.indexOf(dateHeading)

  if (dateStart === -1) {
    const firstDatedSection = changelog.search(/^## \d{4}-\d{2}-\d{2}$/m)
    const insertionPoint = firstDatedSection === -1 ? changelog.length : firstDatedSection
    const prefix = changelog.slice(0, insertionPoint).trimEnd()
    const suffix = changelog.slice(insertionPoint).trimStart()
    const releaseSection = `${dateHeading}\n\n### Changed\n\n${versionEntry}`

    return `${prefix}\n\n${releaseSection}\n\n${suffix}\n`
  }

  const nextDateOffset = changelog
    .slice(dateStart + dateHeading.length)
    .search(/^## \d{4}-\d{2}-\d{2}$/m)
  const dateEnd =
    nextDateOffset === -1 ? changelog.length : dateStart + dateHeading.length + nextDateOffset
  const dateSection = changelog.slice(dateStart, dateEnd)
  const changedHeading = '### Changed'
  const changedStart = dateSection.indexOf(changedHeading)

  if (changedStart === -1) {
    const insertionPoint = dateStart + dateHeading.length
    return `${changelog.slice(0, insertionPoint)}\n\n${changedHeading}\n\n${versionEntry}${changelog.slice(insertionPoint)}`
  }

  const insertionPoint = dateStart + changedStart + changedHeading.length
  return `${changelog.slice(0, insertionPoint)}\n\n${versionEntry}${changelog.slice(insertionPoint)}`
}

export async function prepareRelease({ bump, date, repository, rootDirectory }) {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date)) {
    throw new Error(`Expected date in YYYY-MM-DD format, received "${date}"`)
  }

  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) {
    throw new Error(`Expected repository in owner/name format, received "${repository}"`)
  }

  const packagePath = path.join(rootDirectory, 'package.json')
  const changelogPath = path.join(rootDirectory, 'CHANGELOG.md')
  const packageJson = JSON.parse(await readFile(packagePath, 'utf8'))
  const changelog = await readFile(changelogPath, 'utf8')
  const version = bumpVersion(packageJson.version, bump)
  const tag = `v${version}`
  const releaseUrl = `https://github.com/${repository}/releases/tag/${tag}`
  const updatedChangelog = updateChangelog(changelog, { date, releaseUrl, version })

  packageJson.version = version

  await writeFile(packagePath, `${JSON.stringify(packageJson, null, 2)}\n`)
  await writeFile(changelogPath, updatedChangelog)

  return { releaseUrl, tag, version }
}

function parseArguments(arguments_) {
  const values = new Map()

  for (let index = 0; index < arguments_.length; index += 2) {
    const key = arguments_[index]
    const value = arguments_[index + 1]

    if (!key?.startsWith('--') || value === undefined) {
      throw new Error(
        'Usage: prepare-release.mjs --bump <type> --date <date> --repository <owner/name>'
      )
    }

    values.set(key.slice(2), value)
  }

  return values
}

async function main() {
  const arguments_ = parseArguments(process.argv.slice(2))
  const bump = arguments_.get('bump')
  const date = arguments_.get('date')
  const repository = arguments_.get('repository')

  if (!bump || !date || !repository) {
    throw new Error(
      'Usage: prepare-release.mjs --bump <type> --date <date> --repository <owner/name>'
    )
  }

  const result = await prepareRelease({
    bump,
    date,
    repository,
    rootDirectory: process.cwd(),
  })

  console.log(JSON.stringify(result))
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  await main()
}
