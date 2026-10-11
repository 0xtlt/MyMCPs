// Writes the fixtures of the differential tests: what html-to-text 10.0.1 gives, with the
// options of the iCloud Mail MCP, for each HTML input.
//
//   node crates/icloud-mail/tests/fixtures/html/generate.mjs
//       cases.json  the corpus written by hand below
//       soup.json   seeded random tag soup, half of it loose tags and half of it trees
//       large.json  inputs too large to store, as recipes, with a hash of the text; some of
//                   them nest deeper than the stack of Node allows, and take it a minute
//
// More than is worth storing, for `HTML_PORT_EXTRA_FIXTURE=/tmp/more.json cargo test extra`:
//
//   node crates/icloud-mail/tests/fixtures/html/generate.mjs --soup 200000 --seed 7 --out /tmp/more.json
//       more soup
//   node crates/icloud-mail/tests/fixtures/html/generate.mjs --stray /tmp/more.json
//       every way the entity decoder leaves its trie (see `strayPaths`), of which
//       cases.json has a sample
//
// REPOSITORY is the checkout whose node_modules holds html-to-text 10.0.1: the
// directory the script is run from, unless the variable says otherwise.
import { writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'
import { Worker, isMainThread, parentPort, workerData } from 'node:worker_threads'

const REPOSITORY = process.env.REPOSITORY ?? process.cwd()
const { convert } = await import(
  `${REPOSITORY}/node_modules/.pnpm/html-to-text@10.0.1/node_modules/html-to-text/lib/html-to-text.mjs`
)

// The library says so when it cuts an input that is too long, once for each.
console.warn = () => {}

/** `OPTIONS` of app/services/builtin/icloud_mail/html.ts. */
const OPTIONS = {
  wordwrap: false,
  selectors: [
    { selector: 'a', options: { hideLinkHrefIfSameAsText: true } },
    { selector: 'img', format: 'skip' },
  ],
}

/**
 * The text as the server reads it from the converting process: written as UTF-8, where
 * half a surrogate pair becomes U+FFFD. `{ throws: true }` when the library throws,
 * which the server reports as a message it could not convert.
 */
function expected(html) {
  try {
    return { text: Buffer.from(convert(html, OPTIONS), 'utf8').toString('utf8') }
  } catch (error) {
    // Running out of stack is a limit of Node, not what the library says of the document.
    if (error instanceof RangeError) throw new Error(`Node cannot convert ${JSON.stringify(html.slice(0, 80))}`, { cause: error })
    return { throws: true }
  }
}

// html-to-text calls itself for every element inside another, and Node gives up some
// thousands deep: at 976 for `<ul><li>`, at 3028 for `<div>`. A worker of this script has
// the stack, and the heap, to say what the library would answer.
if (!isMainThread) {
  parentPort.postMessage(expected(workerData))
  process.exit(0)
}

function expectedWithStack(html) {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL(import.meta.url), {
      workerData: html,
      resourceLimits: { stackSizeMb: 1500, maxOldGenerationSizeMb: 12000 },
    })
    worker.on('message', resolve)
    worker.on('error', reject)
  })
}

function write(name, value) {
  const path = fileURLToPath(new URL(name, import.meta.url))
  // One input to a line.
  writeFileSync(path, `[\n${value.map((item) => JSON.stringify(item)).join(',\n')}\n]\n`)
  console.log(`${name}: ${value.length} inputs`)
}

// ---------------------------------------------------------------------------------------
// Random tag soup
// ---------------------------------------------------------------------------------------

/** mulberry32: small, seeded, and the same on every machine. */
function random(seed) {
  let state = seed >>> 0
  return () => {
    state = (state + 0x6d2b79f5) >>> 0
    let t = state
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

const SOUP_TAGS = [
  'a', 'a', 'b', 'i', 'span', 'p', 'p', 'div', 'div', 'ul', 'ol', 'li', 'li', 'blockquote',
  'pre', 'h1', 'h5', 'br', 'hr', 'table', 'tbody', 'thead', 'tr', 'td', 'th', 'body', 'html',
  'head', 'title', 'script', 'style', 'textarea', 'xmp', 'svg', 'math', 'img', 'wbr', 'form',
  'select', 'option', 'optgroup', 'button', 'input', 'dd', 'dt', 'dl', 'section', 'article',
  'center', 'font', 'x-y', 'mi', 'desc', 'link', 'meta', 'rt', 'rp', 'tfoot', 'tmp',
]
const SOUP_ATTRIBUTES = [
  '', '', '', '', ' href="https://e.example/"', ' href=x', " href='a b'", ' href="#top"',
  ' href="mailto:a@b.example"', ' href', ' href=""', ' HREF=y href=z', ' start=3', ' start="-2"',
  ' type=a', ' type="I"', ' type=i start=3998', ' start=x', ' class="c"', ' a=b c d="e"',
  ' href="a&amp;b"', ' href=a&ampb', ' href="&#x41;&lt"', ' /', '/', ' x=">"', " y='<'",
  ' href="X"', ' href="A B"', ' href=" a "',
]
const SOUP_TEXT = [
  'a', 'b', 'c d', ' ', ' ', '  ', '\n', '\n\n', '\t', '\r\n', '\f', '​', ' ', 'x', 'X',
  'A B', 'https://e.example/', '&amp;', '&nbsp;', '&zwnj;', '&lt', '&lt;', '&#65;', '&#x1F600;',
  '&#0;', '&#xD800;', '&notit;', '&not', '&', '&;', '&#', '&#x', '&unknown;', '&Afr;', '&nGt;',
  'é', 'ß', 'ǆ', '😀', '<', '>', '<<', '< ', '"', "'", '=', '/', '-', '--', '!', '?', ']', ']]>',
  'word', 'two words', ' lead', 'trail ', ' ', '.', ',',
]
const SOUP_MARKUP = [
  '<!--', '-->', '<!-- c -->', '<!>', '<!x>', '<!doctype html>', '<![CDATA[', ']]>',
  '<![CDATA[ d ]]>', '<?', '<?x ?>', '</', '</>', '</ >', '</3>', '<', '/>', '>', '</p>',
  '</br>', '</br >', '</hr>', '</svg>', '</title>', '</script>', '</style', '</textarea>',
  '</xmp>', '</body>', '</html>', '<a', '<a href="', "<a href='", '<p ', '<s', '<t', '<scr',
]

function soup(next) {
  const pick = (list) => list[Math.floor(next() * list.length)]
  const count = 1 + Math.floor(next() * (next() < 0.2 ? 60 : 14))
  let html = ''
  for (let index = 0; index < count; index++) {
    const kind = next()
    if (kind < 0.34) {
      const tag = pick(SOUP_TAGS)
      html += `<${next() < 0.08 ? tag.toUpperCase() : tag}${pick(SOUP_ATTRIBUTES)}>`
    } else if (kind < 0.56) {
      html += `</${pick(SOUP_TAGS)}>`
    } else if (kind < 0.92) {
      html += pick(SOUP_TEXT)
    } else {
      html += pick(SOUP_MARKUP)
    }
  }
  return html
}

/**
 * Markup that is closed as it was opened, most of the time: where the blocks, the lists,
 * the quotes and the whitespace between them meet, which loose tags seldom reach.
 */
const TREE_ELEMENTS = [
  'p', 'div', 'div', 'blockquote', 'pre', 'h2', 'h6', 'ul', 'ol', 'li', 'li', 'span', 'b', 'a',
  'table', 'tr', 'td', 'article', 'section', 'center',
]
const TREE_LEAVES = [
  'a', 'b c', ' ', ' ', '\n', '\n  ', ' x ', 'x', '<br>', '<br>', '<hr>', '<img src="i">', '&nbsp;',
  '\u200b', '<!-- c -->', '', '', 'https://e.example/', 'word', 'A', '\t', '<wbr>', '<p>',
  '</p>', '</div>', '<li>', '</blockquote>',
]

function tree(next, depth) {
  const pick = (list) => list[Math.floor(next() * list.length)]
  const count = Math.floor(next() * (depth === 0 ? 6 : 4))
  let html = ''
  for (let index = 0; index < count; index++) {
    if (depth >= 5 || next() < 0.45) {
      html += pick(TREE_LEAVES)
      continue
    }
    const tag = pick(TREE_ELEMENTS)
    const attributes =
      tag === 'a' ? pick(SOUP_ATTRIBUTES) : tag === 'ol' && next() < 0.5 ? pick(SOUP_ATTRIBUTES) : ''
    html += `<${tag}${attributes}>${tree(next, depth + 1)}`
    // Now and then the end tag is missing.
    if (next() < 0.93) html += `</${tag}>`
  }
  return html
}

function soups(count, seed) {
  const next = random(seed)
  return Array.from({ length: count }, (_, index) => {
    const html = index % 2 === 0 ? soup(next) : tree(next, 0)
    return { html, ...expected(html) }
  })
}

const argument = (name) => {
  const at = process.argv.indexOf(name)
  return at < 0 ? undefined : process.argv[at + 1]
}
if (argument('--out')) {
  const fixtures = soups(Number(argument('--soup') ?? 10000), Number(argument('--seed') ?? 1))
  writeFileSync(argument('--out'), JSON.stringify(fixtures))
  console.log(`${argument('--out')}: ${fixtures.length} inputs`)
  process.exit(0)
}

// ---------------------------------------------------------------------------------------
// The corpus
// ---------------------------------------------------------------------------------------

const cases = []
const add = (...htmls) => cases.push(...htmls)

// The tests of the Node server.
add(
  '<html><head><style>p { color: red }</style></head><body><p>Hello‌ ‌ ‌ ‌ </p><img src="https://shop.example/pixel.gif" alt="tracking"><p><a href="https://shop.example/deals">See the deals</a></p><p><a href="https://shop.example">https://shop.example</a></p><p>This paragraph is long enough that a wrapping converter would break it across several lines of output.</p></body></html>',
  '<p>Still <b>here</b></p>',
  '<pre>a\n  b</pre>',
  '',
  '<p>Message 1</p>'
)

// Nothing, text alone, and every kind of whitespace.
add(
  ' ', '\n', '\t', '\r\n', '\f', '​', ' ', 'a', ' a', 'a ', ' a ', 'a b', 'a  b',
  'a\nb', 'a\r\nb', 'a\tb', 'a\fb', 'a​b', 'a ​ b', 'a b', 'a   b',
  ' a ', '&nbsp;', ' &nbsp; ', 'a&nbsp;&nbsp;b', 'a b', 'a b', 'a﻿b',
  'a\u000bb', '\n\n a \n\n b \n\n', 'a‌‍b', '&zwnj;&nbsp;&zwnj;&nbsp;&zwnj;&nbsp;',
  '<div>&zwnj;&nbsp;&zwnj;&nbsp;</div><div>Text</div>', '​​', ' ​ a​',
  'a\0b', '\0', 'a\u001cb', 'a\u0085b'
)

// Whitespace around inline elements, line breaks, and blocks.
add(
  'a<b>b</b>c', 'a <b>b</b> c', 'a<b> b </b>c', 'a <b> b </b> c', '<b>a</b><i>b</i>',
  '<b>a</b> <i>b</i>', '<b>a </b><i>b</i>', '<b>a</b><i> b</i>', '<b> </b>', 'a<b> </b>b',
  'a<b></b>b', 'a<span>\n</span>b', '<span>a</span>\n<span>b</span>', ' <b>a</b> ',
  '<b>a</b>​<b>b</b>', 'a<br>b', 'a <br> b', 'a<br><br>b', 'a<br/>b', 'a<br />b',
  'a</br>b', '<br>', '<br><br>', '<br>a', 'a<br>', ' <br> ', 'a<br> <br>b', '<p>a<br>b</p>',
  '<p>a</p><br><p>b</p>', '<p>a</p><br>b', 'a<p>b</p><br>', '<div>a<br></div>b',
  '<div><br>a</div>', '<div><br></div>', '<div><br><br></div>x', 'x<div><br></div>y',
  '<p> a </p>', '<p>\n  a\n  b\n</p>', '<p>a</p> <p>b</p>', '<p>a</p>\n\n<p>b</p>',
  '<p>a</p>b', 'a<p>b</p>', 'a<p>b</p>c', 'a <p> b </p> c', '<p></p>', '<p> </p>',
  'a<p></p>b', 'a<p> </p>b', '<p></p><p></p>', 'a<p></p><p></p>b', '<p>a</p><p></p><p>b</p>',
  '<div>a</div><div>b</div>', '<div>a</div> <div>b</div>', '<div></div>', 'a<div></div>b',
  '<div><div></div></div>', 'a<div><div></div></div>b', '<div><p></p>hello</div>',
  '<div><p></p><h1></h1>x</div>', 'a<div><p></p><h1>T</h1></div>', '<h1><div>x</div></h1>',
  'a<h1><div>x</div></h1>b', '<div> </div>a', '<div>a</div> ', ' <div>a</div>',
  '<div>a</div>​<div>b</div>', '<div>a</div>&nbsp;<div>b</div>', 'a <div>b</div>c',
  'a<div>b</div> c', '<span>a <div>b</div> c</span>', '<b>a<p>b</p>c</b>',
  '<i>a<div>b<i>c</i></div>d</i>', '<wbr>', 'a<wbr>b', 'a <wbr> b', 'super<wbr>cali<wbr>fragi',
  '<img>', '<img src="x.png">', '<img src="x.png" alt="An image">', 'a<img alt="b">c',
  'a <img src=x> c', '<p><img src=x></p>', 'a<p><img src=x></p>b'
)

// Every formatter on its own.
const FORMATTED = [
  'p', 'div', 'article', 'aside', 'footer', 'form', 'header', 'main', 'nav', 'section',
  'blockquote', 'pre', 'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'table', 'ul', 'ol', 'a', 'span',
  'b', 'center', 'font', 'td', 'tr', 'th', 'li', 'body', 'html', 'head', 'title', 'textarea',
  'xmp', 'script', 'style', 'address', 'details', 'dl', 'dt', 'dd', 'figure', 'fieldset',
  'label', 'button', 'select', 'option', 'svg', 'math', 'tmp', 'constructor', 'x-custom',
]
for (const tag of FORMATTED) {
  add(
    `<${tag}>y</${tag}>`,
    `x<${tag}>y</${tag}>z`,
    `x <${tag}> y </${tag}> z`,
    `x<${tag}></${tag}>z`,
    `<${tag}>y\n  w</${tag}>`,
    `<${tag}>y`
  )
}
for (const tag of ['hr', 'br', 'img', 'wbr', 'input', 'meta', 'link', 'col', 'area']) {
  add(`<${tag}>`, `x<${tag}>z`, `x <${tag}> z`, `<${tag}>y</${tag}>`, `<${tag}/>y`, `x</${tag}>z`)
}

// Each in each.
const NESTED = [
  ['p', '<p>', '</p>'],
  ['div', '<div>', '</div>'],
  ['blockquote', '<blockquote>', '</blockquote>'],
  ['pre', '<pre>', '</pre>'],
  ['h1', '<h1>', '</h1>'],
  ['h4', '<h4>', '</h4>'],
  ['ul', '<ul><li>', '</li></ul>'],
  ['ol', '<ol><li>', '</li></ol>'],
  ['bare ul', '<ul>', '</ul>'],
  ['a', '<a href="https://e.example/x">', '</a>'],
  ['table', '<table><tr><td>', '</td></tr></table>'],
  ['span', '<span>', '</span>'],
  ['b', '<b>', '</b>'],
  ['article', '<article>', '</article>'],
  ['li', '<li>', '</li>'],
]
for (const [, outerOpen, outerClose] of NESTED) {
  for (const [, innerOpen, innerClose] of NESTED) {
    add(`a ${outerOpen}b ${innerOpen}c\nd${innerClose} e${outerClose} f`)
    add(`${outerOpen}${innerOpen}c${innerClose}${outerClose}`)
    add(`${outerOpen}${innerOpen}${innerClose}${outerClose}x`)
  }
}
for (const [, open, close] of NESTED) {
  add(
    `${open}a<br>b${close}`,
    `${open}<br>${close}`,
    `${open}a<hr>b${close}`,
    `${open}${open}${open}deep${close}${close}${close}`,
    `x${open}${open}${close}${close}y`,
    `${open}a${close}${open}b${close}`,
    `${open} a ${close} ${open} b ${close}`
  )
}

// Quotes: the line breaks around them are trimmed, every line starts with "> ".
add(
  '<blockquote></blockquote>', 'a<blockquote></blockquote>b', '<blockquote> </blockquote>',
  '<blockquote>a</blockquote>', '<blockquote>a<br>b</blockquote>',
  '<blockquote><br>a<br></blockquote>', '<blockquote><br><br>a<br><br></blockquote>b',
  '<blockquote><br></blockquote>', '<blockquote><br><br><br></blockquote>x',
  '<blockquote>a<br><br>b</blockquote>', '<blockquote><p>a</p><p>b</p></blockquote>',
  '<blockquote><p>a</p></blockquote><blockquote><p>b</p></blockquote>',
  '<blockquote><blockquote>a</blockquote></blockquote>',
  '<blockquote>a<blockquote>b</blockquote>c</blockquote>',
  '<blockquote><blockquote><blockquote></blockquote></blockquote></blockquote>',
  '<blockquote><br><blockquote><br>a<br></blockquote><br></blockquote>',
  '<blockquote><pre>a\n\nb\n</pre></blockquote>', '<blockquote><pre>\n\na\n\n</pre></blockquote>',
  '<blockquote><pre>\n</pre></blockquote>', '<blockquote><div><br></div><div>a</div></blockquote>',
  '<blockquote><div>a</div><div><br></div></blockquote>',
  '<blockquote><div><br></div></blockquote>', '<blockquote><p></p></blockquote>',
  '<blockquote><p></p>a</blockquote>', '<blockquote>a<p></p></blockquote>',
  '<blockquote><ul><li>a</li><li>b</li></ul></blockquote>',
  '<blockquote><ul><li>a<br>b</li></ul></blockquote>',
  '<blockquote><ul><li><br></li></ul></blockquote>',
  '<blockquote><ul><br></ul></blockquote>', '<blockquote><ul><br>a<br></ul></blockquote>',
  '<blockquote><ul><li>a<br></li></ul><br></blockquote>',
  '<ul><li><blockquote>a<br>b</blockquote></li></ul>',
  '<ul><li>x<blockquote>a<br>b</blockquote>y</li></ul>',
  '<ul><li><blockquote><br>a<br></blockquote></li><li>b</li></ul>',
  '<blockquote>a</blockquote><br><blockquote>b</blockquote>',
  '<blockquote>On Monday, someone wrote:<blockquote>An earlier message<br>on two lines</blockquote>And an answer.</blockquote>',
  '<pre><blockquote>a\nb</blockquote></pre>', '<blockquote><hr></blockquote>',
  '<blockquote><h1>Title</h1>text</blockquote>', '<blockquote><a href="u">t</a></blockquote>',
  '<blockquote><br><ul><li>a</li></ul></blockquote>', '<blockquote><ul></ul></blockquote>',
  '<blockquote><ul> </ul><br></blockquote>', '<blockquote>a<ul> </ul><br></blockquote>b'
)

// Preformatted text.
add(
  '<pre></pre>', '<pre> </pre>', '<pre>\n</pre>', '<pre>\na</pre>', '<pre>a\n</pre>',
  '<pre>a\r\nb</pre>', '<pre>a\tb</pre>', '<pre>  a  b  </pre>', '<pre>a​b</pre>',
  '<pre>a&nbsp;b &amp; c</pre>', 'x<pre>a</pre>y', 'x <pre> a </pre> y', '<pre>a<br>b</pre>',
  '<pre>a<b> b </b>c</pre>', '<pre>a<p>b</p>c</pre>', '<pre>a<div>b</div>c</pre>',
  '<pre><p>a</p><p>b</p></pre>', '<pre><p>a</p> <p>b</p></pre>', '<pre>a<div></div>b</pre>',
  '<pre><h1>title here</h1></pre>', '<pre><a href="https://e.example">link</a></pre>',
  '<pre><a href="https://e.example">https://e.example</a></pre>', '<pre><hr></pre>',
  '<pre><ul><li>a\nb</li><li>c</li></ul></pre>', '<pre><ol><li>a</li></ol></pre>',
  '<pre>a</pre><pre>b</pre>', '<p>a<pre>b</pre>c</p>', '<pre><pre>a\n b</pre></pre>',
  '<div><pre>a\n\n\nb</pre></div>', '<pre>\n\n\n</pre>x', 'x<pre>\n\n\n</pre>',
  '<pre>a<img src=x>b</pre>', '<pre><span>  a</span>\n<span>  b</span></pre>',
  '<pre>line 1\nline 2\n\tindented\n</pre>after'
)

// Headings.
add(
  '<h1>Title</h1>', '<h1>Title</h1>text', 'text<h1>Title</h1>', 'a<h1>b</h1>c<h2>d</h2>e<h3>f</h3>g',
  'a<h4>b</h4>c<h5>d</h5>e<h6>f</h6>g', '<h1>a</h1><h1>b</h1>', '<h1>a</h1><p>b</p>',
  '<p>a</p><h1>b</h1>', '<h1>mixed Case and ßharp ﬁ ǆ ŉ ΐ</h1>', '<h1>ὀδυσσεύς ς σ</h1>',
  '<h1>i̇stanbul ı İ</h1>', '<h2>&amp; &lt;tag&gt; &eacute;</h2>', '<h1>emoji 😀 ok</h1>',
  '<h1><a href="https://e.example/x">link</a></h1>', '<h1><a href="LINK">link</a></h1>',
  '<h1><a href="link">link</a></h1>', '<a href="LINK"><h1>link</h1></a>',
  '<a href="link"><h1>link</h1></a>', '<h1>a <b>b</b> c</h1>', '<h1>a<br>b</h1>',
  '<h1></h1>', 'a<h1></h1>b', '<h1> </h1>', '<h1><h2>nested</h2></h1>', '<h1>a<p>b</p>c</h1>',
  '<h1><pre>raw text</pre></h1>', '<h1><ul><li>item</li></ul></h1>', '<h1><hr></h1>',
  '<h1><img alt="x" src="y">t</h1>', '<h3>one</h3><h4>two</h4><h5>three</h5>',
  '<h1>a</h1>b<h4>c</h4>', '<div><h1>a</h1></div>b', 'x<div><h1>a</h1></div>',
  '<h1><a href="A B">a b</a></h1>', '<h1><a href="ΑΒ">αβ</a></h1>',
  '<a href="SS"><h1>ß</h1></a>', '<a href="ß"><h1>ß</h1></a>'
)

// Rules.
add(
  '<hr>', '<hr><hr>', 'a<hr>b', 'a <hr> b', '<p>a</p><hr><p>b</p>', '<hr/>', '<hr />',
  '<p>a<hr>b</p>', '<div><hr></div>', '<a href="x"><hr></a>', '<a href="----------------------------------------"><hr></a>',
  '<hr>text</hr>more', '<ul><hr></ul>', '<ul><li><hr></li></ul>', '<hr><br><hr>'
)

// Links.
const LINKS = [
  ['https://e.example/', 'text'],
  ['https://e.example/', 'https://e.example/'],
  ['https://e.example/', ' https://e.example/ '],
  ['https://e.example/', 'https://e.example'],
  ['https://e.example/', 'https:// e.example/'],
  ['https://e.example/', 'https://<b>e.example</b>/'],
  ['https://e.example/', 'HTTPS://E.EXAMPLE/'],
  ['https://e.example/', ''],
  ['https://e.example/', ' '],
  ['https://e.example/', '<img src="i.png" alt="logo">'],
  ['https://e.example/', '<br>'],
  ['https://e.example/', '<p>para</p>'],
  ['https://e.example/', 'two words'],
  ['mailto:a@b.example', 'a@b.example'],
  ['mailto:a@b.example', 'Write to us'],
  ['mailto:', 'nothing'],
  ['mailto:#x', 'fragment'],
  ['MAILTO:a@b.example', 'upper'],
  ['mailto:mailto:a', 'a'],
  ['#top', 'Back to top'],
  ['#', 'hash'],
  ['', 'empty'],
  [' ', 'space'],
  ['/path', 'relative'],
  ['a b', 'a b'],
  ['a b', 'ab'],
  ['ab', 'a b'],
  [' a ', 'a'],
  ['a​b', 'ab'],
  ['a b', 'a b'],
  ['a&amp;b', 'a&amp;b'],
  ['a&b', 'a&amp;b'],
  ['https://e.example/?a=1&amp;b=2', 'query'],
  ['https://e.example/?a=1&b=2', 'query'],
  ['https://e.example/?a=1&copy=2', 'legacy entity'],
  ['https://e.example/?a=1&copy;=2', 'entity'],
  ['&#104;ttp://e', 'http://e'],
  ['x&#x20;y', 'numeric space'],
  ['[x]', '[x]'],
  ['😀', '😀'],
  ['é', 'é'],
  ['javascript:void(0)', 'click'],
  ['https://e.example/a\nb', 'newline in href'],
]
for (const [href, text] of LINKS) {
  add(`<a href="${href}">${text}</a>`, `before <a href="${href}">${text}</a> after`)
}
add(
  '<a>no href</a>', '<a name="x">anchor</a>', '<a href>bare</a>', '<a href=>empty</a>',
  '<a href=x>unquoted</a>', "<a href='x y'>single</a>", '<a href=x/>slash</a>',
  '<a href=x y=z>two</a>', '<a HREF="x">upper</a>', '<A Href="x">mixed</A>',
  '<a href="x" href="y">first wins</a>', '<a href href="y">empty first</a>',
  '<a title="t" href="x">other</a>', '<a\nhref="x"\n>newlines</a>', '<a\thref\t=\t"x">tabs</a>',
  '<a href = "x">spaces</a>', '<a href="x"title="t">glued</a>', '<a href="a"b">quote</a>',
  '<a href=a"b>quote in unquoted</a>', '<a href="x>unterminated</a>', '<a href="x"',
  '<a href="x">unclosed', '<a href="x"><a href="y">nested</a></a>',
  '<a href="x">a<a href="y">b</a>c</a>', '<a href="b"><a href="b">b</a></a>',
  '<a href="ab"><a href="b">a</a>b</a>', '<a href="x"><a href="x">x</a></a>',
  '<a href="xy">x<a href="y">y</a></a>', '<a href="x"><b><a href="y"><i>deep</i></a></b></a>',
  '<a href="u"><div>block</div></a>', '<a href="u"><p>a</p><p>b</p></a>',
  '<a href="u"><ul><li>a</li><li>b</li></ul></a>', '<a href="ab"><ul><li>a</li><li>b</li></ul></a>',
  '<a href="*a*b"><ul><li>a</li><li>b</li></ul></a>', '<a href="u"><pre>pre text</pre></a>',
  '<a href="u"><pre></pre></a>', '<a href="u"><blockquote>q</blockquote></a>',
  '<a href="q"><blockquote>q</blockquote></a>', '<a href="u">a</a><a href="v">b</a>',
  '<a href="u">a</a> <a href="v">b</a>', '<a href="u"> a </a><a href="v"> b </a>',
  '<p><a href="u">a</a></p><p><a href="v">b</a></p>', '<a href="u">a<br>b</a>',
  '<a href="ab">a<br>b</a>', '<a href="u"><img src="i"></a>', '<a href="u"><img src="i"> </a>',
  '<a href="u">​</a>', '<a href="u">&nbsp;</a>', '<a href=" ">&nbsp;</a>',
  '<a href="u"><script>s</script></a>', '<a href="u"><!-- c --></a>', '<a href="u">a<!-- c -->b</a>',
  '<a href="ab">a<!-- c -->b</a>', '<a href="a b">a<!-- c --> b</a>', '<a href="u"><wbr></a>',
  '<a href="https://e.example/a">https://<wbr>e.example/<wbr>a</a>',
  '<a href="https://e.example/a">https://e.example/a</a>.',
  'See <a href="https://e.example/a">https://e.example/a</a>, or <a href="https://e.example/b">this</a>.',
  '<a href="x"></a>', '<a href="x"></a>y', 'w<a href="x"></a>y', 'w <a href="x"></a> y',
  '<a href="x"> </a>y', '<td><a href="u">a</a></td><td><a href="v">b</a></td>',
  '<a href="u">a</a>b', '<a href="u">a</a> b', '<a href="u">a </a>b', 'b<a href="u">a</a>',
  '<a href="u" >a</a>', '<a href="tel:+33123456789">Call</a>', '<a href="u">A</a><h1><a href="u">u</a></h1>',
  '<h1><a href="U">u</a></h1>', '<h1><a href="u">u</a></h1>', '<a href="U"><h1>u</h1></a>',
  '<a href="Uv"><h1>u</h1>v</a>', '<a href="uV"><h1>u</h1>v</a>'
)

// Lists.
add(
  '<ul><li>a</li><li>b</li></ul>', '<ul>\n  <li>a</li>\n  <li>b</li>\n</ul>',
  'x<ul><li>a</li></ul>y', 'x <ul> <li> a </li> </ul> y', '<ul></ul>', 'x<ul></ul>y',
  '<ul> </ul>', 'x<ul> \n </ul>y', '<ul>&nbsp;</ul>', 'x<ul>&nbsp;</ul>y', '<ul>​</ul>',
  'x<ul>​</ul>y', '<ul>text</ul>', '<ul>text<li>a</li></ul>', '<ul><li>a</li>text</ul>',
  '<ul><li>a</li> text <li>b</li></ul>', '<ul><!-- c --><li>a</li></ul>', '<ul><!-- c --></ul>',
  'x<ul><!-- c --></ul>y', '<ul><li>a</li><!-- c --><li>b</li></ul>', '<ul><?pi?><li>a</li></ul>',
  '<ul><script>s</script><li>a</li></ul>', '<ul><style>s</style></ul>', '<ul><div>a</div><div>b</div></ul>',
  '<ul><p>a</p><li>b</li></ul>', '<ul><b>a</b><li>b</li></ul>', '<ul><li></li></ul>',
  '<ul><li></li><li></li></ul>', 'x<ul><li></li><li></li></ul>y', '<ul><li> </li></ul>',
  '<ul><li>a<br>b</li></ul>', '<ul><li>a<br></li><li>b</li></ul>', '<ul><li><br>a</li></ul>',
  '<ul><li><p>a</p><p>b</p></li><li><p>c</p></li></ul>', '<ul><li><p>a</p></li></ul>',
  '<ul><li><div>a</div></li><li><div>b</div></li></ul>', '<ul><li><h1>a</h1></li></ul>',
  '<ul><li>a<ul><li>b</li><li>c</li></ul></li><li>d</li></ul>',
  '<ul><li><ul><li>b</li></ul></li></ul>', '<ul><li>a<ul><li>b<ul><li>c</li></ul></li></ul></li></ul>',
  '<ul><ul><li>a</li></ul></ul>', '<ul><ul><ul>a</ul></ul></ul>', '<ul><ul></ul></ul>',
  '<ul><li>a</li><ul><li>b</li></ul><li>c</li></ul>', '<ol><li>a<ol><li>b</li></ol></li></ol>',
  '<ul><li>a<ol><li>b</li><li>c</li></ol></li></ul>', '<ol><li>a<ul><li>b</li></ul>c</li></ol>',
  '<li>a</li><li>b</li>', '<li>a<ul><li>b</li></ul></li>', '<li><ol><li>a</li></ol></li>',
  '<div><li><ul><li>a</li></ul></li></div>', '<ul><li><span><ul><li>not nested</li></ul></span></li></ul>',
  '<ul><li>a</ul>b', '<ul><li>a<li>b<li>c</ul>', '<ul><li>a<p>b<li>c</ul>',
  '<ol><li>a</li><li>b</li></ol>', '<ol></ol>', '<ol> </ol>', '<ol>a</ol>', '<ol><div>a</div></ol>',
  '<ol><li>a</li><div>b</div><li>c</li></ol>', '<ol><li>1</li><li>2</li><li>3</li><li>4</li><li>5</li><li>6</li><li>7</li><li>8</li><li>9</li><li>10</li></ol>',
  '<ol><li>a<br>b</li><li>c</li><li>d</li><li>e</li><li>f</li><li>g</li><li>h</li><li>i</li><li>j</li><li>k<br>l</li></ol>',
  '<ol start="98"><li>a</li><li>b<br>c</li><li>d</li></ol>',
  '<ul><li>a</li></ul><ul><li>b</li></ul>', '<ul><li>a</li></ul>text<ol><li>b</li></ol>',
  '<p>a</p><ul><li>b</li></ul><p>c</p>', '<ul><li>a</li></ul><br><ul><li>b</li></ul>',
  '<ul><li><a href="u">link</a></li><li><a href="v">v</a></li></ul>',
  '<ul><li>a</li> <li>b</li></ul>', '<ul><li>a</li>​<li>b</li></ul>',
  '<ul><li>a</li> <li>b</li></ul>', '<ul><li>a</li>﻿<li>b</li></ul>',
  '<ul><li>a</li>\u0085<li>b</li></ul>', '<ul><LI>upper</LI></ul>', '<UL><li>upper list</li></UL>',
  '<ul><li>a</li></ul></li>stray', '<ul><li><pre>a\nb</pre></li><li>c</li></ul>',
  '<ul><li><pre>\na\n</pre></li></ul>', '<ul><li><blockquote>q</blockquote></li></ul>',
  '<ul><li>a<hr>b</li></ul>', '<ul><br><li>a</li></ul>', '<ul><li>a</li><br></ul>',
  '<ul><br></ul>', '<ul><br><br></ul>x', '<ul><img src=x></ul>', 'x<ul><img src=x></ul>y',
  '<ul><wbr><li>a</li></ul>', '<ul><li>a</li><hr><li>b</li></ul>',
  '<ol><li>a</li><ol><li>b</li></ol></ol>', '<ol><li><ol><li><ol><li>deep</li></ol></li></ol></li></ol>',
  '<ul><li><ol start=9><li>a<li>b</ol></li></ul>', '<dl><dt>term</dt><dd>definition</dd></dl>',
  '<ul><li>one<div></div></li><li>two</li></ul>', '<ul><li><div></div></li><li>two</li></ul>',
  '<ul><li><div></div><div></div>x</li></ul>', '<ul><div></div><li>a</li></ul>',
  '<ul><div></div></ul>', 'x<ul><div></div></ul>y', '<ul><div></div><div></div></ul>y',
  '<ul><li>a</li><div></div></ul>y', '<ul><p></p><p></p>a</ul>'
)
for (const start of [
  '1', '0', '-1', '-0', '5', '09', ' 7 ', '+3', '3.5', '.5', '5.', '1e3', '1E2', '1e21', '1e-7',
  '123456789012345678901234567890', '0x10', '0X1f', '0b101', '0o17', '0x', '0xg', '-0x10',
  'abc', '', ' ', 'Infinity', '-Infinity', '+Infinity', 'infinity', 'NaN', '1,5', '1_0', '1 2',
  '0x20000000000001', '0x20000000000003', '0xfffffffffffff800', '0xfffffffffffffc00',
  '0x1fffffffffffff8', '9007199254740993', '0.1', '1e400', '-1e400', '4.35', '0.000001',
  '0.0000001', '123456789.125', ' 5 ', '　5', '5​', '2147483648', '4294967296',
  '1e10', '1e15', '26', '27', '702', '703', '18278', '3999', '4000', '4999', '5000', '8999', '9000',
  '9999', '10000', '14000', '40000', '49999', '90000', '-5', '-1000', '-4000', '1.5e3',
  '1.e2', '.5e1', '5.e-1', '1e', '1e+', 'e5', '.', '-.', '+.5', '--5', '1e5.5', '0x1p3', '0b12',
  '0o8', '0X', '00x10', '1e309', '1e-324', '5e-324', '2.5e-324', '1.7976931348623157e308',
  '1.7976931348623159e308', '0.1e1', '100e-2', '١٢', '１２',
]) {
  add(`<ol start="${start}"><li>a</li><li>b</li></ol>`)
  for (const type of ['a', 'A', 'i', 'I']) {
    add(`<ol type="${type}" start="${start}"><li>a</li><li>b</li><li>c</li></ol>`)
  }
}
add(
  '<ol type="1"><li>a</li></ol>', '<ol type=""><li>a</li></ol>', '<ol type="x"><li>a</li></ol>',
  '<ol type="a "><li>a</li></ol>', '<ol type="A" type="i"><li>a</li></ol>',
  '<ol TYPE="A" START="3"><li>a</li></ol>', '<ol type="i"><li>1</li><li>2</li><li>3</li><li>4</li><li>5</li><li>6</li><li>7</li><li>8</li><li>9</li><li>10</li></ol>',
  '<ol type="a" start="25"><li>y</li><li>z</li><li>aa</li><li>ab</li></ol>',
  '<ol type="I" start="3998"><li>a</li><li>b</li><li>c</li></ol>',
  '<ol type="i" start="9999"><li>a</li><li>b</li></ol>',
  '<ol type="i" start="9998"><li>a</li><div>b</div><div>c</div></ol>',
  '<ol reversed start="5"><li>a</li><li>b</li></ol>', '<ol start="5" start="9"><li>a</li></ol>',
  '<ol start><li>a</li></ol>', '<ol start=""><li>a</li></ol>', '<ol start="&#53;"><li>a</li></ol>',
  '<ul type="a" start="5"><li>a</li></ul>', '<li><ol type="I" start="5"><li>a</li></ol></li>',
  '<ul><li><ol type="a" start="-3"><li>a</li><li>b</li><li>c</li><li>d</li><li>e</li></ol></li></ul>',
  '<li><ol type="i" start="0"><li>a</li><li>b</li></ol></li>', '<ol type="i" start="0"><li>a</li><li>b</li></ol>'
)

// Tables are containers here: cells and rows are inline.
add(
  '<table><tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table>',
  '<table>\n<tr>\n<td>a</td>\n<td>b</td>\n</tr>\n<tr>\n<td>c</td>\n</tr>\n</table>',
  '<table><tr><th>Head</th></tr><tr><td>Cell</td></tr></table>', 'x<table></table>y',
  '<table><tbody><tr><td><p>a</p></td><td><p>b</p></td></tr></tbody></table>',
  '<table><tr><td><table><tr><td>inner</td></tr></table></td><td>outer</td></tr></table>',
  '<table><tr><td>a<td>b<tr><td>c</table>', '<table><thead><tr><th>h<tbody><tr><td>b<tfoot><tr><td>f</table>',
  '<table class="data" id="t"><tr><td>a</td></tr></table>', '<table><caption>Caption</caption><tr><td>a</td></tr></table>',
  '<table><tr><td><div>a</div></td><td><div>b</div></td></tr></table>',
  '<table><tr><td>a</td></tr></table><table><tr><td>b</td></tr></table>',
  '<p>a<table><tr><td>b</td></tr></table>c</p>', '<table><td>a</td> <td>b</td></table>',
  '<table><tr><td> a </td><td> b </td></tr></table>', '<td>a</td><td>b</td>', '<tr><td>a</td></tr>x'
)

// Entities, in text and in attributes.
add(
  '&amp;', '&lt;b&gt;', '&quot;&apos;', '&amp', '&ampx', '&amp;amp;', '&AMP;', '&AMP', '&Amp;',
  '&copy;', '&copy', '&copyx', '&copy1', '&copy=', '&not', '&notin;', '&notin', '&notit;',
  '&notx', '&eacute;', '&eacute', '&Eacute;', '&euro;', '&euro', '&hellip;', '&mdash;&ndash;',
  '&nbsp', '&nbspx', '&zwj;&zwnj;&lrm;&rlm;&shy;', '&thinsp;&ensp;&emsp;', '&Afr;', '&nGt;',
  '&NotEqualTilde;', '&bne;', '&fjlig;', '&ThickSpace;', '&CounterClockwiseContourIntegral;',
  '&CounterClockwiseContourIntegra;', '&x;', '&;', '&', '& ', '&&', '&&amp;', 'a&b', 'a & b',
  'AT&T', 'a&amp', '&#65;', '&#65', '&#65x', '&#x41;', '&#X41;', '&#x41', '&#x41g', '&#;', '&#',
  '&#x;', '&#x', '&#xg;', '&#a;', '&#0;', '&#00;', '&#128;', '&#x80;', '&#150;', '&#159;',
  '&#x9f;', '&#129;', '&#xD800;', '&#xDFFF;', '&#x10FFFF;', '&#x110000;', '&#1114112;',
  '&#99999999999999999999;', '&#xFFFFFFFFFFFFFFFFFFFFFF;', '&#x1F600;', '&#128512;',
  '&#0065;', '&#x0041;', '&#32;a&#32;b', 'a&#10;b', 'a&#13;&#10;b', 'a&#9;b', 'a&#12;b',
  'a&#x200b;b', 'a&#8203;b', 'a&#xa0;b', '&lt;script&gt;alert(1)&lt;/script&gt;',
  '&amp;lt;', '&#38;amp;', '&am', '&a', '&amp', 'x&', 'x&#', 'x&#1', 'x&#x1', 'x&a', 'x&am',
  'x&no', 'x&not', 'x&noti', 'x&notin', '<p>&amp', '<p>&notit', '<p>&#65',
  '<a href="&amp;">t</a>', '<a href="&amp">t</a>', '<a href="&ampx">t</a>', '<a href="&amp=">t</a>',
  '<a href="&copy">t</a>', '<a href="&copyx">t</a>', '<a href="&copy=1">t</a>',
  '<a href="&copy;=1">t</a>', '<a href="&not">t</a>', '<a href="&notin">t</a>',
  '<a href="&notit;">t</a>', '<a href="&notx">t</a>', '<a href="&not-">t</a>',
  '<a href="?a&lang=en&amp;b&reg=1&reg">t</a>', '<a href=&amp;>t</a>', '<a href=a&amp>t</a>',
  '<a href=&#65;&#x42>t</a>', '<a href="&#65">t</a>', '<a href="&#65x">t</a>', '<a href="&#x41g">t</a>',
  '<a href="&#">t</a>', '<a href="&#x">t</a>', '<a href="&">t</a>', '<a href="&;">t</a>',
  '<a href="&unknown;">t</a>', "<a href='&quot;&apos;'>t</a>", '<a href="&Afr;">t</a>',
  '<a href="&Afr;">&Afr;</a>', '<a href="&nGt;">&nGt;</a>', '<a href="&#0;">t</a>',
  '<a href="&amp', '<a href="x&am', '<a href=&amp', '<a href="&#65', '<a href=x&copy',
  '<title>&amp; &copy &notit; &#65;</title>', '<textarea>&amp; &copy</textarea>',
  '<script>&amp;</script>x&amp;', '<style>&amp;</style>y', '<xmp>&amp;<b></xmp>',
  '<title>a&amp</title>', '<title>&</title>', '<title>&amp;</title>b', '<title>x&am',
  '<p title="&amp;">&amp;</p>', '<p>&ampersand;</p>', '&Aacute&aacute&Acirc', '&AElig&aelig',
  '&gt&lt&GT&LT', '&gta', '&ltb', '&quot', '&QUOT', '&reg', '&REG', '&shy', '&szlig', '&yen&yuml',
  '&sect&uml&copy&ordf&laquo&not&shy&reg&macr&deg&plusmn&sup2&sup3&acute&micro&para&middot',
  '&frac12&frac14&frac34', '&iquest&times&divide', '&notindot;', '&notinE;', '&notinva;',
  '&lang;', '&lang', '&rang;', '&para;graph', '&paragraph', '&curren;cy', '&currency',
  '&centerdot;', '&centerdot', '&cent;erdot', '&times;b', '&timesb;', '&timesb', '&timesbar;'
)

// Comments, CDATA sections, doctypes and processing instructions.
add(
  '<!-- comment -->', 'a<!-- comment -->b', 'a <!-- comment --> b', 'a<!---->b', 'a<!-->b',
  'a<!--->b', 'a<!----->b', 'a<!-- -- -->b', 'a<!-- > -->b', 'a<!-- --!>b-->c', 'a<!--b',
  'a<!--', 'a<!-', 'a<!', 'a<!>b', 'a<!x>b', 'a<!x', 'a<!->b', 'a<!-x>b', 'a<!-- <p>b</p> -->c',
  '<!--[if mso]><p>outlook</p><![endif]-->x', '<!--[if !mso]><!--><p>others</p><!--<![endif]-->x',
  '<![if !mso]><p>a</p><![endif]>', '<!DOCTYPE html><p>a</p>', '<!doctype html>\n<html><body>a</body></html>',
  '<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Transitional//EN" "http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd"><html xmlns="http://www.w3.org/1999/xhtml"><body>a</body></html>',
  '<!DOCTYPE html', '<?xml version="1.0" encoding="UTF-8"?><p>a</p>', 'a<?php echo 1 ?>b',
  'a<?>b', 'a<?', 'a<?x', 'a<![CDATA[b]]>c', 'a<![CDATA[<p>b</p>]]>c', 'a<![CDATA[b]]]>c',
  'a<![CDATA[b]]', 'a<![CDATA[', 'a<![CDAT', 'a<![cdata[b]]>c', 'a<![CDATAb]]>c',
  '<svg><![CDATA[b]]></svg>c', 'a<!---->', '<!-- a --><!-- b -->', 'a<!-- b -->c<!-- d -->e',
  '<p>a<!-- x --></p>b', '<pre>a<!-- x -->b</pre>', '<ul><li>a<!-- x --></li></ul>',
  '<p>a</p><!-- x --><p>b</p>', '<p>a</p> <!-- x --> <p>b</p>', 'a </b> b', 'a<!-- --> </b> b'
)

// Case of names, tags left open, tags closed that were not open, tags closed out of order.
add(
  '<P>upper</P>', '<DIV>a</DIV><Div>b</dIV>', '<H1>Title</H1>', '<BR>a<Br>b', '<UL><LI>a</LI></UL>',
  '<BLOCKQUOTE>q</BLOCKQUOTE>', '<PRE>a\n b</PRE>', '<BODY>a</BODY>b', '<SCRIPT>s</SCRIPT>a',
  '<Style>s</sTYLE>a', '<TITLE>t</TITLE>a', '<TEXTAREA><b></TEXTAREA>a', '<IMG SRC=x>a',
  '<p>a', '<p>a<p>b', '<p>a<div>b', '<div>a<p>b</div>c', '<b>a<i>b</b>c</i>d', '<b>a<p>b</b>c</p>d',
  '<div>a<span>b</div>c</span>d', '<p>a<b>b<p>c</b>d', '<ul><li>a<li>b</ul>c', '<h1>a<h2>b</h1>c</h2>d',
  '</p>', 'a</p>b', '<div></p></div>', '<p></p></p>', 'a</p></p>b', '</div>', 'a</div>b',
  'a</b>b', 'a </b> b', 'a</b></i></u>b', '</br>', 'a</br>b', 'a</br/>b', 'a</BR>b', 'a</hr>b',
  'a</img>b', 'a</x>b', 'a</x y="z">b', 'a</x/>b', 'a</ x>b', 'a</\nx>b', '</>', 'a</>b',
  'a</ >b', 'a</3>b', 'a</3', 'a</', 'a</x', 'a</x ', 'a</x y', 'a</x y>b', 'a<', 'a<b', 'a<b ',
  'a<b c', 'a<b c=', 'a<b c="', 'a<b c="d', 'a<b c="d"', 'a<b c="d" ', 'a<b /', 'a<b / ',
  'a<b c /', 'a<b c="d"/', 'a<b c=d/', 'a<bé>b', 'a<1>b', 'a< b>c', 'a<=b', 'a<>b', 'a<<b>c',
  'a<b<c>d', 'a<b>c>d', 'a>b', '1 < 2 and 3 > 2', 'if (a<b && c>d)', '<<p>>a<</p>>',
  'abc</div ', 'abc</div x', 'abc</div 😀', 'abc<div /', 'abc<div / ', 'abc<div a="b"/',
  'abc<div a=b /x', '<s', '<S', '<sc', '<scr', '<script', '<st', '<sty', '<t', '<ti', '<te',
  '<tm', '<x', '<xm', '<xmp', '<sx', '<tx', 'a<s>b</s>c', 'a<t>b</t>c', 'a<x>b</x>c',
  'a<span>b', 'a<strong>b</strong>c', 'a<table>b', 'a<tt>b</tt>c', 'a<time>b</time>c',
  '<p/>a', '<div/>a', '<span/>a', '<br/>a', '<a href="x"/>a', '<p / >a', '<p/ >a', '<p /x>a',
  '<div><p>a</div>b</p>c', '<p><div>a</div></p>b', '<a href="x"><div><a href="y">b</div>c</a>d',
  '<b><b><b>a</b>b</b>c</b>d', '<b>a</b></b>b', '<i><b>a</i>b</b>c', '<p>a</P>b', '<P>a</p>b',
  '<x-a>a<x-b>b</x-a>c</x-b>d', '<X-A>a</x-a>b', '<é>a</é>', '<aé>a</aé>b', '<aÉ>a</aé>b',
  '<aİ>a</ai̇>b', '<linK>a', '<bloKquote>a</blockquote>', '<aı>a</aı>',
  '<p >a', '<p​>a', '<br >a', '<o:p>a</o:p>b', '<o:p></o:p>', '<v:shape/>a'
)

// Where the text to convert is: the body elements, or everything.
add(
  '<html><head><title>T</title></head><body>B</body></html>', '<title>T</title><p>P</p>',
  '<head><title>T</title></head>x', '<html><head><title>T</title></head>x</html>',
  'before<body>inside</body>after', '<body>a</body><body>b</body>', '<body>a<body>b</body>c</body>d',
  '<body><p>a</p></body><body><p>b</p></body>', '<body>a</body> <body> b</body>',
  '<div><body>a</body></div>b', '<p>x<body>a</body>y</p>', '<body></body>x', '<body>', '<body>a',
  '<BODY>a</BODY>b', '<body class="c" style="margin:0">a</body>', '<html><body>a</body></html>b',
  '<html>a<body>b</body>c</html>', '<script><body>a</body></script>b', '<script/><body>a</body>b',
  '<style/><body>a</body>b', '<script/><p>a</p></script>b', '<body><script/>a</script>b</body>',
  '<svg><body>a</body></svg>b', '<body>a</body><!-- c -->', '<head>h<body>b', '<head><link><body>b',
  '<link><body>b</body>', '<script>s<body>b', '<p><script><body>b</body></script>c</p>',
  '<head><script>x</script><body>b', '<body><ul><li>a</li></ul></body>', '<li><body><ul><li>a</li></ul></body></li>',
  '<body>a</body>b<body>c</body>', '<body><body><body>deep</body></body></body>x',
  '<frameset><frame src="a"></frameset><noframes><body>n</body></noframes>',
  '<html><head><meta charset="utf-8"><style>body{margin:0}</style></head><body><div>a</div></body></html>',
  '<body>a</body>&amp;', '<body>a<', '<body>a</body', '<body', 'x<body', '<bod>a</bod>b',
  '<body/>a</body>b', '<svg><body/>a</svg>b'
)

// An element named `constructor` makes the library throw, where it formats it.
add(
  '<constructor>', 'a<constructor>b', '<CONSTRUCTOR>x</CONSTRUCTOR>', '<constructor/>',
  '<p><b><constructor></constructor></b></p>', '<constructor><body>a</body></constructor>',
  '<body><constructor>x</constructor></body>', '<body>a</body><constructor>x</constructor>',
  '<constructor>x</constructor><body>a</body>', '<script/><constructor>x</constructor></script>y',
  '<style/><constructor>x</constructor>', '<ul><constructor></constructor></ul>',
  '<constructors>x</constructors>', '<construct>x</construct>', '</constructor>x',
  '<a href="constructor">constructor</a>', '<constructor', '<prototype>x</prototype>',
  '<tostring>x</tostring>', '<valueof>x</valueof>', '<hasownproperty>x</hasownproperty>',
  '<a constructor="x" href="y">z</a>', '<ol __proto__="x" start="2"><li>a</li></ol>',
  '<a __proto__ href="y">z</a>', '<a tostring="x" valueof="y">z</a>'
)

// Raw text elements, and the names the tokenizer mistakes for them.
add(
  '<script>var a = "<p>x</p>";</script>y', '<script>a</script>b<script>c</script>d',
  '<script type="text/javascript">a</script>b', '<script>a</scrip>b</script>c',
  '<script>a</script >b', '<script>a</script\n>b', '<script>a</script x>b', '<script>a</scriptx>b</script>c',
  '<script>a</SCRIPT>b', '<script>a<</script>b', '<script>a</</script>b', '<script>a\u001c\u000fscript>b',
  '<script>a<!--</script>-->b', '<script>a', '<script>a</script', '<script>a</scr', '<script', '<script ',
  '<script>', '<script></script>', 'x<script></script>y', 'x <script>s</script> y',
  '<style>a{}</style>b', '<style>a</styl>b</style>c', '<style>a', '<style><!-- a --></style>b',
  '<style>a</style><style>b</style>c', '<p>a<style>s</style>b</p>', '<p>a<script>s</script>b</p>',
  '<title>a<b>c</b></title>d', '<title>a</title >b', '<title>a', '<title>a</titl', '<title></title>x',
  '<textarea>a<b>c</b>\n d</textarea>e', '<textarea>a', '<textarea>a</textarea', '<xmp>a<b>c</b></xmp>d',
  '<xmp>a', '<tmp>x<b>y</b></xmp>z', '<tmp>x<b>y</b></tmp>z', '<tmp>a', '<xitle>a<b>c</b></title>d',
  '<xextarea>a<b></textarea>c', '<tmpx>a<b>c</b></tmpx>d', '<titlex>a<b>c</b></titlex>d',
  '<scripts>a<b>c</b></scripts>d', '<styles>a</styles>b', '<script/>a<b>c</b></script>d',
  '<style/>a</style>b', '<title/>a<b>c</b></title>d', '<textarea/>a<b>c</b></textarea>d',
  '<script />a', '<script a="b"/>c', '<script a=b/>c</script>d', '<svg><script/>a</svg>b',
  '<svg><title>a<b>c</b></title></svg>d', '<svg><style>a</style></svg>b', '<SCRIPT>a</script>b',
  '<sCrIpT>a</ScRiPt>b', '<TITLE>a&amp;</TITLE>b', '<plaintext>a<b>c</b>', '<noscript>a<b>c</b></noscript>d',
  '<iframe>a<b>c</b></iframe>d', '<noembed>a</noembed>b', '<template><p>a</p></template>b',
  '<pre><script>s</script>a</pre>', '<ul><script>s</script></ul>x', '<a href="u"><style>s</style>t</a>'
)

// Self-closing tags only close in svg and math, and what follows a self-closed svg.
add(
  '<svg/>a', '<svg />a<div/>b</div>c', '<svg/><div/>a<p>b</p></div>c', '<math/>a<p/>b</p>c',
  '<svg><path d="M0 0"/><circle/>a</svg>b', '<svg><p/>a</svg>b', '<svg><p>a</svg>b',
  '<svg><title/>a</svg>b', '<svg><desc>a<p/>b</desc>c</svg>d', '<svg><foreignObject><p/>a</p></foreignObject></svg>b',
  '<math><mi/>a<mo>b</mo></math>c', '<svg></svg><p/>a</p>b', '<svg/></svg><p/>a</p>b',
  '</svg><p/>a</p>b', '</title><p/>a', '<title>t</title><p/>a</p>b', '<svg><svg/></svg><p/>a</p>b',
  '<svg/><svg/></svg></svg><p/>a</p>b', '<svg><br/>a</svg>b', '<svg/><br/>a<br/>b',
  '<svg/><ul/>a<li/>b</ul>c', '<svg/><a href="x"/>a', '<svg/><blockquote/>a', '<svg/><pre/>a\n b',
  '<svg/><h1/>title', '<svg/><script/>a<b>c</b>', '<svg/><p/><p/><p/>a', '<mi/><p/>a', '<desc><p/>a</desc>',
  '<svg><desc/><p/>a</svg>'
)

// Mail as it is sent.
add(
  `<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Transitional//EN" "http://www.w3.org/TR/xhtml1/DTD/xhtml1-transitional.dtd">
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:v="urn:schemas-microsoft-com:vml" xmlns:o="urn:schemas-microsoft-com:office:office">
<head>
<!--[if gte mso 9]><xml><o:OfficeDocumentSettings><o:AllowPNG/><o:PixelsPerInch>96</o:PixelsPerInch></o:OfficeDocumentSettings></xml><![endif]-->
<meta http-equiv="Content-Type" content="text/html; charset=UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Your order has shipped</title>
<style type="text/css">
  body { margin: 0; padding: 0; } table td { border-collapse: collapse; }
  @media only screen and (max-width: 480px) { .wrap { width: 100% !important; } }
</style>
</head>
<body style="margin:0;padding:0;background-color:#f4f4f4">
<div style="display:none;max-height:0;overflow:hidden">Your parcel is on its way&zwnj;&nbsp;&zwnj;&nbsp;&zwnj;&nbsp;&zwnj;&nbsp;&zwnj;&nbsp;&zwnj;&nbsp;&zwnj;&nbsp;</div>
<center>
<table border="0" cellpadding="0" cellspacing="0" width="100%" class="wrap">
  <tr>
    <td align="center" valign="top">
      <table border="0" cellpadding="0" cellspacing="0" width="600">
        <tr><td style="padding:20px"><a href="https://shop.example/"><img src="https://shop.example/logo.png" alt="Shop" width="120"></a></td></tr>
        <tr><td><h1 style="font-family:Arial">Your order has shipped</h1></td></tr>
        <tr><td><p>Hello Dave,</p><p>Order <b>#12345</b> left our warehouse <i>today</i>.<br>Track it <a href="https://track.example/12345?utm_source=mail&amp;utm_medium=email">here</a>.</p></td></tr>
        <tr><td>
          <table width="100%"><tr><th align="left">Item</th><th align="right">Price</th></tr>
          <tr><td>Blue mug &times; 2</td><td align="right">&euro;18.00</td></tr>
          <tr><td>Shipping</td><td align="right">&euro;4.90</td></tr></table>
        </td></tr>
        <tr><td align="center"><table><tr><td bgcolor="#0055ff" style="border-radius:4px"><a href="https://shop.example/orders/12345" style="color:#fff;text-decoration:none"><span style="padding:12px">VIEW ORDER</span></a></td></tr></table></td></tr>
        <tr><td style="font-size:11px;color:#999"><p>Shop Ltd &middot; 1 Road &middot; Town<br><a href="https://shop.example/unsubscribe?u=abc">Unsubscribe</a> | <a href="https://shop.example/prefs">Preferences</a></p></td></tr>
      </table>
    </td>
  </tr>
</table>
</center>
<img src="https://track.example/open.gif?id=abc" width="1" height="1" alt="">
</body>
</html>`,
  `<html><body><div dir="ltr">Hi,<div><br></div><div>See you <b>tomorrow</b> at 10.</div><div><br></div><div>Thanks,</div><div>Ann</div></div><br><div class="gmail_quote"><div dir="ltr" class="gmail_attr">On Mon, 5 Oct 2026 at 09:12, Dave &lt;<a href="mailto:dave@example.com">dave@example.com</a>&gt; wrote:<br></div><blockquote class="gmail_quote" style="margin:0 0 0 .8ex;border-left:1px #ccc solid;padding-left:1ex"><div>Are we still on?</div><div><br></div><blockquote class="gmail_quote"><div>Yes, I think so.</div></blockquote></blockquote></div></body></html>`,
  `<html><head><meta http-equiv="Content-Type" content="text/html; charset=utf-8"><style><!-- p.MsoNormal { margin:0cm } --></style></head><body lang="EN-GB" link="#0563C1" vlink="#954F72"><div class="WordSection1"><p class="MsoNormal">Dear all,<o:p></o:p></p><p class="MsoNormal"><o:p>&nbsp;</o:p></p><p class="MsoNormal">Please find the minutes attached.<o:p></o:p></p><p class="MsoNormal"><o:p>&nbsp;</o:p></p><p class="MsoNormal"><span style="font-size:10.0pt">Kind regards<o:p></o:p></span></p><div><div style="border:none;border-top:solid #E1E1E1 1.0pt;padding:3.0pt 0cm 0cm 0cm"><p class="MsoNormal"><b>From:</b> Bob &lt;bob@example.com&gt; <br><b>Sent:</b> 04 October 2026 18:02<br><b>To:</b> Team<br><b>Subject:</b> Minutes<o:p></o:p></p></div></div><p class="MsoNormal"><o:p>&nbsp;</o:p></p><ol style="margin-top:0cm" start="1" type="1"><li class="MsoListParagraph">First point<o:p></o:p></li><li class="MsoListParagraph">Second point<o:p></o:p></li></ol></div></body></html>`,
  `<body><table width="100%"><tr><td><font face="Arial" size="2" color="#333333"><span><span><span>Deals of the <font color="red"><b>week</b></font></span></span></span></font></td></tr><tr><td><center><font size="1">You receive this because you <a href="https://news.example/why">subscribed</a>.</font></center></td></tr></table><img src="https://news.example/p.gif?x=1&y=2" height=1 width=1></body>`,
  `<div style="font-family:Helvetica">
    <h2>Weekly digest</h2>
    <ul>
      <li><a href="https://news.example/1">First story</a> &mdash; a short summary of it.</li>
      <li><a href="https://news.example/2">Second story</a> &mdash; another summary.</li>
      <li>No link here</li>
    </ul>
    <hr>
    <p style="color:#888">You can <a href="https://news.example/unsubscribe">unsubscribe</a> at any time.</p>
  </div>`,
  `<html><body><p>Bonjour,</p><p>Voici le r&eacute;capitulatif de votre commande n&deg;&nbsp;4521&nbsp;:</p><table><tr><td>Caf&eacute; moulu</td><td>12,50&nbsp;&euro;</td></tr><tr><td>Th&eacute; vert</td><td>8,00&nbsp;&euro;</td></tr></table><p>&Agrave; bient&ocirc;t&nbsp;!<br>L'&eacute;quipe</p></body></html>`,
  `<html><body><pre style="white-space:pre-wrap">Build #482 failed

  src/main.rs:12:5  error[E0308]: mismatched types
  expected \`u32\`, found \`&str\`

See <a href="https://ci.example/b/482">https://ci.example/b/482</a>
</pre></body></html>`,
  `<body><p>Hi</p><p>-- <br>Sent from my phone</p></body>`,
  `<html><head></head><body><div><div><div><div><div><span><span><span><span></span></span></span></span></div></div></div></div></div><div>   </div><table><tbody><tr><td></td><td>&nbsp;</td></tr></tbody></table><p>Only this</p></body></html>`,
  `<body><table role="presentation"><tr><td><table role="presentation"><tr><td><table role="presentation"><tr><td><p>Deep <span>inside <b>three</b></span> tables</p></td></tr></table></td><td><p>Next cell</p></td></tr></table></td></tr><tr><td><p>Second row</p></td></tr></table></body>`,
  `<html>\r\n<body>\r\n<p>Windows\r\nline endings</p>\r\n<pre>keep\r\nthem</pre>\r\n</body>\r\n</html>\r\n`,
  `<body><h1>Invoice</h1><h2>Details</h2><p>Amount: <strong>$120.00</strong></p><h3>Notes</h3><blockquote>Paid on receipt.<br>Thank you.</blockquote><h4>Contact</h4><p><a href="mailto:billing@example.com">billing@example.com</a> or <a href="tel:+15550100">+1 555 0100</a></p><h5>Legal</h5><h6>Fine print</h6></body>`
)

// The shapes that are slow or large upstream, small enough for Node to finish.
add(
  '<i>'.repeat(500),
  `${'<i>'.repeat(300)}${'</b>'.repeat(300)}text`,
  `${'<i>'.repeat(300)}text${'</b>'.repeat(300)}more`,
  `${'<blockquote>'.repeat(20)}<pre>${'x\n'.repeat(30)}`,
  `${'<blockquote>'.repeat(40)}a<br>b`,
  `${'<blockquote>'.repeat(30)}${'<br>'.repeat(50)}`,
  `${'<blockquote>'.repeat(30)}x${'<br>'.repeat(50)}`,
  `${'<blockquote>'.repeat(30)}${'<br>'.repeat(50)}x${'</blockquote>'.repeat(15)}y`,
  `${'<a href="https://x.example/y">'.repeat(40)}${'x '.repeat(200)}`,
  `${'<a href="xxxxxxxxxx">'.repeat(12)}${'x'.repeat(1)}${'</a>x'.repeat(12)}`,
  `${'<a href="xxxx">'.repeat(5)}x x<b>x</b>x${'</a>'.repeat(5)}`,
  `${'<div>'.repeat(300)}deep${'</div>'.repeat(300)}after`,
  `${'<div>x'.repeat(200)}${'</div>y'.repeat(200)}`,
  `${'<ul><li>'.repeat(60)}deep`,
  `${'<ol><li>'.repeat(40)}a<br>b`,
  `${'<ul>'.repeat(200)}a<br>b`,
  `${'<ul><li>a'.repeat(30)}${'</li><li>b</li></ul>'.repeat(30)}`,
  `${'<p>'.repeat(400)}x`,
  `${'<p>a'.repeat(100)}`,
  `${'<li>a'.repeat(100)}`,
  `${'<td>a'.repeat(100)}`,
  `${'<h1>a'.repeat(50)}`,
  `${'<b>a</i>'.repeat(200)}`,
  `${'</p>'.repeat(200)}x`,
  `x${'</br>'.repeat(50)}y`,
  `${'<hr>'.repeat(30)}`,
  `<p>Hello</p><pre>${' '.repeat(2000)}</pre><p>hidden</p>`,
  `<table>${'<tr><td style="padding:0 12px">An offer, with a <a href="https://shop.example/deals">link to follow</a>.</td></tr>'.repeat(20)}</table>`,
  `${'<span>'.repeat(400)}a${'</span>'.repeat(400)}`,
  `${'<svg>'.repeat(100)}${'<p/>'.repeat(10)}a`,
  `${'<title>'.repeat(3)}a</title>b`,
  `<ol type="I" start="1">${'<li>x</li>'.repeat(120)}</ol>`,
  `<ol type="a" start="1">${'<li>x</li>'.repeat(60)}</ol>`,
  `<ul>${'<!-- c -->'.repeat(40)}<li>a</li></ul>`,
  `${'&amp;'.repeat(300)}`,
  `${'&'.repeat(300)}`,
  `${'&#'.repeat(300)}`,
  `&#${'0'.repeat(600)}65;`,
  `${'<'.repeat(300)}`,
  `${'</'.repeat(300)}`,
  `${'<a '.repeat(300)}`,
  `<a ${'b '.repeat(300)}href="x" ${'c=d '.repeat(300)}>text</a>`,
  `${'<!--'.repeat(100)}-->a`,
  `${'<![CDATA['.repeat(100)}]]>a`
)

// Every named character reference, as the decoder of htmlparser2 knows them: the names
// are found by asking it which prefixes it would read on from.
const fromHtmlparser2 = createRequire(
  createRequire(
    `${REPOSITORY}/node_modules/.pnpm/html-to-text@10.0.1/node_modules/html-to-text/lib/html-to-text.mjs`
  ).resolve('htmlparser2')
)
const { EntityDecoder, DecodingMode, determineBranch, htmlDecodeTree } =
  fromHtmlparser2('entities/decode')

function entityNames() {
  const decoder = new EntityDecoder(htmlDecodeTree, () => {})
  const read = (text) => {
    decoder.startEntity(DecodingMode.Legacy)
    return decoder.write(text, 0)
  }
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789'
  const names = []
  const prefixes = []
  let level = ['']
  while (level.length) {
    const longer = []
    for (const prefix of level) {
      for (const letter of alphabet) {
        const name = prefix + letter
        // -1: the decoder wants more characters, so some entity starts like this.
        if (read(name) !== -1) continue
        prefixes.push(name)
        if (read(`${name};`) === name.length + 2) names.push(name)
        // A node that holds its value in place of branches has none, though the decoder
        // looks for them all the same: see `strayPaths`.
        if ((htmlDecodeTree[decoder.treeIndex] & 0xc000) >> 14 !== 1) longer.push(name)
      }
    }
    level = longer
  }
  return { names, prefixes }
}
const { names: ENTITIES, prefixes: ENTITY_PREFIXES } = entityNames()
if (ENTITIES.length < 2000 || ENTITIES.length > 2300) throw new Error(`${ENTITIES.length} entities found`)
// What follows a name decides whether it is an entity: nothing, a letter, a digit, `=`.
for (const end of [';', '', '=', 'x', '1', '&']) {
  add(ENTITIES.map((name) => `&${name}${end}`).join('|'))
  add(`<a href="${ENTITIES.map((name) => `&${name}${end}`).join('|')}">t</a>`)
}
const somePrefixes = (start) => ENTITY_PREFIXES.filter((_, index) => index % 4 === start)
add(
  `<title>${ENTITIES.map((name) => `&${name}`).join('|')}</title>`,
  `<title>${ENTITIES.map((name) => `&${name}x;`).join('|')}</title>`,
  somePrefixes(0).map((prefix) => `&${prefix};`).join('|'),
  somePrefixes(1).map((prefix) => `&${prefix}`).join('|'),
  `<a href="${somePrefixes(2).map((prefix) => `&${prefix};`).join('|')}">t</a>`,
  `<a href="${somePrefixes(3).map((prefix) => `&${prefix}`).join('|')}">t</a>`,
  `<a href='${somePrefixes(0).map((prefix) => `&${prefix}=`).join('|')}'>t</a>`,
  `<a href=${ENTITIES.map((name) => `&${name}`).join('|')}>t</a>`,
  // The document ends inside the entity.
  ...ENTITIES.filter((_, index) => index % 40 === 0).flatMap((name) => [
    `a&${name}`,
    `a&${name.slice(0, -1)}`,
    `<a href="&${name}`,
    `<title>&${name}`,
  ])
)

/**
 * The decoder of `entities` 7.0.1 looks for branches in every node of its trie, also in
 * those that hold their value where the others hold their branches. From such a node it
 * walks on through words that are no nodes: `&BreveX;` is `Á`. Text made for it can lead
 * it anywhere in the trie, and past its end. This follows `determineBranch` from every
 * node it can reach and returns, for each, characters that lead there.
 */
function strayPaths() {
  const VALUE_LENGTH = 0xc000
  const FLAG13 = 0x2000
  const BRANCH_LENGTH = 0x1f80
  const JUMP_TABLE = 0x7f
  const paths = new Map([[0, '']])
  const queue = [0]
  const reach = (node, path) => {
    if (paths.has(node) || Number.isNaN(node) || node === undefined) return
    paths.set(node, path)
    queue.push(node)
  }
  while (queue.length) {
    const node = queue.pop()
    const path = paths.get(node)
    const current = htmlDecodeTree[node]
    if (current === undefined) continue
    const valueLength = (current & VALUE_LENGTH) >> 14
    if (valueLength === 0 && (current & FLAG13) !== 0) {
      // A run of characters without branches.
      const runLength = (current & BRANCH_LENGTH) >> 7
      let run = String.fromCharCode(current & JUMP_TABLE)
      for (let index = 0; index + 1 < runLength; index++) {
        const packed = htmlDecodeTree[node + 1 + (index >> 1)] ?? 0
        run += String.fromCharCode(index % 2 === 0 ? packed & 0xff : (packed >> 8) & 0xff)
      }
      reach(node + 1 + (runLength >> 1), path + run)
      continue
    }
    // No word of the trie holds a key above 255, and no jump table reaches 191.
    for (let unit = 0; unit < 256; unit++) {
      const next = determineBranch(htmlDecodeTree, current, node + Math.max(1, valueLength), unit)
      if (next < 0) continue
      reach(next, path + String.fromCharCode(unit))
    }
  }
  return [...paths.values()].filter((path) => path !== '')
}
const STRAY_PATHS = strayPaths()
// Each path as far as it goes, cut short, and with one more character, in the three
// places an entity is decoded in. All of them when asked for, a sample otherwise.
function strayCases(every) {
  const htmls = []
  for (const [index, path] of STRAY_PATHS.entries()) {
    if (index % every !== 0) continue
    const quote = path.includes('"') ? "'" : '"'
    for (const text of [`${path};`, path, `${path}=`, `${path}a;`, `${path.slice(0, -1)};`, `${path}\u00e9;`]) {
      htmls.push(`a&${text}b`, `<a href=${quote}&${text}${quote}>t</a>`, `<title>&${text}</title>`)
    }
  }
  return htmls
}
if (argument('--stray')) {
  const fixtures = strayCases(1).map((html) => ({ html, ...expected(html) }))
  writeFileSync(argument('--stray'), JSON.stringify(fixtures))
  console.log(`${argument('--stray')}: ${fixtures.length} inputs`)
  process.exit(0)
}
add(...strayCases(223))

// Every character with an upper case of its own, in a heading; and with a lower case of
// its own, in a tag name that its end tag spells in lower case. Node and Rust must agree
// on the Unicode they know.
const UPPERED = []
const LOWERED = []
for (let point = 0x80; point <= 0x10ffff; point++) {
  if (point >= 0xd800 && point <= 0xdfff) continue
  const character = String.fromCodePoint(point)
  if (character.toUpperCase() !== character) UPPERED.push(character)
  if (character.toLowerCase() !== character) LOWERED.push(character)
}
add(
  `<h1>${UPPERED.join(' ')}</h1>`,
  `<h1>${UPPERED.join('')}</h1>`,
  `<h1>${UPPERED.map((character) => `a${character}b`).join(' ')}</h1>`,
  UPPERED.map((character) => `<a href="${character.toUpperCase()}"><h1>${character}</h1></a>`).join(' '),
  LOWERED.map((character) => `<div><a${character}><p>x</a${character.toLowerCase()}>y</div>`).join(''),
  LOWERED.map((character) => `<div><a${character}b><p>x</a${character.toLowerCase()}b>y</div>`).join(''),
  LOWERED.map((character) => `<div><a${character}><p>x</a${character}>y</div>`).join(''),
  '<div><aΣ><p>x</aς>y</div><div><aΣb><p>x</aσb>y</div><div><aΣ-><p>x</aς->y</div>'
)

// What a list does not make an item of: text that is only whitespace, as `\s` has it.
for (const point of [
  0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x20, 0x85, 0xa0, 0x1680, 0x180e, 0x2000, 0x2005, 0x200a, 0x200b,
  0x200c, 0x2028, 0x2029, 0x202f, 0x205f, 0x2060, 0x3000, 0xfeff, 0x1c, 0x1f,
]) {
  const space = String.fromCodePoint(point)
  add(`<ul><li>a</li>${space}<li>b</li></ul>`, `<ol start="${space}7${space}"><li>a</li></ol>`)
}

// Numbers as `Number` reads them and `toString` writes them.
{
  const next = random(1234)
  const digits = (count) => Array.from({ length: count }, () => Math.floor(next() * 10)).join('')
  const starts = []
  for (let index = 0; index < 400; index++) {
    const kind = Math.floor(next() * 8)
    const sign = next() < 0.2 ? '-' : ''
    if (kind === 0) starts.push(sign + digits(1 + Math.floor(next() * 30)))
    else if (kind === 1) starts.push(`${sign}${digits(1 + Math.floor(next() * 8))}.${digits(1 + Math.floor(next() * 20))}`)
    else if (kind === 2) starts.push(`${sign}${digits(1 + Math.floor(next() * 3))}e${Math.floor(next() * 60) - 30}`)
    else if (kind === 3) starts.push(`${sign}${1 + Math.floor(next() * 9)}.${digits(1 + Math.floor(next() * 17))}e${Math.floor(next() * 700) - 350}`)
    else if (kind === 4) starts.push('0x' + Array.from({ length: 1 + Math.floor(next() * 40) }, () => '0123456789abcdefABCDEF'[Math.floor(next() * 22)]).join(''))
    else if (kind === 5) starts.push('0b' + Array.from({ length: 1 + Math.floor(next() * 80) }, () => (next() < 0.5 ? '0' : '1')).join(''))
    else if (kind === 6) starts.push('0o' + Array.from({ length: 1 + Math.floor(next() * 30) }, () => Math.floor(next() * 8)).join(''))
    else starts.push(String((next() - 0.5) * 10 ** (Math.floor(next() * 40) - 20)))
  }
  for (const [index, start] of starts.entries()) {
    const type = ['1', '1', 'a', 'A', 'i', 'I'][index % 6]
    add(`<ol type="${type}" start="${start}"><li>a</li><li>b</li></ol>`)
  }
}

write('cases.json', cases.map((html) => ({ html, ...expected(html) })))
write('soup.json', soups(3000, 20261007))

// ---------------------------------------------------------------------------------------
// Inputs too large to store
// ---------------------------------------------------------------------------------------

/** FNV-1a over the UTF-8 of the text: small enough to write the same way in the tests. */
function hash(text) {
  let value = 0xcbf29ce484222325n
  for (const byte of Buffer.from(text, 'utf8')) {
    value = BigInt.asUintN(64, (value ^ BigInt(byte)) * 0x100000001b3n)
  }
  return value.toString(16).padStart(16, '0')
}

/** A recipe is a list of `[text, times]`: the texts repeated, one after the other. */
const build = (recipe) => recipe.map(([text, times]) => text.repeat(times)).join('')

const ROW =
  '<tr><td style="padding:0 12px;font-family:Helvetica,Arial,sans-serif">An offer, with a <a href="https://shop.example/deals">link to follow</a>.</td></tr>'
/** Deeper than Node goes on its own, whatever is nested. */
const DEEP = 3500
const large = [
  // The tests of the Node server.
  { name: 'newsletter', recipe: [['<html><head><style>', 1], ['p{color:red}', 2000], ['</style></head><body><table>', 1], [ROW, 6000], ['</table></body></html>', 1]] },
  { name: 'rules', recipe: [['<hr>', 100_000]] },
  { name: 'spaces in pre', recipe: [['<p>Hello</p><pre>', 1], [' ', 990_000], ['</pre><p>hidden</p>', 1]] },
  // What the Node server gives up on, for the time or the memory it takes there.
  { name: 'tags never closed', deep: true, recipe: [['<i>', 300_000]] },
  { name: 'end tags that close nothing', deep: true, recipe: [['<i>', 100_000], ['</b>', 100_000]] },
  { name: 'links in links', deep: true, recipe: [['<a href="https://x.example/y">', 200], ['x ', 400_000]] },
  // The fourth shape is 400 MB of text with 400,000 lines: this is the same with fewer.
  { name: 'quotes around lines', recipe: [['<blockquote>', 500], ['<pre>', 1], ['x\n', 2000]] },
  // Large, and nothing special.
  { name: 'paragraphs', recipe: [['<p>One paragraph of <b>text</b> &amp; an entity.</p>\n', 20_000]] },
  { name: 'line breaks', recipe: [['a<br>', 200_000]] },
  { name: 'list items', recipe: [['<ol>', 1], ['<li>item</li>', 50_000], ['</ol>', 1]] },
  { name: 'words', recipe: [['word ', 190_000]] },
  { name: 'one word', recipe: [['<h1>', 1], ['é', 400_000], ['</h1>', 1]] },
  { name: 'entities', recipe: [['&amp;&nbsp;&#x41;&notit;', 40_000]] },
  { name: 'links', recipe: [['<a href="https://e.example/a">https://e.example/a</a> <a href="https://e.example/b">b</a> ', 10_000]] },
  { name: 'quoted lines', recipe: [['<blockquote>', 10], ['line<br>', 20_000]] },
  { name: 'pre lines in a list', recipe: [['<ul><li><pre>', 1], ['x\n', 300_000]] },
  // More than `limits.maxInputLength`, 16,777,216 UTF-16 code units, of which the library
  // only reads as many. The second is cut in the middle of a character.
  { name: 'past the input limit', recipe: [['<pre>', 1], ['ab\n', 5_600_000]] },
  { name: 'past the input limit, in a character', recipe: [['<pre>', 1], ['😀', 8_400_000]] },
  { name: 'past the input limit, in words', recipe: [['ab ', 5_600_000]] },
  // Deep, where the text of a block is copied into the block around it at every level.
  { name: 'deep inline', deep: true, recipe: [['<span>', DEEP], ['a <b>b</b> c', 1]] },
  { name: 'deep blocks', deep: true, recipe: [['<div>', DEEP], ['a b<br>c', 1], ['</div>', DEEP], ['after', 1]] },
  { name: 'deep blocks with text', deep: true, recipe: [['<div>x', DEEP], ['</div>y', DEEP >> 1]] },
  { name: 'deep paragraphs in sections', deep: true, recipe: [['<section><p>p</p>', DEEP], ['x', 1]] },
  { name: 'deep links', deep: true, recipe: [['<a href="https://x.example/y">', DEEP], ['x ', 50]] },
  { name: 'deep links to their text', deep: true, recipe: [['<a href="xx">', DEEP], ['x<b>x</b>', 1]] },
  { name: 'deep quotes', deep: true, recipe: [['<blockquote>', DEEP], ['a<br>b<br>', 1]] },
  { name: 'deep quotes around pre', deep: true, recipe: [['<blockquote>', DEEP], ['<pre>', 1], ['x\n', 5]] },
  { name: 'deep lists', deep: true, recipe: [['<ul><li>', DEEP], ['a<br>b', 1]] },
  { name: 'deep lists without items', deep: true, recipe: [['<ul>', DEEP], ['a<br>b<br> c', 1]] },
  { name: 'deep numbered lists', deep: true, recipe: [['<ol start=9><li>', DEEP], ['a<br>b', 1]] },
  { name: 'deep headings', deep: true, recipe: [['<h1><span>', DEEP], ['title', 1]] },
  { name: 'deep pre', deep: true, recipe: [['<pre>', DEEP], [' a\n b ', 1]] },
  { name: 'deep tables', deep: true, recipe: [['<table><tr><td>', DEEP], ['cell', 1]] },
  { name: 'deep everything', deep: true, recipe: [['<div><blockquote><ul><li><a href="u"><b>', 500], ['x y<br>z', 1]] },
]
const largeFixtures = []
for (const { name, recipe, deep } of large) {
  const html = build(recipe)
  const { text } = deep ? await expectedWithStack(html) : expected(html)
  largeFixtures.push({
    name,
    recipe,
    bytes: Buffer.byteLength(text, 'utf8'),
    hash: hash(text),
    // The first 80 characters, not code units: what `chars().take(80)` is in the tests.
    start: Array.from(text.slice(0, 160)).slice(0, 80).join(''),
  })
}
write('large.json', largeFixtures)
