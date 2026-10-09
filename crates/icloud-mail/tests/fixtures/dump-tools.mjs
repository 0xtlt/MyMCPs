// Writes what the Node app advertises for each tool of iCloud Mail, which
// `tests/vine_builtin_input_schemas.rs` holds the port to. From the root of
// the repository:
//
//   NODE_ENV=test node --import=./node_modules/@poppinss/ts-exec/build/index.js \
//     crates/icloud-mail/tests/fixtures/dump-tools.mjs > crates/icloud-mail/tests/fixtures/icloud_mail_tools.json
import { pathToFileURL } from 'node:url'

const root = pathToFileURL(`${process.cwd()}/`)
const load = (path) => import(new URL(path, root).href)

// The tools sign links with the application, so it has to be booted.
const { Ignitor } = await load('./node_modules/@adonisjs/core/build/index.js')
const importer = (path) => (path.startsWith('.') ? load(path) : import(path))
const app = new Ignitor(root, { importer }).createApp('console')
await app.init()
await app.boot()

const { icloudMailMcp } = await load('./app/services/builtin/icloud_mail/index.js')
const tools = icloudMailMcp.tools.map((tool) => ({
  name: tool.name,
  description: tool.description,
  inputSchema: tool.inputSchema,
  requiresAnyScope: tool.requiresAnyScope ?? [],
  write: Boolean(tool.write),
}))
const { usernamePattern, passwordPattern, ...password } = icloudMailMcp.password
const definition = {
  key: icloudMailMcp.key,
  name: icloudMailMcp.name,
  password: { usernamePattern: usernamePattern.source, passwordPattern: passwordPattern.source, ...password },
  tools,
}
process.stdout.write(`${JSON.stringify(definition, null, 2)}\n`)
process.exit(0)
