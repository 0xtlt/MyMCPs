// Writes src/mime_host_tables.rs: what the URL host parser of Node 24.21 knows
// about each character of a domain label.
//
//   node crates/icloud-mail/tools/mime_host_tables.mjs
//
// nodemailer encodes a domain with `url.domainToASCII`, which Node runs on the
// ada library (4.0.0 here). ada maps characters with the tables of Unicode 17,
// but checks a label with older tables of its own: as far as can be told, the
// Bidi classes and the combining marks of Unicode 13, the combining classes
// and the compositions of Unicode 15, and the joining types of a few scripts.
// No crate has that mix, and Node does not expose the tables, so they are read
// from the outside: each character is put in labels that the parser takes or
// refuses depending on one property of it.
import { writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import url from 'node:url'

if (process.versions.ada !== '4.0.0' || process.versions.unicode !== '17.0') {
  throw new Error(
    `The port follows ada 4.0.0 with Unicode 17.0 (Node 24.21), and this is ada ${process.versions.ada} with Unicode ${process.versions.unicode}`
  )
}

// A second label that only mapping turns into "a". A domain with a label the
// parser refuses comes back empty, and one that is all ASCII is not checked.
const WITNESS = '.ａ'
const takes = (label) => url.domainToASCII(label + WITNESS) !== ''
const keeps = (label) => url.domainToUnicode(label + WITNESS) === `${label}.a`

// Letters that compose with nothing: a left-to-right one that joins with
// nothing, a right-to-left one, and a left-to-right one that joins both ways.
const KA = 'क'
const ALEF = 'א'
const MONGOLIAN_A = 'ᠠ'
const VIRAMA = '्'
const ZWNJ = '‌'
const ZWJ = '‍'

const CLASSES = ['OTHER', 'RIGHT_TO_LEFT', 'ARABIC_NUMBER', 'EUROPEAN_NUMBER', 'NONSPACING_MARK', 'NEUTRAL']
const COMBINING = 1 << 3
const VIRAMA_FLAG = 1 << 4
const JOINS_NEXT = 1 << 5
const JOINS_PREVIOUS = 1 << 6
const INERT = 1 << 7

/** Whether mapping keeps the character, so that it can be in a label when the label is checked. */
function isKept(c) {
  return keeps(KA + c + KA) || keeps(ALEF + c + ALEF)
}

/** The flags of one character, or null for one that is never in a checked label. */
function flagsOf(cp) {
  const c = String.fromCodePoint(cp)
  // The two joiners have rules of their own. All that is asked of one is
  // whether it joins with the non-joiner before it.
  if (c === ZWNJ || c === ZWJ) return takes(MONGOLIAN_A + ZWNJ + c) ? JOINS_PREVIOUS : 0
  // The dot separates labels.
  if (c === '.' || !isKept(c)) return null

  // The Bidi rule only tells these classes apart. A label with a right-to-left
  // character is refused whether another one is left-to-right or unknown.
  const inside = takes(ALEF + c + ALEF)
  const last = takes(ALEF + c)
  const afterLeftToRight = takes(KA + c)
  const withEuropeanNumber = takes(`${ALEF}1${c}${ALEF}`)
  const withArabicNumber = takes(`${ALEF}١${c}${ALEF}`)
  const pattern = [inside, last, afterLeftToRight, withEuropeanNumber, withArabicNumber].map(Number).join('')
  const direction = {
    '11011': 'RIGHT_TO_LEFT',
    '11001': 'ARABIC_NUMBER',
    '11110': 'EUROPEAN_NUMBER',
    '11111': 'NONSPACING_MARK',
    '10111': 'NEUTRAL',
    '00100': 'OTHER',
  }[pattern]
  if (!direction) throw new Error(`U+${cp.toString(16)} is taken in a way no Bidi class explains: ${pattern}`)

  let flags = CLASSES.indexOf(direction)
  // A joiner after a virama ends the check with a yes, unless the label
  // starts with a combining mark.
  if (!takes(c + KA + VIRAMA + ZWJ)) flags |= COMBINING
  const isVirama = takes(KA + c + ZWJ)
  if (isVirama) flags |= VIRAMA_FLAG
  if (!isVirama && takes(KA + c + ZWNJ + MONGOLIAN_A)) flags |= JOINS_NEXT
  if (takes(MONGOLIAN_A + ZWNJ + c)) flags |= JOINS_PREVIOUS
  return flags
}

const flags = new Map()
for (let cp = 0; cp <= 0x10ffff; cp++) {
  if (cp >= 0xd800 && cp <= 0xdfff) continue
  const value = flagsOf(cp)
  if (value !== null) flags.set(cp, value)
}

// Normalization. Node's own ICU has the combining classes and the
// compositions of Unicode 17. Where ada does not order a mark or compose a
// pair as ICU does, the character is one ada has no data for: to ada it is an
// ordinary character that nothing combines with.
const adaNormalizes = (label) => {
  const host = url.domainToUnicode(label + WITNESS)
  return host === '' ? null : host.slice(0, -2)
}
const inert = new Set()
for (const cp of flags.keys()) {
  const c = String.fromCodePoint(cp)
  for (const base of [KA, ALEF]) {
    // After a mark of class 230, and before one of class 1.
    for (const label of [`${base}́${c}`, `${base}${c}̴`]) {
      const normalized = adaNormalizes(label)
      if (normalized !== null && normalized !== label.normalize('NFC')) inert.add(cp)
    }
  }
}
const unknownPairs = []
for (let cp = 0x80; cp <= 0x10ffff; cp++) {
  // Hangul syllables are composed by arithmetic, in ada as everywhere.
  if ((cp >= 0xd800 && cp <= 0xdfff) || (cp >= 0xac00 && cp <= 0xd7a3)) continue
  const composed = String.fromCodePoint(cp)
  const parts = [...composed.normalize('NFD')]
  if (parts.length < 2) continue
  const first = parts.slice(0, -1).join('').normalize('NFC')
  const second = parts[parts.length - 1]
  if ([...first].length !== 1 || (first + second).normalize('NFC') !== composed) continue
  // Mapping replaces some of these characters before anything is composed.
  if (!flags.has(first.codePointAt(0)) || !flags.has(second.codePointAt(0))) continue
  const normalized = adaNormalizes(first + second)
  if (normalized === first + second) unknownPairs.push([first, second, composed])
  else if (normalized !== null && normalized !== composed) {
    throw new Error(`ada composes U+${cp.toString(16)} in a way of its own`)
  }
}
for (const [first, second, composed] of unknownPairs) {
  // The composed character is one ada does not take apart, which ICU would,
  // to put it together again with what follows.
  if (flags.has(composed.codePointAt(0))) inert.add(composed.codePointAt(0))
  // Making either character of the pair inert stops the composition. The
  // second one is the choice when ada composes it with nothing else.
  const secondComposes = [...flags.keys()].some((cp) => {
    const starter = String.fromCodePoint(cp)
    const pair = starter + second
    const expected = pair.normalize('NFC')
    return expected !== pair && adaNormalizes(pair) === expected
  })
  inert.add((secondComposes ? first : second).codePointAt(0))
}
for (const cp of inert) {
  if (!flags.has(cp)) throw new Error(`U+${cp.toString(16)} is inert but never in a label`)
  flags.set(cp, flags.get(cp) | INERT)
}

// Ranges of characters with the same flags. A character that is never in a
// checked label takes the flags of its neighbours, which keeps the ranges few.
const ranges = []
for (const [cp, value] of [...flags].sort(([a], [b]) => a - b)) {
  const range = ranges[ranges.length - 1]
  if (range && range.flags === value) range.end = cp
  else ranges.push({ start: cp, end: cp, flags: value })
}
const listed = ranges.filter((range) => range.flags !== 0)

const hex = (cp) => `0x${cp.toString(16).toUpperCase().padStart(4, '0')}`
const lines = [
  '//! What the URL host parser of Node 24.21 (ada 4.0.0) knows about the',
  '//! characters of a domain label.',
  '//!',
  '//! Generated by `tools/mime_host_tables.mjs`, which reads it off `url.domainToASCII`.',
  '//! Do not edit.',
  '',
  '/// The classes the Bidi rule tells apart, in the low bits of the flags. A',
  '/// character that is not listed is of the first: left-to-right, or unknown to ada.',
  ...CLASSES.map((name, index) => `pub(crate) const ${name}: u8 = ${index};`),
  'pub(crate) const CLASS_MASK: u8 = 0b111;',
  '/// A combining mark, which a label does not start with.',
  `pub(crate) const COMBINING: u8 = 1 << 3;`,
  '/// A virama, which a zero width joiner or non-joiner may follow.',
  `pub(crate) const VIRAMA: u8 = 1 << 4;`,
  '/// Joins with the character after it (Joining_Type L or D).',
  `pub(crate) const JOINS_NEXT: u8 = 1 << 5;`,
  '/// Joins with the character before it (Joining_Type R or D).',
  `pub(crate) const JOINS_PREVIOUS: u8 = 1 << 6;`,
  '/// A character ada has no normalization data for: it is not reordered as a',
  '/// mark and composes with nothing.',
  `pub(crate) const INERT: u8 = 1 << 7;`,
  '',
  '/// First character, last character and flags of each range, in order.',
  '#[rustfmt::skip]',
  `pub(crate) static CHARACTERS: [(u32, u32, u8); ${listed.length}] = [`,
  ...listed.map((range) => `    (${hex(range.start)}, ${hex(range.end)}, 0b${range.flags.toString(2).padStart(8, '0')}),`),
  '];',
  '',
]
const here = dirname(fileURLToPath(import.meta.url))
writeFileSync(join(here, '../src/mime_host_tables.rs'), lines.join('\n'))
console.log(`${flags.size} characters in ${listed.length} ranges, ${inert.size} of them inert, written to src/mime_host_tables.rs`)
console.log('pairs ada does not compose:', unknownPairs.map((pair) => pair.map((c) => 'U+' + c.codePointAt(0).toString(16)).join(' ')).join(', '))
console.log('inert:', [...inert].sort((a, b) => a - b).map((cp) => cp.toString(16)).join(' '))
