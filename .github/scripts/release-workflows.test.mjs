// @ts-check

import assert from 'node:assert/strict'
import { readdir, readFile } from 'node:fs/promises'
import { test } from 'node:test'

const workflowsDirectory = new URL('../workflows/', import.meta.url)

/** @param {string} name */
async function readWorkflow(name) {
  return readFile(new URL(name, workflowsDirectory), 'utf8')
}

/**
 * Splits a workflow into the text of each job, keyed by job id. Comment lines
 * are dropped so assertions only see what the workflow does.
 *
 * @param {string} workflow
 */
function jobsOf(workflow) {
  const jobsStart = workflow.indexOf('\njobs:\n')
  assert.notEqual(jobsStart, -1)

  /** @type {Map<string, string>} */
  const jobs = new Map()
  let current = ''
  for (const line of workflow.slice(jobsStart + '\njobs:\n'.length).split('\n')) {
    const header = /^ {2}([A-Za-z][\w-]*):$/.exec(line)
    if (header) {
      current = header[1]
      jobs.set(current, '')
    } else if (current && !/^\s*#/.test(line)) {
      jobs.set(current, `${jobs.get(current)}${line}\n`)
    }
  }

  return jobs
}

/**
 * @param {Map<string, string>} jobs
 * @param {string} id
 */
function job(jobs, id) {
  const text = jobs.get(id)
  assert.ok(text, `expected a "${id}" job`)
  return text
}

const releaseWorkflows = /** @type {const} */ ([
  ['stable-release.yml', 'stable'],
  ['nightly-release.yml', 'nightly'],
])

test('release workflows publish their exact successful release through the reusable workflow', async () => {
  for (const [name, channel] of releaseWorkflows) {
    const workflow = await readWorkflow(name)

    assert.match(workflow, /published: \$\{\{ steps\.publish\.outputs\.published \}\}/)
    assert.match(workflow, /release_tag: \$\{\{ steps\.(?:release|publish)\.outputs\.tag \}\}/)
    assert.match(workflow, /if: needs\.release\.outputs\.published == 'true'/)
    assert.match(workflow, /uses: \.\/\.github\/workflows\/publish-container\.yml/)
    assert.match(workflow, new RegExp(`release_channel: ${channel}`))
    assert.match(workflow, /release_tag: \$\{\{ needs\.release\.outputs\.release_tag \}\}/)
  }
})

test('release workflows keep dependency code away from the write token', async () => {
  for (const [name] of releaseWorkflows) {
    const workflow = await readWorkflow(name)
    const jobs = jobsOf(workflow)

    assert.deepEqual([...jobs.keys()], ['validate', 'release', 'publish-container'])
    assert.match(workflow, /^permissions:\n {2}contents: read\n\n/m)
    assert.equal(workflow.match(/contents: write/g)?.length, 1)

    // Everything that installs or runs dependencies happens with a read-only
    // token and without credentials left in the checkout.
    const validate = job(jobs, 'validate')
    assert.match(validate, /^ {4}permissions:\n {6}contents: read\n {4}outputs:/m)
    assert.match(validate, /ref: main\n/)
    assert.equal(validate.match(/uses: actions\/checkout@/g)?.length, 1)
    assert.match(validate, /persist-credentials: false/)
    assert.match(validate, /sha: \$\{\{ steps\.(?:commit|plan)\.outputs\.sha \}\}/)
    assert.match(validate, /echo "sha=\$\(git rev-parse HEAD\)"/)
    for (const command of [
      'pnpm install --frozen-lockfile --config.minimumReleaseAge=360',
      'pnpm exec playwright install --with-deps chromium',
      'pnpm run test:release',
      'pnpm run lint',
      'pnpm run typecheck',
      'pnpm test',
      'pnpm run build',
      'pnpm audit --audit-level=high',
    ]) {
      assert.ok(validate.includes(command), `expected validate to run "${command}"`)
    }
    assert.doesNotMatch(validate, /git push|git tag --annotate|git commit|gh release create|gh api/)

    // The job holding the write token only tags and publishes the commit that
    // was validated.
    const release = job(jobs, 'release')
    assert.match(release, /^ {4}needs: validate\n/m)
    assert.match(release, /^ {4}permissions:\n {6}contents: write\n {4}outputs:/m)
    assert.equal(release.match(/uses: actions\/checkout@/g)?.length, 1)
    assert.match(
      release,
      /fetch-depth: 0\n {10}persist-credentials: true\n {10}ref: \$\{\{ needs\.validate\.outputs\.sha \}\}\n/
    )
    assert.match(release, /"\$\(git rev-parse HEAD\)" != "\$VALIDATED_SHA"/)
    assert.match(
      release,
      /git merge-base --is-ancestor "\$VALIDATED_SHA" refs\/remotes\/origin\/main/
    )
    assert.doesNotMatch(release, /\b(?:pnpm|npm|npx|yarn|corepack|playwright)\b/)
    assert.doesNotMatch(release, /^\s+cache: /m)
    assert.match(release, /gh release create "\$TAG"/)
    assert.match(release, /--verify-tag/)

    const container = job(jobs, 'publish-container')
    assert.match(container, /^ {4}needs: release\n/m)
    assert.match(container, /^ {4}permissions:\n {6}contents: read\n {6}packages: write\n/m)
  }
})

test('the nightly workflow only releases when the validation job planned one', async () => {
  const jobs = jobsOf(await readWorkflow('nightly-release.yml'))
  const validate = job(jobs, 'validate')
  const release = job(jobs, 'release')

  assert.match(validate, /publish: \$\{\{ steps\.plan\.outputs\.publish \}\}/)
  assert.equal(validate.match(/echo "publish=false" >> "\$GITHUB_OUTPUT"/g)?.length, 2)
  // Every step after the plan is skipped when there is nothing to release.
  const afterPlan = validate.slice(validate.indexOf('      - name: Set up pnpm\n'))
  assert.equal(afterPlan.match(/^ {6}- name: /gm)?.length, 6)
  assert.equal(afterPlan.match(/^ {8}if: steps\.plan\.outputs\.publish == 'true'$/gm)?.length, 6)

  assert.match(release, /^ {4}if: needs\.validate\.outputs\.publish == 'true'\n/m)
  assert.match(release, /TAG: \$\{\{ needs\.validate\.outputs\.tag \}\}/)
  assert.match(release, /PREVIOUS_TAG: \$\{\{ needs\.validate\.outputs\.previous_tag \}\}/)
  assert.match(release, /git push origin "refs\/tags\/\$TAG"/)
  assert.match(release, /--prerelease/)
})

test('the stable workflow prepares, pushes, and publishes in the release job', async () => {
  const jobs = jobsOf(await readWorkflow('stable-release.yml'))
  const release = job(jobs, 'release')

  assert.match(release, /BUMP: \$\{\{ inputs\.bump \}\}/)
  assert.match(release, /node \.github\/scripts\/prepare-release\.mjs/)
  assert.match(release, /git diff --check\n {10}node --test \.github\/scripts\/\*\.test\.mjs/)
  assert.match(release, /git push --atomic origin HEAD:refs\/heads\/main "refs\/tags\/\$TAG"/)
  assert.doesNotMatch(release, /--prerelease/)
})

test('the release preparation script has no dependencies to install', async () => {
  const script = await readFile(new URL('prepare-release.mjs', import.meta.url), 'utf8')
  const imported = [...script.matchAll(/^import .* from '([^']+)'$/gm)].map((match) => match[1])

  assert.ok(imported.length > 0)
  for (const specifier of imported) {
    assert.match(specifier, /^node:/)
  }
  assert.doesNotMatch(script, /\bimport\(|\brequire\(/)
})

test('pull requests and main run the test suites with a read-only token', async () => {
  const workflow = await readWorkflow('quality.yml')
  const jobs = jobsOf(workflow)
  const tests = job(jobs, 'tests')

  assert.match(
    workflow,
    /^on:\n {2}pull_request:\n[\s\S]*?^ {2}push:\n {4}branches:\n {6}- main\n/m
  )
  assert.match(workflow, /^permissions:\n {2}contents: read\n\n/m)
  assert.doesNotMatch(workflow, /: write/)
  assert.equal(
    workflow.match(/uses: actions\/checkout@/g)?.length,
    workflow.match(/persist-credentials: false/g)?.length
  )

  assert.match(tests, /pnpm install --frozen-lockfile --config\.minimumReleaseAge=360\n/)
  assert.match(tests, /pnpm exec playwright install --with-deps chromium\n/)
  assert.match(tests, /pnpm run test:release\n/)
  // `pnpm test` runs every suite in adonisrc.ts: unit, functional, and browser.
  assert.match(tests, /run: pnpm test\n/)
})

test('container workflow validates before publishing exact multi-platform channel tags', async () => {
  const workflow = await readWorkflow('publish-container.yml')

  assert.match(workflow, /workflow_call:/)
  assert.match(workflow, /ref: refs\/tags\/\$\{\{ inputs\.release_tag \}\}/)
  assert.match(workflow, /platforms: linux\/amd64,linux\/arm64/)
  assert.match(workflow, /push: false[\s\S]*Smoke-test container health[\s\S]*push: true/)
  assert.match(workflow, /--env LOG_LEVEL=info/)
  assert.match(workflow, /--env SESSION_DRIVER=cookie/)
  assert.match(workflow, /\$\{\{ env\.IMAGE_NAME \}\}:\$\{\{ inputs\.release_channel \}\}/)
  assert.match(workflow, /\$\{\{ env\.IMAGE_NAME \}\}:\$\{\{ inputs\.release_tag \}\}/)
  assert.match(
    workflow,
    /org\.opencontainers\.image\.revision=\$\{\{ steps\.revision\.outputs\.sha \}\}/
  )
  assert.match(workflow, /cache-from: type=gha/)
  assert.match(workflow, /cache-to: type=gha/)
  assert.equal(workflow.match(/provenance: false/g)?.length, 2)
  assert.equal(workflow.match(/sbom: false/g)?.length, 2)
})

test('every external action in every workflow is pinned to a full commit SHA', async () => {
  const names = (await readdir(workflowsDirectory)).filter((name) => name.endsWith('.yml'))
  assert.ok(names.length >= 5)

  for (const workflow of await Promise.all(names.map(readWorkflow))) {
    for (const match of workflow.matchAll(/^\s*(?:- )?uses:\s*([^\s]+).*$/gm)) {
      const action = match[1]
      if (action.startsWith('./')) continue
      assert.match(action, /^[^@]+@[0-9a-f]{40}$/)
    }
  }
})
