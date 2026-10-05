import { spawn } from 'node:child_process'
import type { HtmlToTextOptions } from 'html-to-text'

const OPTIONS: HtmlToTextOptions = {
  wordwrap: false,
  selectors: [
    { selector: 'a', options: { hideLinkHrefIfSameAsText: true } },
    { selector: 'img', format: 'skip' },
  ],
}

/** A newsletter converts in a few milliseconds. The rest is the time a process takes to start. */
const TIMEOUT_MS = 5000
const MAX_HEAP_MB = 128

/** Tests shorten the time a conversion may take. */
export const htmlConversion = { timeoutMs: TIMEOUT_MS }

/**
 * Reads the HTML on its standard input and writes the text to its standard
 * output, cut to the number of characters given as its second argument.
 */
const CONVERTER = `
import { text } from 'node:stream/consumers'
const [converter, maxChars] = process.argv.slice(1)
const { convert } = await import(converter)
process.stdout.write(convert(await text(process.stdin), ${JSON.stringify(OPTIONS)}).slice(0, Number(maxChars)))
`

/** Resolves with `null` when the process fails, takes too long, or writes more than it may. */
function convertInChild(html: string, maxChars: number) {
  return new Promise<string | null>((resolve) => {
    const child = spawn(
      process.execPath,
      [
        `--max-old-space-size=${MAX_HEAP_MB}`,
        '--input-type=module',
        '--eval',
        CONVERTER,
        import.meta.resolve('html-to-text'),
        String(maxChars),
      ],
      // The converter needs nothing from the server: no secrets, no loaders.
      { env: {}, stdio: ['pipe', 'pipe', 'ignore'] }
    )
    const chunks: Buffer[] = []
    let size = 0
    const stop = (text: string | null) => {
      clearTimeout(timer)
      child.kill('SIGKILL')
      resolve(text)
    }
    const timer = setTimeout(() => stop(null), htmlConversion.timeoutMs)

    child.on('error', () => stop(null))
    // The process may be gone before it has read everything.
    child.stdin.on('error', () => {})
    child.stdout.on('data', (chunk: Buffer) => {
      size += chunk.length
      // A UTF-16 code unit is at most three bytes of UTF-8.
      if (size > maxChars * 3) return stop(null)
      chunks.push(chunk)
    })
    child.on('close', (code) => stop(code === 0 ? Buffer.concat(chunks).toString('utf8') : null))
    child.stdin.end(html)
  })
}

/** One conversion at a time: each is a process of its own. */
let queue: Promise<unknown> = Promise.resolve()

/**
 * The text of an HTML message, or `null` when it could not be converted.
 *
 * html-to-text reads the whole document in one go, and markup written for the
 * purpose keeps it busy for many seconds or makes it allocate hundreds of
 * megabytes: thousands of unclosed tags, quotes nested around many lines,
 * links nested in links. No usable limit on the size of the input rules these
 * out, and a message is written by a stranger. It is therefore converted in a
 * process of its own, which is killed when it takes too long and can only run
 * out of its own memory, while this one keeps serving requests.
 *
 * The text is cut to `maxChars` characters, with `isTruncated` set when it
 * was longer.
 */
export function htmlToText(html: string, maxChars: number) {
  const converted = queue
    .then(() => convertInChild(html, maxChars + 1))
    .then((text) =>
      text === null ? null : { text: text.slice(0, maxChars), isTruncated: text.length > maxChars }
    )
    .catch(() => null)
  queue = converted
  return converted
}
