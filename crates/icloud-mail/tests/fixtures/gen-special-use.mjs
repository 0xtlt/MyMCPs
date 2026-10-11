// Writes `src/imap/special_use_names.rs`: the names imapflow 2.2.4 knows
// special mailboxes by. From the root of the repository:
//
//   node crates/icloud-mail/tests/fixtures/gen-special-use.mjs > crates/icloud-mail/src/imap/special_use_names.rs
import { mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'

// The two tables are private to the module: a copy of it exports them.
const imapflow = `${process.cwd()}/node_modules/.pnpm/imapflow@2.2.4/node_modules/imapflow/dist/esm`
const source = await readFile(`${imapflow}/special-use.js`, 'utf8')
const copy = join(await mkdtemp(join(tmpdir(), 'special-use-')), 'special-use.mjs')
await writeFile(copy, source.replace(/^const (GENERIC_TOKENS|NAME_INDEX)/gm, 'export const $1'))
const { GENERIC_TOKENS, NAME_INDEX } = await import(pathToFileURL(copy).href)

const literal = (text) => JSON.stringify(text).replace(/[\u007f-￿]/g, (c) => `\\u{${c.charCodeAt(0).toString(16)}}`)
// Rust compares strings by their UTF-8 bytes, which orders them by code point.
const byCodePoints = (a, b) => {
  const left = [...a].map((c) => c.codePointAt(0))
  const right = [...b].map((c) => c.codePointAt(0))
  for (let i = 0; i < Math.min(left.length, right.length); i++) {
    if (left[i] !== right[i]) return left[i] - right[i]
  }
  return left.length - right.length
}

const names = [...NAME_INDEX.entries()].sort(([a], [b]) => byCodePoints(a, b))
const tokens = [...GENERIC_TOKENS].sort(byCodePoints)

console.log('//! Generated from `special-use.js` of imapflow 2.2.4 by `tests/fixtures/gen-special-use.mjs`. Do not edit.')
console.log('')
console.log('/// A mailbox name in lower case and NFKC, and the special use it stands for. Sorted by name.')
console.log('pub(super) static NAMES: &[(&str, &str)] = &[')
for (const [name, flag] of names) console.log(`    (${literal(name)}, ${literal(flag)}),`)
console.log('];')
console.log('')
console.log('/// Words that say nothing about what a mailbox is for, such as "mail" or "items". Sorted.')
console.log('pub(super) static GENERIC_TOKENS: &[&str] = &[')
for (const token of tokens) console.log(`    ${literal(token)},`)
console.log('];')
