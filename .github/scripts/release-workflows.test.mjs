import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { test } from 'node:test'

const workflowsDirectory = new URL('../workflows/', import.meta.url)

/** @param {string} name */
async function readWorkflow(name) {
  return readFile(new URL(name, workflowsDirectory), 'utf8')
}

test('release workflows publish their exact successful release through the reusable workflow', async () => {
  const stable = await readWorkflow('stable-release.yml')
  const nightly = await readWorkflow('nightly-release.yml')

  for (const [workflow, channel] of [
    [stable, 'stable'],
    [nightly, 'nightly'],
  ]) {
    assert.match(workflow, /published: \$\{\{ steps\.publish\.outputs\.published \}\}/)
    assert.match(workflow, /release_tag: \$\{\{ steps\.(?:release|plan)\.outputs\.tag \}\}/)
    assert.match(workflow, /if: needs\.release\.outputs\.published == 'true'/)
    assert.match(workflow, /uses: \.\/\.github\/workflows\/publish-container\.yml/)
    assert.match(workflow, new RegExp(`release_channel: ${channel}`))
    assert.match(workflow, /release_tag: \$\{\{ needs\.release\.outputs\.release_tag \}\}/)
  }
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

test('every external action in release workflows is pinned to a full commit SHA', async () => {
  const workflows = await Promise.all(
    ['stable-release.yml', 'nightly-release.yml', 'publish-container.yml'].map(readWorkflow)
  )

  for (const workflow of workflows) {
    for (const match of workflow.matchAll(/^\s*uses:\s*([^\s]+).*$/gm)) {
      const action = match[1]
      if (action.startsWith('./')) continue
      assert.match(action, /^[^@]+@[0-9a-f]{40}$/)
    }
  }
})
