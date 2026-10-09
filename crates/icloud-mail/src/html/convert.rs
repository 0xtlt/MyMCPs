//! The text of an HTML mail body, exactly as the npm package `html-to-text` 10.0.1 writes it.
//!
//! The Node server called `convert(html, OPTIONS)` in a process of its own, because markup
//! written for the purpose keeps that library busy for seconds or makes it allocate
//! hundreds of megabytes. This is the same conversion done in the server: in one pass over
//! the document and one over its tree, without recursion, and within a [`Budget`].
//!
//! What is fixed, and therefore not ported:
//!
//! - the options are `{ wordwrap: false, selectors: [{ selector: 'a', options: {
//!   hideLinkHrefIfSameAsText: true } }, { selector: 'img', format: 'skip' }] }` over the
//!   defaults of the library. Lines are never wrapped, so `wbr` and `longWordSplit` do
//!   nothing; `baseElements` is `body`; no table is a data table; `encodeCharacters`,
//!   `preserveNewlines`, `baseUrl`, `pathRewrite`, `limits.maxDepth`, `maxChildNodes` and
//!   `maxBaseElements` keep their defaults, which is to say they do nothing;
//! - the parser is htmlparser2 10.1.0 in HTML mode with `decodeEntities`, fed the whole
//!   document as one chunk, its tree is the one domhandler 5.0.3 builds, and character
//!   references are decoded as `entities` 7.0.1 does.
//!
//! The JavaScript is the specification, including where it is surprising: each part below
//! names the function it ports. `tests/differential.rs` holds this to the text Node gives.
//!
//! Where the two part ways:
//!
//! - html-to-text calls itself for every element inside another, and Node runs out of
//!   stack between 1,000 and 3,000 elements deep, so that the Node server had no text for
//!   such a document. This has the text the library would give with the stack for it;
//! - a string of JavaScript can hold half a surrogate pair, a string of Rust cannot. Two
//!   quirks of upstream make one (see `handle_trailing_data` and `emit_named_entity`), and
//!   so does `maxInputLength` when it cuts a character in two. Each is U+FFFD here from
//!   the start, which is what it becomes when Node writes the text as UTF-8. The text
//!   only differs when an anchor compares one with a U+FFFD of the document:
//!   `<a href="\u{fffd}"></x 😀` shows the address to Node and not here.

use std::borrow::Cow;
use std::collections::HashMap;
use std::time::Instant;

mod entity_tree;

use entity_tree::HTML_DECODE_TREE;

/// What a conversion may spend. Both are checked as it goes, so a message written to be
/// slow or huge is given up on instead of being finished.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// Give up once this instant has passed.
    pub deadline: Option<Instant>,
    /// Give up once the converter holds more than this many bytes.
    ///
    /// Counted: the capacity of everything whose size follows the document, which is the
    /// tree (16 bytes a node), the decoded text and link addresses of the tree, the names
    /// of elements HTML does not define, the stacks of open elements and open blocks, and
    /// the text being written, which is the result. Not counted: `html` itself, which the
    /// caller holds, and the copy a reallocation keeps while it moves a buffer.
    pub max_bytes: usize,
}

/// Why a document has no text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GaveUp {
    /// The deadline of the [`Budget`] passed.
    #[error("the conversion took too long")]
    TooSlow,
    /// The tree or the text needs more bytes than the [`Budget`] allows.
    #[error("the conversion needs too much memory")]
    TooLarge,
    /// `html-to-text` itself throws on this document, so there is no text to match: an
    /// element named `constructor`, which its selector lookup mistakes for a selector, or
    /// a list numbered in Roman numerals past 9999.
    #[error("html-to-text throws on this document")]
    Throws,
}

/// `convert(html, OPTIONS)` of html-to-text.
///
/// The text is whole: the caller cuts it. Nothing written is ever taken back, so what a
/// conversion has written when it gives up is a prefix of the full text.
pub fn convert(html: &str, budget: &Budget) -> Result<String, GaveUp> {
    let mut meter = Meter::new(budget);
    meter.check_time()?;
    let html = within_input_limit(html, &mut meter)?;
    let dom = Parser::new(&html, &mut meter).parse()?;
    Walk::new(&dom, &mut meter).run()
}

/// `limits.maxInputLength`: `process` keeps the first 16,777,216 UTF-16 code units.
const MAX_INPUT_UNITS: usize = 1 << 24;

/// `html.substring(0, maxInputLength)`, in UTF-16 code units as JavaScript counts.
fn within_input_limit<'a>(html: &'a str, meter: &mut Meter) -> Result<Cow<'a, str>, GaveUp> {
    // A code unit is at least one byte.
    if html.len() <= MAX_INPUT_UNITS {
        return Ok(Cow::Borrowed(html));
    }
    let mut units = 0;
    for (at, character) in html.char_indices() {
        let after = units + character.len_utf16();
        if after > MAX_INPUT_UNITS {
            let kept = html.get(..at).unwrap_or_default();
            if units == MAX_INPUT_UNITS {
                return Ok(Cow::Borrowed(kept));
            }
            // The cut falls inside a surrogate pair. JavaScript is left with half a
            // character, which leaves Node as U+FFFD when the text is written as UTF-8.
            let mut cut = String::new();
            meter.room_in(&mut cut, kept.len() + REPLACEMENT.len())?;
            cut.push_str(kept);
            cut.push_str(REPLACEMENT);
            return Ok(Cow::Owned(cut));
        }
        units = after;
    }
    Ok(Cow::Borrowed(html))
}

const REPLACEMENT: &str = "\u{fffd}";

// ---------------------------------------------------------------------------------------
// The budget
// ---------------------------------------------------------------------------------------

/// How much work is done between two looks at the clock: about a byte read or written
/// for each unit. Small enough to stop within a millisecond of the deadline, large enough
/// that reading the clock costs nothing.
const WORK_BETWEEN_CLOCK_READS: usize = 1 << 16;

/// Counts the time and the memory of one conversion against its [`Budget`].
struct Meter {
    deadline: Option<Instant>,
    max_bytes: usize,
    held: usize,
    work_left: usize,
}

impl Meter {
    fn new(budget: &Budget) -> Self {
        Self {
            deadline: budget.deadline,
            max_bytes: budget.max_bytes,
            held: 0,
            work_left: WORK_BETWEEN_CLOCK_READS,
        }
    }

    fn check_time(&self) -> Result<(), GaveUp> {
        match self.deadline {
            Some(deadline) if Instant::now() >= deadline => Err(GaveUp::TooSlow),
            _ => Ok(()),
        }
    }

    /// Accounts for `work` bytes read or written.
    fn spend(&mut self, work: usize) -> Result<(), GaveUp> {
        if work < self.work_left {
            self.work_left -= work;
            return Ok(());
        }
        self.work_left = WORK_BETWEEN_CLOCK_READS;
        self.check_time()
    }

    /// Accounts for bytes held outside the buffers `room` and `room_in` grow.
    fn hold(&mut self, bytes: usize) -> Result<(), GaveUp> {
        if bytes > self.max_bytes.saturating_sub(self.held) {
            return Err(GaveUp::TooLarge);
        }
        self.held += bytes;
        Ok(())
    }

    /// Accounts for bytes that are held no more.
    fn release(&mut self, bytes: usize) {
        self.held = self.held.saturating_sub(bytes);
    }

    /// How much capacity to ask for so that `more` items fit, or `TooLarge`.
    ///
    /// Doubles while the budget has room for it, so that appending stays linear, and
    /// never asks for a byte the budget does not have: nothing is allocated to find out
    /// that it was too much.
    fn growth(
        &self,
        len: usize,
        capacity: usize,
        more: usize,
        item: usize,
    ) -> Result<usize, GaveUp> {
        let allowed = self.max_bytes.saturating_sub(self.held) / item;
        let needed = len.checked_add(more).ok_or(GaveUp::TooLarge)?;
        let missing = needed.saturating_sub(capacity);
        if missing > allowed {
            return Err(GaveUp::TooLarge);
        }
        let extra = missing.max(capacity).max(16).min(allowed).max(missing);
        Ok(capacity - len + extra)
    }

    /// Makes room for `more` items in `items`.
    fn room<T>(&mut self, items: &mut Vec<T>, more: usize) -> Result<(), GaveUp> {
        if items.capacity() - items.len() >= more {
            return Ok(());
        }
        let item = size_of::<T>().max(1);
        let before = items.capacity();
        let additional = self.growth(items.len(), before, more, item)?;
        items
            .try_reserve_exact(additional)
            .map_err(|_| GaveUp::TooLarge)?;
        self.held += (items.capacity() - before) * item;
        Ok(())
    }

    /// Makes room for `more` bytes in `text`.
    fn room_in(&mut self, text: &mut String, more: usize) -> Result<(), GaveUp> {
        if text.capacity() - text.len() >= more {
            return Ok(());
        }
        let before = text.capacity();
        let additional = self.growth(text.len(), before, more, 1)?;
        text.try_reserve_exact(additional)
            .map_err(|_| GaveUp::TooLarge)?;
        self.held += text.capacity() - before;
        Ok(())
    }

    fn push<T>(&mut self, items: &mut Vec<T>, item: T) -> Result<(), GaveUp> {
        self.room(items, 1)?;
        items.push(item);
        Ok(())
    }

    fn append(&mut self, text: &mut String, more: &str) -> Result<(), GaveUp> {
        self.room_in(text, more.len())?;
        text.push_str(more);
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------
// Element names
// ---------------------------------------------------------------------------------------

/// An element name as a number: the ones below first, then the others in the order the
/// document introduces them.
type Name = u32;

macro_rules! names {
    ($($constant:ident $text:literal)*) => {
        /// The names htmlparser2 or a formatter treats in a way of their own.
        mod name {
            #[allow(non_camel_case_types, clippy::upper_case_acronyms)]
            #[repr(u32)]
            enum Index { $($constant,)* COUNT }
            $(pub const $constant: super::Name = Index::$constant as super::Name;)*
            pub const COUNT: super::Name = Index::COUNT as super::Name;

            pub fn known(text: &str) -> Option<super::Name> {
                match text {
                    $($text => Some($constant),)*
                    _ => None,
                }
            }
        }
    };
}

names! {
    A "a" ADDRESS "address" ANNOTATION_XML "annotation-xml" AREA "area" ARTICLE "article"
    ASIDE "aside" BASE "base" BASEFONT "basefont" BLOCKQUOTE "blockquote" BODY "body"
    BR "br" BUTTON "button" COL "col" COMMAND "command" CONSTRUCTOR "constructor"
    DATALIST "datalist" DD "dd" DESC "desc" DETAILS "details" DIV "div" DL "dl" DT "dt"
    EMBED "embed" FIELDSET "fieldset" FIGCAPTION "figcaption" FIGURE "figure"
    FOOTER "footer" FOREIGNOBJECT "foreignobject" FORM "form" FRAME "frame" H1 "h1"
    H2 "h2" H3 "h3" H4 "h4" H5 "h5" H6 "h6" HEAD "head" HEADER "header" HR "hr"
    IMG "img" INPUT "input" ISINDEX "isindex" KEYGEN "keygen" LI "li" LINK "link"
    MAIN "main" MATH "math" META "meta" MI "mi" MN "mn" MO "mo" MS "ms" MTEXT "mtext"
    NAV "nav" OL "ol" OPTGROUP "optgroup" OPTION "option" OUTPUT "output" P "p"
    PARAM "param" PRE "pre" RP "rp" RT "rt" SCRIPT "script" SECTION "section"
    SELECT "select" SOURCE "source" STYLE "style" SVG "svg" TABLE "table" TBODY "tbody"
    TD "td" TEXTAREA "textarea" TFOOT "tfoot" TH "th" THEAD "thead" TITLE "title"
    TR "tr" TRACK "track" UL "ul" WBR "wbr"
}

/// `voidElements` of htmlparser2's `Parser.js`.
fn is_void(name: Name) -> bool {
    use name::*;
    matches!(
        name,
        AREA | BASE
            | BASEFONT
            | BR
            | COL
            | COMMAND
            | EMBED
            | FRAME
            | HR
            | IMG
            | INPUT
            | ISINDEX
            | KEYGEN
            | LINK
            | META
            | PARAM
            | SOURCE
            | TRACK
            | WBR
    )
}

/// `foreignContextElements` of `Parser.js`.
fn is_foreign(name: Name) -> bool {
    matches!(name, name::MATH | name::SVG)
}

/// `htmlIntegrationElements` of `Parser.js`.
fn is_integration(name: Name) -> bool {
    use name::*;
    matches!(
        name,
        MI | MO | MN | MS | MTEXT | ANNOTATION_XML | FOREIGNOBJECT | DESC | TITLE
    )
}

/// `openImpliesClose` of `Parser.js`: whether the start tag `opening` closes `open`, the
/// innermost open element.
fn implies_close(opening: Name, open: Name) -> bool {
    use name::*;
    match opening {
        TR => matches!(open, TR | TH | TD),
        TH => open == TH,
        TD => matches!(open, THEAD | TH | TD),
        BODY => matches!(open, HEAD | LINK | SCRIPT),
        LI => open == LI,
        P | H1 | H2 | H3 | H4 | H5 | H6 | ADDRESS | ARTICLE | ASIDE | BLOCKQUOTE | DETAILS
        | DIV | DL | FIELDSET | FIGCAPTION | FIGURE | FOOTER | FORM | HEADER | HR | MAIN | NAV
        | OL | PRE | SECTION | TABLE | UL => open == P,
        SELECT | INPUT | OUTPUT | BUTTON | DATALIST | TEXTAREA => matches!(
            open,
            INPUT | OPTION | OPTGROUP | SELECT | BUTTON | DATALIST | TEXTAREA
        ),
        OPTION => open == OPTION,
        OPTGROUP => matches!(open, OPTGROUP | OPTION),
        DD | DT => matches!(open, DD | DT),
        RT | RP => matches!(open, RT | RP),
        TBODY | TFOOT => matches!(open, THEAD | TBODY),
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------
// The tree
// ---------------------------------------------------------------------------------------

/// No node: the end of a list of siblings, or an element without children.
const NONE: u32 = u32::MAX;
/// The `kind` of a text node.
const TEXT: u32 = u32::MAX - 1;
/// The `kind` of what the walk steps over but a list still counts as an item: a comment,
/// a doctype or processing instruction, and a `script` or `style` element, which
/// domhandler gives a type of its own that `recursiveWalk` does not handle.
const OTHER: u32 = u32::MAX - 2;

/// A node of domhandler's tree, as little of it as the conversion reads.
#[derive(Clone, Copy)]
struct Node {
    /// The [`Name`] of an element, [`TEXT`] or [`OTHER`].
    kind: u32,
    /// The next sibling.
    next: u32,
    /// The first child of an element; where the data of a text node starts in `Dom::text`.
    first: u32,
    /// For `a` and `ol`, the place of their attributes in `Dom::values`, plus one; the
    /// length of the data of a text node.
    extra: u32,
}

/// The value of an attribute, in `Dom::attributes`.
#[derive(Clone, Copy)]
struct Value {
    at: u32,
    /// [`NONE`] when the element has no such attribute.
    len: u32,
}

const NO_VALUE: Value = Value { at: 0, len: NONE };

/// What `parseDocument` returns, and what `findBases` finds in it.
struct Dom {
    nodes: Vec<Node>,
    /// The data of every text node, entities decoded.
    text: String,
    /// The values of the attributes a formatter reads: `href` of `a`, `start` and `type`
    /// of `ol`.
    attributes: String,
    /// One value for an `a`, two for an `ol`.
    values: Vec<Value>,
    /// The first child of the document.
    first: u32,
    /// The `body` elements `findBases` would return.
    bases: Vec<u32>,
}

impl Dom {
    fn node(&self, id: u32) -> Option<Node> {
        self.nodes.get(id as usize).copied()
    }

    fn next(&self, id: u32) -> u32 {
        self.node(id).map_or(NONE, |node| node.next)
    }

    /// The data of a text node.
    fn data(&self, node: Node) -> &str {
        let start = node.first as usize;
        self.text
            .get(start..start + node.extra as usize)
            .unwrap_or_default()
    }

    /// The `index`th attribute kept for an element, when it has it.
    fn value(&self, node: Node, index: usize) -> Option<&str> {
        let slot = (node.extra as usize).checked_sub(1)?;
        let value = self.values.get(slot + index)?;
        if value.len == NONE {
            return None;
        }
        let start = value.at as usize;
        self.attributes.get(start..start + value.len as usize)
    }
}

/// A place in a buffer as the tree stores it.
fn place(at: usize) -> Result<u32, GaveUp> {
    match u32::try_from(at) {
        Ok(at) if at < OTHER => Ok(at),
        _ => Err(GaveUp::TooLarge),
    }
}

// ---------------------------------------------------------------------------------------
// htmlparser2: Tokenizer.js, Parser.js, and the DomHandler of domhandler
// ---------------------------------------------------------------------------------------

/// The states of `Tokenizer.js`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Text,
    BeforeTagName,
    InTagName,
    InSelfClosingTag,
    BeforeClosingTagName,
    InClosingTagName,
    AfterClosingTagName,
    BeforeAttributeName,
    InAttributeName,
    AfterAttributeName,
    BeforeAttributeValue,
    InAttributeValueDq,
    InAttributeValueSq,
    InAttributeValueNq,
    BeforeDeclaration,
    InDeclaration,
    InProcessingInstruction,
    BeforeComment,
    CdataSequence,
    InSpecialComment,
    InCommentLike,
    BeforeSpecialS,
    BeforeSpecialT,
    SpecialStartSequence,
    InSpecialTag,
    InEntity,
}

/// The `...End` sequences of `Sequences` in `Tokenizer.js`: what ends a CDATA section, a
/// comment, and the content of the elements whose content is not markup.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sequence {
    Cdata,
    Comment,
    Script,
    Style,
    Title,
    Textarea,
    Xmp,
}

impl Sequence {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Cdata => b"]]>",
            Self::Comment => b"-->",
            Self::Script => b"</script",
            Self::Style => b"</style",
            Self::Title => b"</title",
            Self::Textarea => b"</textarea",
            Self::Xmp => b"</xmp",
        }
    }
}

/// `Sequences.Cdata`: what follows `<![` where a CDATA section starts.
const CDATA: &[u8] = b"CDATA[";

/// The tokenizer's `sectionStart` is -1 while it is between two sections.
const NO_SECTION: usize = usize::MAX;

fn is_whitespace(c: u8) -> bool {
    matches!(c, b' ' | b'\n' | b'\t' | 0x0c | b'\r')
}

fn is_end_of_tag_section(c: u8) -> bool {
    c == b'/' || c == b'>' || is_whitespace(c)
}

/// An attribute a formatter reads.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kept {
    Href,
    Start,
    Type,
}

/// An open element: an entry of the `stack` of `Parser.js` and of the `tagStack` of
/// `DomHandler`, which only differ while a start tag is being read.
struct Open {
    name: Name,
    /// The last child added, to link the next one after it.
    last_child: u32,
    /// The element in the tree, [`NONE`] when it is inside a `script` or `style` element.
    node: u32,
}

/// `Tokenizer`, `Parser` and `DomHandler` in one: the tokenizer calls the parser, which
/// calls the handler, and nothing else listens.
///
/// It reads bytes where the JavaScript reads UTF-16 code units. Every character the
/// tokenizer or the entity decoder looks for is ASCII, and neither a byte of a longer
/// UTF-8 sequence nor a code unit above 127 is taken for one, so the sections are cut
/// at the same places.
struct Parser<'a, 'm> {
    source: &'a str,
    bytes: &'a [u8],
    meter: &'m mut Meter,

    // Tokenizer.js
    state: State,
    base_state: State,
    index: usize,
    section_start: usize,
    entity_start: usize,
    is_special: bool,
    sequence: Sequence,
    sequence_index: usize,
    entity: EntityDecoder,
    /// How many characters the entity decoder may still read: see `entity_step`.
    entity_steps_left: usize,
    /// The characters of the named entity being emitted: two at most.
    decoded: String,

    // Parser.js
    /// `tagname`: the start tag being read.
    tag: Name,
    /// `foreignContext`, innermost last.
    foreign: Vec<bool>,
    /// How many elements of each name are open, so that an end tag that closes nothing
    /// is known as such without searching the whole stack.
    open_count: Vec<u32>,
    /// The names HTML does not define, as the document spells them in lower case.
    names: HashMap<Box<str>, Name>,
    /// The bytes `names` was counted for.
    names_held: usize,
    lowered: String,
    /// `attribname`, when it is one a formatter reads and the tag does not have it yet:
    /// the first of two attributes of the same name wins.
    attribute: Option<Kept>,
    /// Where `attribvalue` starts in `dom.attributes`.
    attribute_start: usize,
    /// `attribs`, as much of it as is kept.
    href: Value,
    start: Value,
    r#type: Value,

    // DomHandler
    dom: Dom,
    /// `tagStack`, with the document first.
    open: Vec<Open>,
    /// `lastNode` when it is a text node: the one more text is added to.
    last_text: u32,
    /// How many `script` and `style` elements are open. Neither `findBases` nor the walk
    /// enters them, so what is inside is not kept.
    hidden: usize,
    /// How many `body` elements are open: `findBases` does not look inside one.
    bodies: usize,
}

impl<'a, 'm> Parser<'a, 'm> {
    fn new(source: &'a str, meter: &'m mut Meter) -> Self {
        Self {
            source,
            bytes: source.as_bytes(),
            meter,
            state: State::Text,
            base_state: State::Text,
            index: 0,
            section_start: 0,
            entity_start: 0,
            is_special: false,
            sequence: Sequence::Comment,
            sequence_index: 0,
            entity: EntityDecoder::new(DecodingMode::Legacy),
            entity_steps_left: source.len().saturating_mul(8).saturating_add(1024),
            decoded: String::new(),
            tag: NONE,
            foreign: Vec::new(),
            open_count: Vec::new(),
            names: HashMap::new(),
            names_held: 0,
            lowered: String::new(),
            attribute: None,
            attribute_start: 0,
            href: NO_VALUE,
            start: NO_VALUE,
            r#type: NO_VALUE,
            dom: Dom {
                nodes: Vec::new(),
                text: String::new(),
                attributes: String::new(),
                values: Vec::new(),
                first: NONE,
                bases: Vec::new(),
            },
            open: Vec::new(),
            last_text: NONE,
            hidden: 0,
            bodies: 0,
        }
    }

    /// `parseDocument(html, { decodeEntities: true })`.
    fn parse(mut self) -> Result<Dom, GaveUp> {
        self.meter
            .room(&mut self.open_count, name::COUNT as usize)?;
        self.open_count.resize(name::COUNT as usize, 0);
        let document = Open {
            name: NONE,
            last_child: NONE,
            node: NONE,
        };
        self.meter.push(&mut self.open, document)?;
        // `[!this.htmlMode]`
        self.meter.push(&mut self.foreign, false)?;

        // `Tokenizer.parse`.
        while let Some(&c) = self.bytes.get(self.index) {
            self.meter.spend(1)?;
            match self.state {
                State::Text => self.state_text(c)?,
                State::SpecialStartSequence => self.state_special_start_sequence(c)?,
                State::InSpecialTag => self.state_in_special_tag(c)?,
                State::CdataSequence => self.state_cdata_sequence(c)?,
                State::InAttributeValueDq => self.handle_in_attribute_value(c, b'"')?,
                State::InAttributeName => self.state_in_attribute_name(c)?,
                State::InCommentLike => self.state_in_comment_like(c)?,
                State::InSpecialComment => self.state_in_special_comment(c)?,
                State::BeforeAttributeName => self.state_before_attribute_name(c)?,
                State::InTagName => self.state_in_tag_name(c)?,
                State::InClosingTagName => self.state_in_closing_tag_name(c)?,
                State::BeforeTagName => self.state_before_tag_name(c)?,
                State::AfterAttributeName => self.state_after_attribute_name(c)?,
                State::InAttributeValueSq => self.handle_in_attribute_value(c, b'\'')?,
                State::BeforeAttributeValue => self.state_before_attribute_value(c)?,
                State::BeforeClosingTagName => self.state_before_closing_tag_name(c),
                State::AfterClosingTagName => self.state_after_closing_tag_name(c)?,
                State::BeforeSpecialS => self.state_before_special_s(c)?,
                State::BeforeSpecialT => self.state_before_special_t(c)?,
                State::InAttributeValueNq => self.state_in_attribute_value_no_quotes(c)?,
                State::InSelfClosingTag => self.state_in_self_closing_tag(c)?,
                State::InDeclaration => self.state_in_declaration(c)?,
                State::BeforeDeclaration => self.state_before_declaration(c),
                State::BeforeComment => self.state_before_comment(c),
                State::InProcessingInstruction => self.state_in_declaration(c)?,
                State::InEntity => self.state_in_entity()?,
            }
            self.index += 1;
        }
        self.cleanup()?;
        self.finish()?;
        // Only the tree outlives the parser.
        self.meter.release(
            self.names_held
                + self.open.capacity() * size_of::<Open>()
                + self.open_count.capacity() * size_of::<u32>()
                + self.foreign.capacity()
                + self.lowered.capacity(),
        );
        Ok(self.dom)
    }

    // --- Tokenizer.js ---

    fn state_text(&mut self, c: u8) -> Result<(), GaveUp> {
        if c == b'<' {
            if self.index > self.section_start {
                self.on_text(self.section_start, self.index)?;
            }
            self.state = State::BeforeTagName;
            self.section_start = self.index;
        } else if c == b'&' {
            self.start_entity();
        } else {
            // No other character does anything here: go to the next one that does.
            let rest = self.bytes.get(self.index + 1..).unwrap_or_default();
            let plain = rest
                .iter()
                .position(|&next| next == b'<' || next == b'&')
                .unwrap_or(rest.len());
            self.meter.spend(plain)?;
            self.index += plain;
        }
        Ok(())
    }

    fn state_special_start_sequence(&mut self, c: u8) -> Result<(), GaveUp> {
        let sequence = self.sequence.bytes();
        let is_end = self.sequence_index == sequence.len();
        let is_match = if is_end {
            // At the end of the sequence, the tag name must have ended as well.
            is_end_of_tag_section(c)
        } else {
            sequence.get(self.sequence_index) == Some(&(c | 0x20))
        };
        if !is_match {
            self.is_special = false;
        } else if !is_end {
            self.sequence_index += 1;
            return Ok(());
        }
        self.sequence_index = 0;
        self.state = State::InTagName;
        self.state_in_tag_name(c)
    }

    /// Looks for the end tag of `script`, `style`, `title`, `textarea` or `xmp`.
    fn state_in_special_tag(&mut self, c: u8) -> Result<(), GaveUp> {
        let sequence = self.sequence.bytes();
        if self.sequence_index == sequence.len() {
            if c == b'>' || is_whitespace(c) {
                let end_of_text = self.index.saturating_sub(sequence.len());
                if self.section_start < end_of_text {
                    self.on_text(self.section_start, end_of_text)?;
                }
                self.is_special = false;
                // Skip over the `</`.
                self.section_start = end_of_text + 2;
                return self.state_in_closing_tag_name(c);
            }
            self.sequence_index = 0;
        }
        if sequence.get(self.sequence_index) == Some(&(c | 0x20)) {
            self.sequence_index += 1;
        } else if self.sequence_index == 0 {
            if self.sequence == Sequence::Title {
                // Entities are decoded in `title`.
                if c == b'&' {
                    self.start_entity();
                }
            } else if self.fast_forward_to(b'<')? {
                self.sequence_index = 1;
            }
        } else {
            // A `<` may start the sequence again, as in `<</script>`.
            self.sequence_index = usize::from(c == b'<');
        }
        Ok(())
    }

    fn state_cdata_sequence(&mut self, c: u8) -> Result<(), GaveUp> {
        if CDATA.get(self.sequence_index) == Some(&c) {
            self.sequence_index += 1;
            if self.sequence_index == CDATA.len() {
                self.state = State::InCommentLike;
                self.sequence = Sequence::Cdata;
                self.sequence_index = 0;
                self.section_start = self.index + 1;
            }
            Ok(())
        } else {
            self.sequence_index = 0;
            self.state = State::InDeclaration;
            self.state_in_declaration(c)
        }
    }

    /// Moves to the next `c` after the current character, or to the last character.
    fn fast_forward_to(&mut self, c: u8) -> Result<bool, GaveUp> {
        let rest = self.bytes.get(self.index + 1..).unwrap_or_default();
        match rest.iter().position(|&next| next == c) {
            Some(at) => {
                self.meter.spend(at)?;
                self.index += at + 1;
                Ok(true)
            }
            None => {
                self.meter.spend(rest.len())?;
                // The loop moves on by one after each state.
                self.index = self.bytes.len().saturating_sub(1);
                Ok(false)
            }
        }
    }

    /// Comments end with `-->` and CDATA sections with `]]>`.
    fn state_in_comment_like(&mut self, c: u8) -> Result<(), GaveUp> {
        let sequence = self.sequence.bytes();
        if sequence.get(self.sequence_index) == Some(&c) {
            self.sequence_index += 1;
            if self.sequence_index == sequence.len() {
                // `oncdata` reports a comment too: the parser is not in XML mode.
                self.on_comment()?;
                self.sequence_index = 0;
                self.section_start = self.index + 1;
                self.state = State::Text;
            }
        } else if self.sequence_index == 0 {
            if let Some(&first) = sequence.first()
                && self.fast_forward_to(first)?
            {
                self.sequence_index = 1;
            }
        } else if sequence.get(self.sequence_index - 1) != Some(&c) {
            // Longer runs, as in `--->` and `]]]>`, still end the section.
            self.sequence_index = 0;
        }
        Ok(())
    }

    fn start_special(&mut self, sequence: Sequence, offset: usize) {
        self.is_special = true;
        self.sequence = sequence;
        self.sequence_index = offset;
        self.state = State::SpecialStartSequence;
    }

    fn state_before_tag_name(&mut self, c: u8) -> Result<(), GaveUp> {
        if c == b'!' {
            self.state = State::BeforeDeclaration;
            self.section_start = self.index + 1;
        } else if c == b'?' {
            self.state = State::InProcessingInstruction;
            self.section_start = self.index + 1;
        } else if c.is_ascii_alphabetic() {
            let lower = c | 0x20;
            self.section_start = self.index;
            self.state = match lower {
                b's' => State::BeforeSpecialS,
                // `x` as well: a name such as `tmp` is then taken for `xmp`, as upstream.
                b't' | b'x' => State::BeforeSpecialT,
                _ => State::InTagName,
            };
        } else if c == b'/' {
            self.state = State::BeforeClosingTagName;
        } else {
            self.state = State::Text;
            self.state_text(c)?;
        }
        Ok(())
    }

    fn state_in_tag_name(&mut self, c: u8) -> Result<(), GaveUp> {
        if is_end_of_tag_section(c) {
            self.on_open_tag_name(self.section_start, self.index)?;
            self.section_start = NO_SECTION;
            self.state = State::BeforeAttributeName;
            self.state_before_attribute_name(c)?;
        }
        Ok(())
    }

    fn state_before_closing_tag_name(&mut self, c: u8) {
        if is_whitespace(c) {
            // Ignored.
        } else if c == b'>' {
            // `sectionStart` stays on the `<`: `</>` is text.
            self.state = State::Text;
        } else {
            self.state = if c.is_ascii_alphabetic() {
                State::InClosingTagName
            } else {
                State::InSpecialComment
            };
            self.section_start = self.index;
        }
    }

    fn state_in_closing_tag_name(&mut self, c: u8) -> Result<(), GaveUp> {
        if c == b'>' || is_whitespace(c) {
            self.on_close_tag(self.section_start, self.index)?;
            self.section_start = NO_SECTION;
            self.state = State::AfterClosingTagName;
            self.state_after_closing_tag_name(c)?;
        }
        Ok(())
    }

    fn state_after_closing_tag_name(&mut self, c: u8) -> Result<(), GaveUp> {
        // Everything up to the `>` is skipped.
        if c == b'>' || self.fast_forward_to(b'>')? {
            self.state = State::Text;
            self.section_start = self.index + 1;
        }
        Ok(())
    }

    fn state_before_attribute_name(&mut self, c: u8) -> Result<(), GaveUp> {
        if c == b'>' {
            self.end_open_tag()?;
            if self.is_special {
                self.state = State::InSpecialTag;
                self.sequence_index = 0;
            } else {
                self.state = State::Text;
            }
            self.section_start = self.index + 1;
        } else if c == b'/' {
            self.state = State::InSelfClosingTag;
        } else if !is_whitespace(c) {
            self.state = State::InAttributeName;
            self.section_start = self.index;
        }
        Ok(())
    }

    fn state_in_self_closing_tag(&mut self, c: u8) -> Result<(), GaveUp> {
        if c == b'>' {
            self.on_self_closing_tag()?;
            self.state = State::Text;
            self.section_start = self.index + 1;
            // A self-closing `script` has no content to skip.
            self.is_special = false;
        } else if !is_whitespace(c) {
            self.state = State::BeforeAttributeName;
            self.state_before_attribute_name(c)?;
        }
        Ok(())
    }

    fn state_in_attribute_name(&mut self, c: u8) -> Result<(), GaveUp> {
        if c == b'=' || is_end_of_tag_section(c) {
            self.on_attrib_name(self.section_start, self.index);
            self.section_start = self.index;
            self.state = State::AfterAttributeName;
            self.state_after_attribute_name(c)?;
        }
        Ok(())
    }

    fn state_after_attribute_name(&mut self, c: u8) -> Result<(), GaveUp> {
        if c == b'=' {
            self.state = State::BeforeAttributeValue;
        } else if c == b'/' || c == b'>' {
            self.on_attrib_end()?;
            self.section_start = NO_SECTION;
            self.state = State::BeforeAttributeName;
            self.state_before_attribute_name(c)?;
        } else if !is_whitespace(c) {
            self.on_attrib_end()?;
            self.state = State::InAttributeName;
            self.section_start = self.index;
        }
        Ok(())
    }

    fn state_before_attribute_value(&mut self, c: u8) -> Result<(), GaveUp> {
        if c == b'"' {
            self.state = State::InAttributeValueDq;
            self.section_start = self.index + 1;
        } else if c == b'\'' {
            self.state = State::InAttributeValueSq;
            self.section_start = self.index + 1;
        } else if !is_whitespace(c) {
            self.section_start = self.index;
            self.state = State::InAttributeValueNq;
            self.state_in_attribute_value_no_quotes(c)?;
        }
        Ok(())
    }

    fn handle_in_attribute_value(&mut self, c: u8, quote: u8) -> Result<(), GaveUp> {
        if c == quote {
            self.on_attrib_data(self.section_start, self.index)?;
            self.section_start = NO_SECTION;
            self.on_attrib_end()?;
            self.state = State::BeforeAttributeName;
        } else if c == b'&' {
            self.start_entity();
        }
        Ok(())
    }

    fn state_in_attribute_value_no_quotes(&mut self, c: u8) -> Result<(), GaveUp> {
        if is_whitespace(c) || c == b'>' {
            self.on_attrib_data(self.section_start, self.index)?;
            self.section_start = NO_SECTION;
            self.on_attrib_end()?;
            self.state = State::BeforeAttributeName;
            self.state_before_attribute_name(c)?;
        } else if c == b'&' {
            self.start_entity();
        }
        Ok(())
    }

    fn state_before_declaration(&mut self, c: u8) {
        if c == b'[' {
            self.state = State::CdataSequence;
            self.sequence_index = 0;
        } else {
            self.state = if c == b'-' {
                State::BeforeComment
            } else {
                State::InDeclaration
            };
        }
    }

    /// `stateInDeclaration` and `stateInProcessingInstruction`: a doctype and a processing
    /// instruction both end at the next `>` and both become a node the walk steps over.
    fn state_in_declaration(&mut self, c: u8) -> Result<(), GaveUp> {
        if c == b'>' || self.fast_forward_to(b'>')? {
            self.on_processing_instruction()?;
            self.state = State::Text;
            self.section_start = self.index + 1;
        }
        Ok(())
    }

    fn state_before_comment(&mut self, c: u8) {
        if c == b'-' {
            self.state = State::InCommentLike;
            self.sequence = Sequence::Comment;
            // Short comments, as in `<!-->`, end here.
            self.sequence_index = 2;
            self.section_start = self.index + 1;
        } else {
            self.state = State::InDeclaration;
        }
    }

    fn state_in_special_comment(&mut self, c: u8) -> Result<(), GaveUp> {
        if c == b'>' || self.fast_forward_to(b'>')? {
            self.on_comment()?;
            self.state = State::Text;
            self.section_start = self.index + 1;
        }
        Ok(())
    }

    fn state_before_special_s(&mut self, c: u8) -> Result<(), GaveUp> {
        match c | 0x20 {
            b'c' => self.start_special(Sequence::Script, 4),
            b't' => self.start_special(Sequence::Style, 4),
            _ => {
                self.state = State::InTagName;
                self.state_in_tag_name(c)?;
            }
        }
        Ok(())
    }

    fn state_before_special_t(&mut self, c: u8) -> Result<(), GaveUp> {
        match c | 0x20 {
            b'i' => self.start_special(Sequence::Title, 4),
            b'e' => self.start_special(Sequence::Textarea, 4),
            b'm' => self.start_special(Sequence::Xmp, 4),
            _ => {
                self.state = State::InTagName;
                self.state_in_tag_name(c)?;
            }
        }
        Ok(())
    }

    fn start_entity(&mut self) {
        self.base_state = self.state;
        self.state = State::InEntity;
        self.entity_start = self.index;
        let in_text = matches!(self.base_state, State::Text | State::InSpecialTag);
        self.entity = EntityDecoder::new(if in_text {
            DecodingMode::Legacy
        } else {
            DecodingMode::Attribute
        });
    }

    fn state_in_entity(&mut self) -> Result<(), GaveUp> {
        match self.entity_write(self.index)? {
            Some(length) => {
                self.state = self.base_state;
                if length == 0 {
                    // Not an entity: the character after the `&` is read again as text.
                    self.index -= 1;
                }
            }
            // The document ends inside the entity: `finish` asks the decoder what it has.
            // (Upstream also looks for a second `&` here, which only a document fed in
            // several chunks can have.)
            None => self.index = self.bytes.len().saturating_sub(1),
        }
        Ok(())
    }

    /// What `parse` does once the chunk, here the whole document, is read.
    fn cleanup(&mut self) -> Result<(), GaveUp> {
        if self.section_start == self.index {
            return Ok(());
        }
        let in_text = self.state == State::Text
            || (self.state == State::InSpecialTag && self.sequence_index == 0);
        if in_text {
            self.on_text(self.section_start, self.index)?;
            self.section_start = self.index;
        } else if matches!(
            self.state,
            State::InAttributeValueDq | State::InAttributeValueSq | State::InAttributeValueNq
        ) {
            self.on_attrib_data(self.section_start, self.index)?;
            self.section_start = self.index;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<(), GaveUp> {
        if self.state == State::InEntity {
            self.entity_end()?;
            self.state = self.base_state;
        }
        self.handle_trailing_data()
        // `onend` then closes every open element, which adds nothing to the tree.
    }

    fn handle_trailing_data(&mut self) -> Result<(), GaveUp> {
        let end = self.bytes.len();
        if self.section_start != NO_SECTION && self.section_start >= end {
            return Ok(());
        }
        match self.state {
            State::InCommentLike => self.on_comment(),
            // A tag the document ends in is ignored.
            State::InTagName
            | State::BeforeAttributeName
            | State::BeforeAttributeValue
            | State::AfterAttributeName
            | State::InAttributeName
            | State::InAttributeValueSq
            | State::InAttributeValueDq
            | State::InAttributeValueNq
            | State::InClosingTagName => Ok(()),
            // After `<name /` or `</name `, `sectionStart` is -1 and upstream takes
            // `buffer.slice(-1, end)` for text: the last code unit of the document.
            _ if self.section_start == NO_SECTION => {
                let source = self.source;
                match source.char_indices().next_back() {
                    Some((at, last)) if last.len_utf16() == 1 => {
                        self.on_text_data(source.get(at..).unwrap_or_default())
                    }
                    // The second half of a surrogate pair, which leaves Node as U+FFFD.
                    Some(_) => self.on_text_data(REPLACEMENT),
                    None => Ok(()),
                }
            }
            _ => self.on_text(self.section_start, end),
        }
    }

    /// `emitCodePoint`, for all the code points of one entity at once.
    fn emit_decoded(&mut self, decoded: &str, consumed: usize) -> Result<(), GaveUp> {
        let in_text = matches!(self.base_state, State::Text | State::InSpecialTag);
        if self.section_start < self.entity_start {
            if in_text {
                self.on_text(self.section_start, self.entity_start)?;
            } else {
                self.on_attrib_data(self.section_start, self.entity_start)?;
            }
        }
        self.section_start = self.entity_start + consumed;
        self.index = self.section_start - 1;
        if in_text {
            self.on_text_data(decoded)
        } else {
            self.on_attrib_text(decoded)
        }
    }

    // --- Parser.js ---

    /// `name.toLowerCase()`, then the number of the name. `None` for a name the document
    /// has not used when `add` is false: an end tag does not introduce one.
    fn name(&mut self, start: usize, end: usize, add: bool) -> Result<Option<Name>, GaveUp> {
        let text = self.source.get(start..end).unwrap_or_default();
        let mut lowered = std::mem::take(&mut self.lowered);
        let text = if !text.is_ascii() {
            // The final sigma has a lower case of its own, as in JavaScript.
            let lower = text.to_lowercase();
            lowered.clear();
            self.meter.append(&mut lowered, &lower)?;
            lowered.as_str()
        } else if text.bytes().any(|c| c.is_ascii_uppercase()) {
            lowered.clear();
            self.meter.append(&mut lowered, text)?;
            lowered.make_ascii_lowercase();
            lowered.as_str()
        } else {
            text
        };
        let name = self.name_of(text, add);
        self.lowered = lowered;
        name
    }

    fn name_of(&mut self, text: &str, add: bool) -> Result<Option<Name>, GaveUp> {
        if let Some(name) = name::known(text) {
            return Ok(Some(name));
        }
        if let Some(&name) = self.names.get(text) {
            return Ok(Some(name));
        }
        if !add {
            return Ok(None);
        }
        let name = place(self.open_count.len())?;
        // The text of the name and, roughly, its entry in the map.
        let held = text.len() + 4 * size_of::<usize>();
        self.meter.hold(held)?;
        self.names_held += held;
        self.meter.push(&mut self.open_count, 0)?;
        self.names.insert(text.into(), name);
        Ok(Some(name))
    }

    fn on_open_tag_name(&mut self, start: usize, end: usize) -> Result<(), GaveUp> {
        if let Some(name) = self.name(start, end, true)? {
            self.emit_open_tag(name)?;
        }
        Ok(())
    }

    /// `emitOpenTag`, except that the element joins the stack in `end_open_tag`, when it
    /// joins the tree. Nothing reads the stack in between.
    fn emit_open_tag(&mut self, name: Name) -> Result<(), GaveUp> {
        self.tag = name;
        while let Some(open) = self.open.last()
            && self.open.len() > 1
            && implies_close(name, open.name)
        {
            self.close_innermost();
        }
        if !is_void(name) {
            if is_foreign(name) {
                self.meter.push(&mut self.foreign, true)?;
            } else if is_integration(name) {
                self.meter.push(&mut self.foreign, false)?;
            }
        }
        // `this.attribs = {}`
        self.attribute = None;
        self.href = NO_VALUE;
        self.start = NO_VALUE;
        self.r#type = NO_VALUE;
        Ok(())
    }

    /// `endOpenTag`, and `onopentag` of the handler.
    fn end_open_tag(&mut self) -> Result<(), GaveUp> {
        let name = self.tag;
        let is_hidden = matches!(name, name::SCRIPT | name::STYLE);
        let mut extra = 0;
        if self.hidden == 0 {
            let kept: &[Value] = match name {
                name::A if self.href.len != NONE => &[self.href],
                name::OL if self.start.len != NONE || self.r#type.len != NONE => {
                    &[self.start, self.r#type]
                }
                _ => &[],
            };
            if !kept.is_empty() {
                extra = place(self.dom.values.len())? + 1;
                self.meter.room(&mut self.dom.values, kept.len())?;
                self.dom.values.extend_from_slice(kept);
            }
        }
        let kind = if is_hidden { OTHER } else { name };
        let node = self.add_node(Node {
            kind,
            next: NONE,
            first: NONE,
            extra,
        })?;
        if name == name::BODY && node != NONE && self.bodies == 0 {
            // `findBases`: a `body` that is not inside another one.
            self.meter.push(&mut self.dom.bases, node)?;
        }
        if is_void(name) {
            // `onclosetag` right away, which `add_node` already accounted for.
            return Ok(());
        }
        self.meter.push(
            &mut self.open,
            Open {
                name,
                last_child: NONE,
                node,
            },
        )?;
        if let Some(count) = self.open_count.get_mut(name as usize) {
            *count += 1;
        }
        self.hidden += usize::from(is_hidden);
        self.bodies += usize::from(name == name::BODY);
        Ok(())
    }

    /// `onclosetag` of the handler, for the innermost open element.
    fn close_innermost(&mut self) -> Option<Name> {
        if self.open.len() <= 1 {
            return None;
        }
        let closed = self.open.pop()?;
        if let Some(count) = self.open_count.get_mut(closed.name as usize) {
            *count = count.saturating_sub(1);
        }
        if matches!(closed.name, name::SCRIPT | name::STYLE) {
            self.hidden = self.hidden.saturating_sub(1);
        }
        if closed.name == name::BODY {
            self.bodies = self.bodies.saturating_sub(1);
        }
        self.last_text = NONE;
        Some(closed.name)
    }

    fn on_close_tag(&mut self, start: usize, end: usize) -> Result<(), GaveUp> {
        // A name no start tag has used closes nothing and is none of the names below.
        let Some(name) = self.name(start, end, false)? else {
            return Ok(());
        };
        if is_foreign(name) || is_integration(name) {
            self.foreign.pop();
        }
        if !is_void(name) {
            let is_open = self
                .open_count
                .get(name as usize)
                .is_some_and(|&count| count > 0);
            if is_open {
                // Upstream searches the stack for the name: a search that fails costs the
                // depth of the document, which the count above spares.
                while let Some(closed) = self.close_innermost() {
                    if closed == name {
                        break;
                    }
                }
            } else if name == name::P {
                // `</p>` without a `<p>` is an empty paragraph.
                self.emit_open_tag(name::P)?;
                self.close_current_tag()?;
            }
        } else if name == name::BR {
            // `</br>` is a line break.
            self.add_node(Node {
                kind: name::BR,
                next: NONE,
                first: NONE,
                extra: 0,
            })?;
        }
        Ok(())
    }

    fn on_self_closing_tag(&mut self) -> Result<(), GaveUp> {
        // `recognizeSelfClosing` is off in HTML: `/>` only closes inside `svg` and `math`.
        if self.foreign.last().copied().unwrap_or(false) {
            self.close_current_tag()
        } else {
            self.end_open_tag()
        }
    }

    fn close_current_tag(&mut self) -> Result<(), GaveUp> {
        let name = self.tag;
        self.end_open_tag()?;
        if self.open.len() > 1 && self.open.last().is_some_and(|open| open.name == name) {
            self.close_innermost();
        }
        Ok(())
    }

    fn on_attrib_name(&mut self, start: usize, end: usize) {
        self.attribute_start = self.dom.attributes.len();
        self.attribute = None;
        if self.hidden > 0 {
            return;
        }
        let text = self.source.get(start..end).unwrap_or_default();
        // No character lowers to one of these letters but its ASCII capital.
        let is = |name: &str| text.eq_ignore_ascii_case(name);
        self.attribute = match self.tag {
            name::A if is("href") && self.href.len == NONE => Some(Kept::Href),
            name::OL if is("start") && self.start.len == NONE => Some(Kept::Start),
            name::OL if is("type") && self.r#type.len == NONE => Some(Kept::Type),
            _ => None,
        };
    }

    fn on_attrib_data(&mut self, start: usize, end: usize) -> Result<(), GaveUp> {
        let source = self.source;
        self.on_attrib_text(source.get(start..end).unwrap_or_default())
    }

    fn on_attrib_text(&mut self, text: &str) -> Result<(), GaveUp> {
        if self.attribute.is_some() {
            self.meter.append(&mut self.dom.attributes, text)?;
        }
        Ok(())
    }

    fn on_attrib_end(&mut self) -> Result<(), GaveUp> {
        let Some(attribute) = self.attribute.take() else {
            return Ok(());
        };
        let value = Value {
            at: place(self.attribute_start)?,
            len: place(self.dom.attributes.len() - self.attribute_start)?,
        };
        match attribute {
            Kept::Href => self.href = value,
            Kept::Start => self.start = value,
            Kept::Type => self.r#type = value,
        }
        Ok(())
    }

    // --- DomHandler ---

    fn on_text(&mut self, start: usize, end: usize) -> Result<(), GaveUp> {
        let source = self.source;
        self.on_text_data(source.get(start..end).unwrap_or_default())
    }

    /// `ontext`: more data for the text node being written, or a new one.
    fn on_text_data(&mut self, data: &str) -> Result<(), GaveUp> {
        if self.hidden > 0 || data.is_empty() {
            return Ok(());
        }
        let at = self.dom.text.len();
        let len = place(data.len())?;
        self.meter.append(&mut self.dom.text, data)?;
        if let Some(node) = self.dom.nodes.get_mut(self.last_text as usize) {
            node.extra = place(node.extra as usize + data.len())?;
            return Ok(());
        }
        let node = Node {
            kind: TEXT,
            next: NONE,
            first: place(at)?,
            extra: len,
        };
        self.last_text = self.add_node(node)?;
        Ok(())
    }

    /// `oncomment` then `oncommentend`. A CDATA section is a comment too in HTML.
    fn on_comment(&mut self) -> Result<(), GaveUp> {
        self.add_node(Node {
            kind: OTHER,
            next: NONE,
            first: NONE,
            extra: 0,
        })?;
        Ok(())
    }

    /// `onprocessinginstruction`, for `<!...>` and `<?...>`.
    fn on_processing_instruction(&mut self) -> Result<(), GaveUp> {
        self.on_comment()
    }

    /// `addNode`: the node becomes the last child of the innermost open element, and no
    /// text node is being written any more.
    fn add_node(&mut self, node: Node) -> Result<u32, GaveUp> {
        self.last_text = NONE;
        if self.hidden > 0 {
            return Ok(NONE);
        }
        let id = place(self.dom.nodes.len())?;
        self.meter.push(&mut self.dom.nodes, node)?;
        let Some(parent) = self.open.last_mut() else {
            return Ok(id);
        };
        if let Some(previous) = self.dom.nodes.get_mut(parent.last_child as usize) {
            previous.next = id;
        } else if let Some(element) = self.dom.nodes.get_mut(parent.node as usize) {
            element.first = id;
        } else {
            self.dom.first = id;
        }
        parent.last_child = id;
        Ok(id)
    }
}

// ---------------------------------------------------------------------------------------
// entities: decode.js
// ---------------------------------------------------------------------------------------

/// `BinTrieFlags`: how a word of the trie is laid out.
mod flag {
    pub const VALUE_LENGTH: u16 = 0b1100_0000_0000_0000;
    pub const FLAG13: u16 = 0b0010_0000_0000_0000;
    pub const BRANCH_LENGTH: u16 = 0b0001_1111_1000_0000;
    pub const JUMP_TABLE: u16 = 0b0000_0000_0111_1111;
}

fn tree(index: usize) -> u16 {
    HTML_DECODE_TREE.get(index).copied().unwrap_or(0)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DecodingMode {
    /// In text: an entity may end with any character.
    Legacy,
    /// In an attribute value: `&name=` and `&nameX` are not entities.
    Attribute,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum EntityState {
    EntityStart,
    NumericStart,
    NumericDecimal,
    NumericHex,
    NamedEntity,
}

/// The state of `EntityDecoder`.
#[derive(Clone, Copy)]
struct EntityDecoder {
    state: EntityState,
    /// Bytes read while they are part of the entity, the `&` included.
    consumed: usize,
    /// The code point of a numeric entity, or the place in the trie of the longest named
    /// entity read so far that does not need a semicolon.
    result: usize,
    tree_index: usize,
    /// Bytes read past `result`, before the character being read. Upstream's `excess`
    /// is one more: it counts the character being read, in UTF-16 code units, which is
    /// right for the semicolon it is used for.
    excess: usize,
    mode: DecodingMode,
}

/// A numeric entity past the last code point is U+FFFD whatever its digits: stop counting.
const PAST_UNICODE: usize = 0x11_0000;

impl EntityDecoder {
    fn new(mode: DecodingMode) -> Self {
        Self {
            state: EntityState::EntityStart,
            consumed: 1,
            result: 0,
            tree_index: 0,
            excess: 0,
            mode,
        }
    }
}

/// `replaceCodePoint` then `fromCodePoint`: what a numeric entity stands for.
fn numeric_character(code_point: usize) -> char {
    let replaced = match code_point {
        0 => 0xfffd,
        128 => 8364,
        130 => 8218,
        131 => 402,
        132 => 8222,
        133 => 8230,
        134 => 8224,
        135 => 8225,
        136 => 710,
        137 => 8240,
        138 => 352,
        139 => 8249,
        140 => 338,
        142 => 381,
        145 => 8216,
        146 => 8217,
        147 => 8220,
        148 => 8221,
        149 => 8226,
        150 => 8211,
        151 => 8212,
        152 => 732,
        153 => 8482,
        154 => 353,
        155 => 8250,
        156 => 339,
        158 => 382,
        159 => 376,
        other => other,
    };
    // Surrogates and what lies past U+10FFFF are no characters.
    u32::try_from(replaced)
        .ok()
        .and_then(char::from_u32)
        .unwrap_or('\u{fffd}')
}

/// What upstream's `treeIndex` is after it read past the end of the trie: `undefined` or
/// NaN, which is no node, and which the next character then fails to leave.
const OUTSIDE_THE_TREE: usize = usize::MAX / 2;

/// `determineBranch`: the node of the trie `c` leads to from `current`.
///
/// `c` is a UTF-16 code unit, as upstream compares them. The names of the entities are
/// ASCII, but upstream also looks for branches in nodes that have none, and reads their
/// value, or whatever follows, as if it were branches: `&BreveX;` is `Á` to it. What it
/// finds there can be any byte, and is found here as well.
fn determine_branch(current: u16, node_index: usize, c: u16) -> Option<usize> {
    let branch_count = usize::from((current & flag::BRANCH_LENGTH) >> 7);
    let jump_offset = current & flag::JUMP_TABLE;
    // A single branch, its character in place of the jump offset.
    if branch_count == 0 {
        return (jump_offset != 0 && c == jump_offset).then_some(node_index);
    }
    // A jump table.
    if jump_offset != 0 {
        let value = usize::from(c.checked_sub(jump_offset)?);
        if value >= branch_count {
            return None;
        }
        return match HTML_DECODE_TREE.get(node_index + value) {
            Some(&next) => usize::from(next).checked_sub(1),
            None => Some(OUTSIDE_THE_TREE),
        };
    }
    // A sorted dictionary, two keys to a word, searched exactly as upstream does: where
    // it reads words that are no dictionary, the keys are in no order, and another way
    // to halve the range would find another key.
    let packed_key_slots = (branch_count + 1) >> 1;
    let (mut low, mut high) = (0, branch_count);
    while low < high {
        // `high` is one past upstream's `hi`.
        let middle = (low + high - 1) >> 1;
        let packed = tree(node_index + (middle >> 1));
        let key = (packed >> ((middle & 1) * 8)) & 0xff;
        match key.cmp(&c) {
            std::cmp::Ordering::Less => low = middle + 1,
            std::cmp::Ordering::Greater => high = middle,
            std::cmp::Ordering::Equal => {
                let next = HTML_DECODE_TREE.get(node_index + packed_key_slots + middle);
                return Some(next.map_or(OUTSIDE_THE_TREE, |&next| usize::from(next)));
            }
        }
    }
    None
}

impl Parser<'_, '_> {
    /// `EntityDecoder.write(buffer, offset)`: how many characters the entity takes with
    /// its `&`, 0 when there is none, `None` when the document ends before it is known.
    fn entity_write(&mut self, offset: usize) -> Result<Option<usize>, GaveUp> {
        if self.bytes.get(offset) == Some(&b'#') {
            self.entity.state = EntityState::NumericStart;
            self.entity.consumed += 1;
            return self.entity_numeric_start(offset + 1);
        }
        self.entity.state = EntityState::NamedEntity;
        self.entity_named(offset)
    }

    fn entity_numeric_start(&mut self, offset: usize) -> Result<Option<usize>, GaveUp> {
        let Some(&c) = self.bytes.get(offset) else {
            return Ok(None);
        };
        if c | 0x20 == b'x' {
            self.entity.state = EntityState::NumericHex;
            self.entity.consumed += 1;
            return self.entity_digits(offset + 1, 16, 3);
        }
        self.entity.state = EntityState::NumericDecimal;
        self.entity_digits(offset, 10, 2)
    }

    /// `stateNumericDecimal` and `stateNumericHex`.
    fn entity_digits(
        &mut self,
        mut offset: usize,
        radix: u32,
        expected_length: usize,
    ) -> Result<Option<usize>, GaveUp> {
        while let Some(&c) = self.bytes.get(offset) {
            let Some(digit) = char::from(c).to_digit(radix) else {
                return self.emit_numeric_entity(c, expected_length).map(Some);
            };
            self.meter.spend(1)?;
            let result = self.entity.result * radix as usize + digit as usize;
            self.entity.result = result.min(PAST_UNICODE);
            self.entity.consumed += 1;
            offset += 1;
        }
        Ok(None)
    }

    /// `emitNumericEntity`: `expected_length` is what `&#` or `&#x` alone consume.
    fn emit_numeric_entity(&mut self, last: u8, expected_length: usize) -> Result<usize, GaveUp> {
        // No digit at all.
        if self.entity.consumed <= expected_length {
            return Ok(0);
        }
        if last == b';' {
            self.entity.consumed += 1;
        }
        let mut utf8 = [0; 4];
        let decoded = numeric_character(self.entity.result).encode_utf8(&mut utf8);
        let consumed = self.entity.consumed;
        self.emit_decoded(decoded, consumed)?;
        Ok(consumed)
    }

    /// Accounts for one character read by `entity_named`.
    ///
    /// With the trie as it is meant, no walk goes past the 32 characters of the longest
    /// name, and none reads a `&`: every character is read once. Where upstream strays
    /// (see `determine_branch`), the trie has cycles and `&` among their characters, so a
    /// document could be written to have each `&` start a walk through all that follows.
    /// None was found, but none is needed: a document that makes the decoder read eight
    /// times its length is given up on, which keeps the parser linear whatever the trie.
    fn entity_step(&mut self) -> Result<(), GaveUp> {
        self.meter.spend(1)?;
        self.entity_steps_left = self
            .entity_steps_left
            .checked_sub(1)
            .ok_or(GaveUp::TooSlow)?;
        Ok(())
    }

    /// The UTF-16 code unit at `offset` and how many bytes it takes, for the code units
    /// the trie can hold: up to U+00FF. Anything above is a code unit that matches nothing.
    fn entity_unit(&self, offset: usize) -> Option<(u16, usize)> {
        let &first = self.bytes.get(offset)?;
        Some(match (first, self.bytes.get(offset + 1)) {
            (0..0x80, _) => (u16::from(first), 1),
            (0xc2 | 0xc3, Some(&second)) => {
                (u16::from(first & 0x1f) << 6 | u16::from(second & 0x3f), 2)
            }
            _ => (u16::MAX, 1),
        })
    }

    /// `stateNamedEntity`. The trie holds runs of characters without branches as such,
    /// which upstream can leave half read between two chunks; here a run is read at once.
    fn entity_named(&mut self, mut offset: usize) -> Result<Option<usize>, GaveUp> {
        let mut current = tree(self.entity.tree_index);
        let mut value_length = (current & flag::VALUE_LENGTH) >> 14;
        while offset < self.bytes.len() {
            if value_length == 0 && current & flag::FLAG13 != 0 {
                let run_length = usize::from((current & flag::BRANCH_LENGTH) >> 7);
                // The first character is in the node and the others, two to a word, after
                // it. The first is read whatever the length says, as upstream does with
                // the words it takes for runs.
                for run_consumed in 0..run_length.max(1) {
                    let Some((c, width)) = self.entity_unit(offset) else {
                        return Ok(None);
                    };
                    let expected = match run_consumed.checked_sub(1) {
                        None => current & flag::JUMP_TABLE,
                        Some(index) => {
                            let packed = tree(self.entity.tree_index + 1 + (index >> 1));
                            if index % 2 == 0 {
                                packed & 0xff
                            } else {
                                packed >> 8
                            }
                        }
                    };
                    if c != expected {
                        return self.entity_not_matched().map(Some);
                    }
                    self.entity_step()?;
                    offset += width;
                    self.entity.excess += width;
                }
                self.entity.tree_index += 1 + (run_length >> 1);
                current = tree(self.entity.tree_index);
                value_length = (current & flag::VALUE_LENGTH) >> 14;
            }
            let Some((c, width)) = self.entity_unit(offset) else {
                break;
            };
            // A node that needs a semicolon and has no branch for it.
            if c == u16::from(b';') && value_length != 0 && current & flag::FLAG13 != 0 {
                let consumed = self.entity.consumed + self.entity.excess + 1;
                return self
                    .emit_named_entity(self.entity.tree_index, consumed)
                    .map(Some);
            }
            let after_value = self.entity.tree_index + usize::from(value_length.max(1));
            let Some(next) = determine_branch(current, after_value, c) else {
                // In an attribute, `&name=` and `&nameX` are text, as browsers have it.
                let is_invalid_end =
                    u8::try_from(c).is_ok_and(|c| c == b'=' || c.is_ascii_alphanumeric());
                let is_text = self.entity.result == 0
                    || (self.entity.mode == DecodingMode::Attribute
                        && (value_length == 0 || is_invalid_end));
                return if is_text {
                    Ok(Some(0))
                } else {
                    self.entity_not_terminated().map(Some)
                };
            };
            self.entity_step()?;
            self.entity.tree_index = next;
            current = tree(next);
            value_length = (current & flag::VALUE_LENGTH) >> 14;
            offset += width;
            self.entity.excess += width;
            if value_length != 0 {
                if c == u16::from(b';') {
                    let consumed = self.entity.consumed + self.entity.excess;
                    return self.emit_named_entity(next, consumed).map(Some);
                }
                // An entity HTML also knows without its semicolon: remember it, and
                // look for a longer one.
                if current & flag::FLAG13 == 0 {
                    self.entity.result = next;
                    self.entity.consumed += self.entity.excess;
                    self.entity.excess = 0;
                }
            }
        }
        Ok(None)
    }

    /// A run of the trie does not match: `result === 0 ? 0 : emitNotTerminatedNamedEntity()`.
    fn entity_not_matched(&mut self) -> Result<usize, GaveUp> {
        if self.entity.result == 0 {
            Ok(0)
        } else {
            self.entity_not_terminated()
        }
    }

    /// `emitNotTerminatedNamedEntity`.
    fn entity_not_terminated(&mut self) -> Result<usize, GaveUp> {
        let consumed = self.entity.consumed;
        self.emit_named_entity(self.entity.result, consumed)
    }

    /// `emitNamedEntityData`. The trie holds UTF-16 code units: one in the node itself,
    /// one in the next word, or two in the next words, which are either two characters or
    /// the two halves of one.
    fn emit_named_entity(&mut self, result: usize, consumed: usize) -> Result<usize, GaveUp> {
        // `String.fromCodePoint(undefined)` is a RangeError.
        let word = |index: usize| HTML_DECODE_TREE.get(index).copied().ok_or(GaveUp::Throws);
        let node = word(result)?;
        let (first, second) = match (node & flag::VALUE_LENGTH) >> 14 {
            1 => (node & !(flag::VALUE_LENGTH | flag::FLAG13), None),
            3 => (word(result + 1)?, Some(word(result + 2)?)),
            _ => (word(result + 1)?, None),
        };
        let mut decoded = std::mem::take(&mut self.decoded);
        decoded.clear();
        // Half a pair, which only the nodes upstream reads by mistake hold, leaves Node
        // as U+FFFD.
        decoded.extend(
            char::decode_utf16(std::iter::once(first).chain(second))
                .map(|character| character.unwrap_or('\u{fffd}')),
        );
        let emitted = self.emit_decoded(&decoded, consumed);
        self.decoded = decoded;
        emitted?;
        Ok(consumed)
    }

    /// `EntityDecoder.end()`: the document ends inside an entity.
    fn entity_end(&mut self) -> Result<(), GaveUp> {
        match self.entity.state {
            EntityState::NamedEntity => {
                let is_entity = self.entity.result != 0
                    && (self.entity.mode != DecodingMode::Attribute
                        || self.entity.result == self.entity.tree_index);
                if is_entity {
                    self.entity_not_terminated()?;
                }
            }
            EntityState::NumericDecimal => {
                self.emit_numeric_entity(0, 2)?;
            }
            EntityState::NumericHex => {
                self.emit_numeric_entity(0, 3)?;
            }
            EntityState::NumericStart | EntityState::EntityStart => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------
// html-to-text: BlockTextBuilder, InlineTextBuilder, WhitespaceProcessor
// ---------------------------------------------------------------------------------------

/// What a block does to its text when it closes.
#[derive(Clone, Copy)]
enum Shape {
    /// `BlockStackItem`: nothing.
    Plain,
    /// A `blockquote`: `blockTransform` trims the line breaks around the text and starts
    /// every line with `> `.
    Quote,
    /// `ListStackItem`. `widest` is `maxPrefixLength`.
    List { widest: usize },
    /// `ListItemStackItem`: its prefix, at `at` for `len` bytes in `Builder::prefixes`,
    /// padded to `width`, then its text with every further line indented by `width`.
    Item { at: usize, len: usize, width: usize },
}

/// A `StackItem` of `BlockTextBuilder` with its `InlineTextBuilder`.
///
/// Upstream keeps the text of each open block in a string of its own and copies it into
/// its parent when the block closes, once for each level it is nested in. Here every
/// block writes straight into the one result, which is possible because what a closing
/// block adds before its text (line breaks, `> `, a list prefix) is known by the time
/// the block gets its first character: see `Builder::begin`.
struct Block {
    shape: Shape,
    /// `leadingLineBreaks`.
    leading: u32,
    /// `stashedLineBreaks`.
    stashed: u32,
    /// Whether `getText` of the block would be a non-empty string once the blocks open
    /// inside it are closed.
    has_text: bool,
    /// `isPre`.
    is_pre: bool,
    /// `nextLineWords` is not empty: the next word is put after a space.
    line_has_words: bool,
    /// `stashedSpace` of the inline builder.
    stashed_space: bool,
}

/// A `blockquote` or a list item with text, which changes the lines written inside it.
struct Margin {
    /// The length of `Builder::margin` without this one.
    outer: usize,
    is_quote: bool,
    /// A quote has only had line breaks so far: `trimCharacter(str, '\n')` drops them.
    at_start: bool,
    /// Line breaks of a quote not written yet: they are dropped if the quote ends with
    /// them, and written with the next character otherwise.
    pending: usize,
}

/// A word transform pushed by `pushWordTransform`.
enum Transform<'d> {
    /// Of a heading: `str => str.toUpperCase()`.
    Upper,
    /// Of an anchor, which gathers the words inside it to compare them with its `href`.
    Link(Link<'d>),
}

/// The `text` variable of `formatAnchor`, as much of it as is used: whether it is still
/// the beginning of `href`, and whether it is empty.
struct Link<'d> {
    href: &'d str,
    /// What of `href` the words so far have not matched.
    rest: &'d str,
    /// `Builder::words` when the anchor opened.
    words_before: u64,
}

/// `BlockTextBuilder`, writing the text as it goes.
struct Builder<'d, 'm> {
    meter: &'m mut Meter,
    /// The result. Only ever appended to.
    out: String,
    /// `_stackItem` and the items after it, the root first.
    blocks: Vec<Block>,
    /// The blocks before this one have text, the others do not.
    first_empty: usize,
    /// The open quotes and list items that have text.
    margins: Vec<Margin>,
    /// What they put at the start of every line but the first of each.
    margin: String,
    /// The prefixes of the open list items.
    prefixes: String,
    /// `_wordTransformer`, innermost last.
    transforms: Vec<Transform<'d>>,
    /// Where the `Upper` transforms are in `transforms`.
    uppers: Vec<usize>,
    /// Where the links are whose words so far are the beginning of their `href`. A link
    /// leaves the list for good with the first word that does not match, so comparing a
    /// word costs each link no more than the length of its `href` over the whole document,
    /// however many links are open.
    matching: Vec<usize>,
    /// How many words went through the transforms.
    words: u64,
    scratch: String,
}

/// `whitespaceCharacters`, which is `' \t\r\n\f\u200b'`: the length of the one `text`
/// starts with, 0 when it starts with none.
fn space_at(text: &[u8]) -> usize {
    match text {
        [b' ' | b'\t' | b'\r' | b'\n' | 0x0c, ..] => 1,
        [0xe2, 0x80, 0x8b, ..] => 3,
        _ => 0,
    }
}

/// `trailingWhitespaceRe`.
fn ends_with_space(text: &str) -> bool {
    text.ends_with([' ', '\t', '\r', '\n', '\u{c}', '\u{200b}'])
}

/// The matches of `wordRe`: what is between whitespace characters.
struct Words<'t> {
    text: &'t str,
    at: usize,
}

impl<'t> Iterator for Words<'t> {
    type Item = &'t str;

    fn next(&mut self) -> Option<&'t str> {
        let bytes = self.text.as_bytes();
        loop {
            match space_at(bytes.get(self.at..)?) {
                0 => break,
                space => self.at += space,
            }
        }
        let start = self.at;
        while let Some(rest) = bytes.get(self.at..)
            && !rest.is_empty()
            && space_at(rest) == 0
        {
            self.at += 1;
        }
        // A whitespace character starts a UTF-8 sequence, so both ends are between two.
        self.text
            .get(start..self.at)
            .filter(|word| !word.is_empty())
    }
}

impl<'d, 'm> Builder<'d, 'm> {
    fn new(meter: &'m mut Meter) -> Self {
        Self {
            meter,
            out: String::new(),
            blocks: Vec::new(),
            first_empty: 0,
            margins: Vec::new(),
            margin: String::new(),
            prefixes: String::new(),
            transforms: Vec::new(),
            uppers: Vec::new(),
            matching: Vec::new(),
            words: 0,
            scratch: String::new(),
        }
    }

    fn open(&mut self, shape: Shape, leading: u32, is_pre: bool) -> Result<(), GaveUp> {
        let is_pre = is_pre || self.blocks.last().is_some_and(|parent| parent.is_pre);
        let block = Block {
            shape,
            leading,
            stashed: 0,
            has_text: false,
            is_pre,
            line_has_words: false,
            stashed_space: false,
        };
        self.meter.push(&mut self.blocks, block)
    }

    /// The innermost block gets its first character, or closes with text of its own
    /// making: it and the blocks around it that were empty until now start here.
    ///
    /// This is `addText` of each of these blocks into its parent, done ahead. Upstream
    /// does it when the blocks close, one after the other from the innermost: a parent
    /// without text takes the text as it is and its `leadingLineBreaks` from the child,
    /// and the first parent with text puts line breaks before it. All of that is decided
    /// now, since none of these blocks can change before the innermost closes.
    fn begin(&mut self) -> Result<(), GaveUp> {
        let first = self.first_empty;
        if first >= self.blocks.len() {
            return Ok(());
        }
        let mut leading = None;
        for block in self.blocks.iter_mut().skip(first).rev() {
            if let Some(of_child) = leading {
                block.leading = block.stashed.max(of_child);
            }
            leading = Some(block.leading);
        }
        if let Some(parent) = first.checked_sub(1).and_then(|at| self.blocks.get(at)) {
            let breaks = parent.stashed.max(leading.unwrap_or(0));
            self.line_breaks(breaks as usize)?;
        }
        for at in first..self.blocks.len() {
            let Some(block) = self.blocks.get_mut(at) else {
                break;
            };
            block.has_text = true;
            match block.shape {
                Shape::Quote => {
                    self.settle()?;
                    self.meter.append(&mut self.out, "> ")?;
                    self.push_margin(true)?;
                    self.meter.append(&mut self.margin, "> ")?;
                }
                // `prefix.padEnd(prefixLength)`, and `'\n' + ' '.repeat(prefixLength)`
                // for the line breaks to come.
                Shape::Item { at, len, width } if width > 0 => {
                    self.settle()?;
                    let prefix = self.prefixes.get(at..at + len).unwrap_or_default();
                    self.meter.room_in(&mut self.out, width)?;
                    self.out.push_str(prefix);
                    self.out
                        .extend(std::iter::repeat_n(' ', width.saturating_sub(len)));
                    self.push_margin(false)?;
                    self.meter.room_in(&mut self.margin, width)?;
                    self.margin.extend(std::iter::repeat_n(' ', width));
                }
                _ => {}
            }
        }
        self.first_empty = self.blocks.len();
        Ok(())
    }

    fn push_margin(&mut self, is_quote: bool) -> Result<(), GaveUp> {
        let margin = Margin {
            outer: self.margin.len(),
            is_quote,
            at_start: true,
            pending: 0,
        };
        self.meter.push(&mut self.margins, margin)
    }

    /// Adds line breaks to the text of the innermost block that has text.
    fn line_breaks(&mut self, count: usize) -> Result<(), GaveUp> {
        match self.margins.last_mut() {
            Some(quote) if quote.is_quote => {
                if !quote.at_start {
                    quote.pending += count;
                }
                Ok(())
            }
            _ => self.write_line_breaks(count),
        }
    }

    /// Each line break is followed by what the open quotes and list items start a line
    /// with, as their transforms would add when they close.
    fn write_line_breaks(&mut self, count: usize) -> Result<(), GaveUp> {
        let bytes = count
            .checked_mul(1 + self.margin.len())
            .ok_or(GaveUp::TooLarge)?;
        self.meter.spend(bytes)?;
        self.meter.room_in(&mut self.out, bytes)?;
        for _ in 0..count {
            self.out.push('\n');
            self.out.push_str(&self.margin);
        }
        Ok(())
    }

    /// Something other than a line break is about to be written: the innermost quote is
    /// past the line breaks it trims at its start, and keeps the ones it was holding.
    fn settle(&mut self) -> Result<(), GaveUp> {
        match self.margins.last_mut() {
            Some(quote) if quote.is_quote => {
                quote.at_start = false;
                let pending = std::mem::take(&mut quote.pending);
                self.write_line_breaks(pending)
            }
            _ => Ok(()),
        }
    }

    /// `closeBlock`, `closeListItem` and `closeList`: the `addText` they end with.
    fn close(&mut self, trailing: u32) -> Result<(), GaveUp> {
        let Some(block) = self.blocks.last() else {
            return Ok(());
        };
        let makes_text = match block.shape {
            // `'> ' + line` for the one empty line of an empty quote.
            Shape::Quote => true,
            Shape::Item { width, .. } => width > 0,
            _ => false,
        };
        if makes_text {
            self.begin()?;
        }
        let Some(block) = self.blocks.pop() else {
            return Ok(());
        };
        self.first_empty = self.first_empty.min(self.blocks.len());
        if let Shape::Item { at, .. } = block.shape {
            self.prefixes.truncate(at);
        }
        if block.has_text {
            if makes_text && let Some(margin) = self.margins.pop() {
                // The line breaks a quote was still holding are the ones it trims.
                self.margin.truncate(margin.outer);
            }
        } else if matches!(block.shape, Shape::List { .. }) {
            // `closeList` adds nothing for a list without text.
            return Ok(());
        }
        let stashed = match block.shape {
            Shape::Plain | Shape::Quote => block.stashed.max(trailing),
            Shape::Item { .. } => block.stashed.max(1),
            Shape::List { .. } => trailing,
        };
        let Some(parent) = self.blocks.last_mut() else {
            return Ok(());
        };
        let breaks = parent.stashed.max(block.leading);
        let parent_has_text = parent.has_text;
        if !block.has_text && !parent_has_text {
            parent.leading = breaks;
        }
        parent.stashed = stashed;
        // `inlineTextBuilder.clear()`, which leaves `stashedSpace` as it is.
        parent.line_has_words = false;
        if !block.has_text && parent_has_text {
            // The line breaks before a text that is empty.
            self.line_breaks(breaks as usize)?;
        }
        Ok(())
    }

    /// `openBlock`.
    fn open_block(&mut self, leading: u32, is_pre: bool) -> Result<(), GaveUp> {
        self.open(Shape::Plain, leading, is_pre)
    }

    /// `openList`.
    fn open_list(&mut self, leading: u32, widest: usize) -> Result<(), GaveUp> {
        self.open(Shape::List { widest }, leading, false)
    }

    /// `openListItem`.
    fn open_item(&mut self, prefix: &str) -> Result<(), GaveUp> {
        let widest = match self.blocks.last().map(|list| list.shape) {
            Some(Shape::List { widest }) => widest,
            _ => 0,
        };
        let at = self.prefixes.len();
        self.meter.append(&mut self.prefixes, prefix)?;
        let shape = Shape::Item {
            at,
            len: prefix.len(),
            width: prefix.len().max(widest),
        };
        // `interRowLineBreaks`.
        self.open(shape, 1, false)
    }

    /// `addLineBreak`.
    fn add_line_break(&mut self) -> Result<(), GaveUp> {
        self.begin()?;
        self.line_breaks(1)?;
        if let Some(block) = self.blocks.last_mut()
            && !block.is_pre
        {
            // `startNewLine`.
            block.line_has_words = false;
        }
        Ok(())
    }

    /// `addInline`, with `noWordTransform` when `transform` is false.
    fn add_inline(&mut self, text: &str, transform: bool) -> Result<(), GaveUp> {
        self.meter.spend(text.len())?;
        let Some(block) = self.blocks.last() else {
            return Ok(());
        };
        if text.is_empty() {
            return Ok(());
        }
        if block.is_pre {
            // `rawText += str`.
            self.begin()?;
            for (line, text) in text.split('\n').enumerate() {
                if line > 0 {
                    self.line_breaks(1)?;
                }
                if !text.is_empty() {
                    self.settle()?;
                    self.meter.append(&mut self.out, text)?;
                }
            }
            return Ok(());
        }

        let stashed = block.stashed;
        let stashed_space = block.stashed_space;
        let mut words = Words { text, at: 0 };
        let first = words.next();
        // Stashed line breaks make whitespace irrelevant.
        if stashed > 0 && first.is_none() {
            return Ok(());
        }
        if stashed > 0 {
            // `startNewLine(stashedLineBreaks)`.
            self.begin()?;
            self.line_breaks(stashed as usize)?;
            if let Some(block) = self.blocks.last_mut() {
                block.line_has_words = false;
            }
        }
        // `shrinkWrapAdd`: the first word continues the last one unless whitespace is
        // between them, here or stashed.
        if let Some(first) = first {
            self.begin()?;
            let is_apart = stashed_space || space_at(text.as_bytes()) > 0;
            self.add_word(first, is_apart, transform)?;
            for word in words {
                self.add_word(word, true, transform)?;
            }
        }
        if let Some(block) = self.blocks.last_mut() {
            block.stashed_space = (stashed_space && first.is_none()) || ends_with_space(text);
            block.stashed = 0;
        }
        Ok(())
    }

    /// `pushWord` when `is_apart`, `concatWord` otherwise, after the word transforms.
    fn add_word(&mut self, word: &str, is_apart: bool, transform: bool) -> Result<(), GaveUp> {
        let mut is_upper = false;
        if transform && !self.transforms.is_empty() {
            self.words += 1;
            self.compare_with_links(word)?;
            is_upper = !self.uppers.is_empty();
        }
        self.settle()?;
        let Some(block) = self.blocks.last_mut() else {
            return Ok(());
        };
        if is_apart && block.line_has_words {
            self.meter.append(&mut self.out, " ")?;
        }
        block.line_has_words = true;
        if !is_upper {
            self.meter.append(&mut self.out, word)
        } else if word.is_ascii() {
            let start = self.out.len();
            self.meter.append(&mut self.out, word)?;
            if let Some(written) = self.out.get_mut(start..) {
                written.make_ascii_uppercase();
            }
            Ok(())
        } else {
            // `toUpperCase` maps each character on its own: no context is involved.
            let upper = || word.chars().flat_map(char::to_uppercase);
            self.meter
                .room_in(&mut self.out, upper().map(char::len_utf8).sum())?;
            self.out.extend(upper());
            Ok(())
        }
    }

    /// The `text += str` of every open anchor, without the text: each one only needs to
    /// know whether its words, one after the other, are its `href`.
    fn compare_with_links(&mut self, word: &str) -> Result<(), GaveUp> {
        if self.matching.is_empty() {
            return Ok(());
        }
        self.meter.spend(self.matching.len())?;
        // A link sees the word in upper case when a heading is open inside it: the
        // transforms apply from the innermost.
        let innermost_upper = self.uppers.last().copied();
        let transforms = &mut self.transforms;
        self.matching.retain(|&at| {
            let Some(Transform::Link(link)) = transforms.get_mut(at) else {
                return false;
            };
            let is_upper = innermost_upper.is_some_and(|upper| upper > at);
            match strip_word(link.rest, word, is_upper) {
                Some(rest) => {
                    link.rest = rest;
                    true
                }
                None => false,
            }
        });
        Ok(())
    }

    /// `pushWordTransform(str => str.toUpperCase())`.
    fn push_upper(&mut self) -> Result<(), GaveUp> {
        self.meter.push(&mut self.uppers, self.transforms.len())?;
        self.meter.push(&mut self.transforms, Transform::Upper)
    }

    /// `pushWordTransform` of `formatAnchor`.
    fn push_link(&mut self, href: &'d str) -> Result<(), GaveUp> {
        self.meter.push(&mut self.matching, self.transforms.len())?;
        let link = Link {
            href,
            rest: href,
            words_before: self.words,
        };
        self.meter.push(&mut self.transforms, Transform::Link(link))
    }

    /// `popWordTransform`, and for an anchor what `formatAnchor` does next.
    fn pop_transform(&mut self) -> Result<(), GaveUp> {
        let at = self.transforms.len().saturating_sub(1);
        match self.transforms.pop() {
            None => Ok(()),
            Some(Transform::Upper) => {
                self.uppers.pop();
                Ok(())
            }
            Some(Transform::Link(link)) => {
                let is_matching = self.matching.last() == Some(&at);
                if is_matching {
                    self.matching.pop();
                }
                // `hideLinkHrefIfSameAsText && href === text`
                if is_matching && link.rest.is_empty() {
                    return Ok(());
                }
                if self.words == link.words_before {
                    // No text: the address alone.
                    return self.add_inline(link.href, false);
                }
                let mut bracketed = std::mem::take(&mut self.scratch);
                bracketed.clear();
                self.meter.room_in(&mut bracketed, link.href.len() + 3)?;
                bracketed.push_str(" [");
                bracketed.push_str(link.href);
                bracketed.push(']');
                let added = self.add_inline(&bracketed, false);
                self.scratch = bracketed;
                added
            }
        }
    }
}

/// `rest` without `word` at its start, or `None` when it does not start with it.
fn strip_word<'d>(rest: &'d str, word: &str, is_upper: bool) -> Option<&'d str> {
    if !is_upper {
        return rest.strip_prefix(word);
    }
    word.chars()
        .flat_map(char::to_uppercase)
        .try_fold(rest, |rest, character| rest.strip_prefix(character))
}

// ---------------------------------------------------------------------------------------
// html-to-text: the formatters and the walk
// ---------------------------------------------------------------------------------------

/// The formatter html-to-text picks for an element: `selectors` of `DEFAULT_OPTIONS` with
/// the two of `OPTIONS` merged in, and the options of each.
enum Format {
    /// `inline`, the format of `*`.
    Inline,
    /// `skip`, the format `OPTIONS` gives `img`; and `wbr`, which only matters to lines
    /// that wrap.
    Skip,
    /// `block`, `paragraph`, and `table` for a table that is not a data table:
    /// `leadingLineBreaks` and `trailingLineBreaks`.
    Block(u32, u32),
    Pre,
    /// `heading` in upper case: `leadingLineBreaks`.
    Heading(u32),
    Blockquote,
    HorizontalLine,
    LineBreak,
    /// `anchor`, with `hideLinkHrefIfSameAsText` from `OPTIONS`.
    Anchor,
    UnorderedList,
    OrderedList,
    /// Not a formatter: the picker looks the name up in an object, finds
    /// `Object.prototype.constructor`, and fails on what calling it returns.
    Throws,
}

fn format_of(name: Name) -> Format {
    use name::*;
    match name {
        A => Format::Anchor,
        ARTICLE | ASIDE | DIV | FOOTER | FORM | HEADER | MAIN | NAV | SECTION => {
            Format::Block(1, 1)
        }
        P | TABLE => Format::Block(2, 2),
        BLOCKQUOTE => Format::Blockquote,
        BR => Format::LineBreak,
        H1 | H2 | H3 => Format::Heading(3),
        H4 | H5 | H6 => Format::Heading(2),
        HR => Format::HorizontalLine,
        IMG | WBR => Format::Skip,
        OL => Format::OrderedList,
        PRE => Format::Pre,
        UL => Format::UnorderedList,
        CONSTRUCTOR => Format::Throws,
        _ => Format::Inline,
    }
}

/// What a formatter does after `walk(elem.children, builder)`.
#[derive(Clone, Copy)]
enum Then {
    Nothing,
    /// `closeBlock({ trailingLineBreaks })`.
    CloseBlock(u32),
    /// `popWordTransform()`, then `closeBlock`.
    CloseHeading,
    /// The end of `formatAnchor`.
    CloseAnchor,
    /// `closeListItem()`.
    CloseItem,
}

/// How a list numbers its items.
#[derive(Clone, Copy)]
enum Numbering {
    /// `ul`: `itemPrefix`.
    Bullet,
    /// `ol`: `getOrderedListIndexFunction(type)`.
    Decimal,
    Letters(u8),
    Roman {
        is_lower: bool,
    },
}

/// A call of `walk` that has not returned: `recursiveWalk` without the recursion, so
/// that nesting costs memory that is counted and not stack.
enum Frame {
    /// The rest of a `dom` array, and what the formatter that walks it does last.
    Nodes {
        next: u32,
        /// `walk([node], builder)`: one node, not its siblings.
        is_alone: bool,
        /// The name of the element these nodes are children of.
        parent: Name,
        then: Then,
    },
    /// The loop over `listItems` of `formatList`.
    List {
        next: u32,
        numbering: Numbering,
        is_nested: bool,
        /// `nextIndex`.
        index: f64,
    },
}

/// `process`: `walk(bases, builder)` then `builder.toString()`.
struct Walk<'d, 'm> {
    dom: &'d Dom,
    builder: Builder<'d, 'm>,
    frames: Vec<Frame>,
    prefix: String,
}

/// The dashes of `hr`: `'-'.repeat(formatOptions.length || options.wordwrap || 40)`.
const HORIZONTAL_LINE: &str = "----------------------------------------";

impl<'d, 'm> Walk<'d, 'm> {
    fn new(dom: &'d Dom, meter: &'m mut Meter) -> Self {
        Self {
            dom,
            builder: Builder::new(meter),
            frames: Vec::new(),
            prefix: String::new(),
        }
    }

    fn run(mut self) -> Result<String, GaveUp> {
        // The root `BlockStackItem`.
        self.builder.open_block(1, false)?;
        if self.dom.bases.is_empty() {
            // `returnDomByDefault`: no `body`, the whole document.
            self.walk(self.dom.first, false, NONE, Then::Nothing)?;
            self.walk_frames()?;
        } else {
            for &base in &self.dom.bases {
                self.walk(base, true, NONE, Then::Nothing)?;
                self.walk_frames()?;
            }
        }
        Ok(self.builder.out)
    }

    fn walk(&mut self, next: u32, is_alone: bool, parent: Name, then: Then) -> Result<(), GaveUp> {
        let frame = Frame::Nodes {
            next,
            is_alone,
            parent,
            then,
        };
        self.builder.meter.push(&mut self.frames, frame)
    }

    fn walk_frames(&mut self) -> Result<(), GaveUp> {
        while let Some(frame) = self.frames.last_mut() {
            self.builder.meter.spend(1)?;
            match frame {
                Frame::Nodes {
                    next,
                    is_alone,
                    parent,
                    then,
                } => {
                    let (id, parent, then) = (*next, *parent, *then);
                    if id == NONE {
                        self.frames.pop();
                        self.after_walk(then)?;
                    } else {
                        *next = if *is_alone { NONE } else { self.dom.next(id) };
                        self.format(id, parent)?;
                    }
                }
                Frame::List { .. } => self.next_list_item()?,
            }
        }
        Ok(())
    }

    fn after_walk(&mut self, then: Then) -> Result<(), GaveUp> {
        match then {
            Then::Nothing => Ok(()),
            Then::CloseBlock(trailing) => self.builder.close(trailing),
            Then::CloseHeading => {
                self.builder.pop_transform()?;
                self.builder.close(2)
            }
            Then::CloseAnchor => self.builder.pop_transform(),
            Then::CloseItem => self.builder.close(1),
        }
    }

    /// The body of the loop of `recursiveWalk`, and the formatters.
    fn format(&mut self, id: u32, parent: Name) -> Result<(), GaveUp> {
        let Some(node) = self.dom.node(id) else {
            return Ok(());
        };
        match node.kind {
            TEXT => return self.builder.add_inline(self.dom.data(node), true),
            OTHER => return Ok(()),
            _ => {}
        }
        let name = node.kind;
        match format_of(name) {
            Format::Inline => self.walk_children(node)?,
            Format::Skip => {}
            Format::Block(leading, trailing) => {
                self.builder.open_block(leading, false)?;
                self.walk(node.first, false, name, Then::CloseBlock(trailing))?;
            }
            Format::Pre => {
                self.builder.open_block(2, true)?;
                self.walk(node.first, false, name, Then::CloseBlock(2))?;
            }
            Format::Heading(leading) => {
                self.builder.open_block(leading, false)?;
                self.builder.push_upper()?;
                self.walk(node.first, false, name, Then::CloseHeading)?;
            }
            Format::Blockquote => {
                self.builder.open(Shape::Quote, 2, false)?;
                self.walk(node.first, false, name, Then::CloseBlock(2))?;
            }
            Format::HorizontalLine => {
                self.builder.open_block(2, false)?;
                self.builder.add_inline(HORIZONTAL_LINE, true)?;
                self.builder.close(2)?;
            }
            Format::LineBreak => self.builder.add_line_break()?,
            Format::Anchor => {
                // `getHref`: no address, and none shown for a link into the page itself.
                let href = self.dom.value(node, 0).unwrap_or_default();
                let href = href.strip_prefix("mailto:").unwrap_or(href);
                if href.is_empty() || href.starts_with('#') {
                    self.walk_children(node)?;
                } else {
                    self.builder.push_link(href)?;
                    self.walk(node.first, false, name, Then::CloseAnchor)?;
                }
            }
            Format::UnorderedList => self.format_list(node, parent, Numbering::Bullet, 1.0)?,
            Format::OrderedList => {
                // `Number(elem.attribs.start || '1')`
                let start = match self.dom.value(node, 0) {
                    Some(start) if !start.is_empty() => to_number(start),
                    _ => 1.0,
                };
                let numbering = match self.dom.value(node, 1) {
                    Some("a") => Numbering::Letters(b'a'),
                    Some("A") => Numbering::Letters(b'A'),
                    Some("i") => Numbering::Roman { is_lower: true },
                    Some("I") => Numbering::Roman { is_lower: false },
                    _ => Numbering::Decimal,
                };
                self.format_list(node, parent, numbering, start)?;
            }
            Format::Throws => return Err(GaveUp::Throws),
        }
        Ok(())
    }

    /// `walk(elem.children, builder)` with nothing to do after it.
    fn walk_children(&mut self, element: Node) -> Result<(), GaveUp> {
        if element.first == NONE {
            return Ok(());
        }
        self.walk(element.first, false, element.kind, Then::Nothing)
    }

    /// The children of a list `formatList` makes items of: all but the text nodes that
    /// are only whitespace.
    fn list_item(&mut self, mut id: u32) -> Result<Option<(u32, Node)>, GaveUp> {
        while let Some(node) = self.dom.node(id) {
            self.builder.meter.spend(1)?;
            if node.kind != TEXT {
                return Ok(Some((id, node)));
            }
            let data = self.dom.data(node);
            self.builder.meter.spend(data.len())?;
            if !data.chars().all(is_js_whitespace) {
                return Ok(Some((id, node)));
            }
            id = node.next;
        }
        Ok(None)
    }

    /// `formatList`, up to its loop over the items.
    fn format_list(
        &mut self,
        list: Node,
        parent: Name,
        numbering: Numbering,
        start: f64,
    ) -> Result<(), GaveUp> {
        let is_nested = parent == name::LI;
        // Upstream makes every prefix before the first item to know the longest. Here
        // they are made twice instead of kept: now for their length, later for each item.
        let mut widest = 0;
        let mut index = start;
        let mut has_items = false;
        let mut next = list.first;
        while let Some((_, item)) = self.list_item(next)? {
            has_items = true;
            if item.kind == name::LI {
                self.make_prefix(numbering, index, is_nested)?;
                widest = widest.max(self.prefix.len());
                index += 1.0;
            }
            next = item.next;
        }
        if !has_items {
            return Ok(());
        }
        self.builder
            .open_list(if is_nested { 1 } else { 2 }, widest)?;
        let frame = Frame::List {
            next: list.first,
            numbering,
            is_nested,
            index: start,
        };
        self.builder.meter.push(&mut self.frames, frame)
    }

    /// One turn of the loop over `listItems`, or `closeList` after the last.
    fn next_list_item(&mut self) -> Result<(), GaveUp> {
        let Some(&Frame::List {
            next,
            numbering,
            is_nested,
            index,
        }) = self.frames.last()
        else {
            return Ok(());
        };
        let Some((id, item)) = self.list_item(next)? else {
            self.frames.pop();
            return self.builder.close(if is_nested { 1 } else { 2 });
        };
        self.prefix.clear();
        let is_numbered = item.kind == name::LI;
        if is_numbered {
            self.make_prefix(numbering, index, is_nested)?;
        }
        if let Some(Frame::List { next, index, .. }) = self.frames.last_mut() {
            *next = item.next;
            if is_numbered {
                *index += 1.0;
            }
        }
        self.builder.open_item(&self.prefix)?;
        let list = if matches!(numbering, Numbering::Bullet) {
            name::UL
        } else {
            name::OL
        };
        self.walk(id, true, list, Then::CloseItem)
    }

    /// `nextPrefixCallback()`, with `trimStart()` in a list nested in a list item. The
    /// prefix is ASCII, so that its length in bytes is the `length` JavaScript pads with.
    fn make_prefix(
        &mut self,
        numbering: Numbering,
        index: f64,
        is_nested: bool,
    ) -> Result<(), GaveUp> {
        let prefix = &mut self.prefix;
        prefix.clear();
        // The space `trimStart` removes: no index starts with one.
        if !is_nested {
            prefix.push(' ');
        }
        match numbering {
            Numbering::Bullet => prefix.push_str("* "),
            Numbering::Decimal => {
                prefix.push_str(&number_to_string(index));
                prefix.push_str(". ");
            }
            Numbering::Letters(base) => {
                number_to_letter_sequence(index, base, prefix);
                prefix.push_str(". ");
            }
            Numbering::Roman { is_lower } => {
                let start = prefix.len();
                number_to_roman(index, prefix)?;
                if is_lower && let Some(roman) = prefix.get_mut(start..) {
                    roman.make_ascii_lowercase();
                }
                prefix.push_str(". ");
            }
        }
        Ok(())
    }
}

/// `\s` of a JavaScript regular expression, which is also what `Number` takes for
/// whitespace.
fn is_js_whitespace(character: char) -> bool {
    matches!(character, '\t'..='\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}')
        || matches!(
            character,
            '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
        )
}

// ---------------------------------------------------------------------------------------
// JavaScript numbers, for the `start` of an ordered list
// ---------------------------------------------------------------------------------------

/// `Number(text)`.
fn to_number(text: &str) -> f64 {
    let text = text.trim_matches(is_js_whitespace);
    if text.is_empty() {
        return 0.0;
    }
    let radix = match text.get(..2) {
        Some("0x" | "0X") => 16,
        Some("0b" | "0B") => 2,
        Some("0o" | "0O") => 8,
        _ => return decimal_to_number(text),
    };
    integer_to_number(text.get(2..).unwrap_or_default(), radix)
}

/// A `StrDecimalLiteral`, or NaN.
fn decimal_to_number(text: &str) -> f64 {
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    if unsigned == "Infinity" {
        return if text.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    // Digits, a fraction, an exponent: what Rust parses as well, but Rust also takes
    // `inf`, `nan` and `infinity` in any case, which JavaScript does not.
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (unsigned, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let is_digits = |digits: &str| digits.bytes().all(|digit| digit.is_ascii_digit());
    let is_exponent = exponent.is_none_or(|exponent| {
        let digits = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        !digits.is_empty() && is_digits(digits)
    });
    if !is_digits(whole)
        || !is_digits(fraction)
        || whole.len() + fraction.len() == 0
        || !is_exponent
    {
        return f64::NAN;
    }
    text.parse().unwrap_or(f64::NAN)
}

/// The digits of a hexadecimal, binary or octal literal, rounded to the nearest number.
fn integer_to_number(digits: &str, radix: u32) -> f64 {
    if digits.is_empty() {
        return f64::NAN;
    }
    let bits_per_digit = radix.trailing_zeros();
    // The first 64 significant bits, how many follow them, and whether one of those is set.
    let mut mantissa: u64 = 0;
    let mut dropped: i32 = 0;
    let mut is_inexact = false;
    for digit in digits.chars() {
        let Some(digit) = digit.to_digit(radix) else {
            return f64::NAN;
        };
        for bit in (0..bits_per_digit).rev() {
            let bit = u64::from(digit >> bit & 1);
            if mantissa >> 63 == 0 {
                mantissa = mantissa << 1 | bit;
            } else {
                dropped = dropped.saturating_add(1);
                is_inexact |= bit == 1;
            }
        }
    }
    // A set bit past the 64th only matters to break a tie, which this does.
    (mantissa | u64::from(is_inexact)) as f64 * 2f64.powi(dropped)
}

/// `Number.prototype.toString()`.
fn number_to_string(number: f64) -> String {
    if number.is_nan() {
        return "NaN".into();
    }
    if number == 0.0 {
        return "0".into();
    }
    if number.is_infinite() {
        return if number < 0.0 {
            "-Infinity"
        } else {
            "Infinity"
        }
        .into();
    }
    // The shortest digits that read back as the same number, as JavaScript picks them.
    let scientific = format!("{:e}", number.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let count = digits.len() as i32;
    // The number is 0.digits times ten to the `point`th.
    let point = exponent.parse::<i32>().unwrap_or(0) + 1;
    let mut text = String::new();
    if number < 0.0 {
        text.push('-');
    }
    if count <= point && point <= 21 {
        text.push_str(&digits);
        text.extend(std::iter::repeat_n('0', (point - count) as usize));
    } else if 0 < point && point <= 21 {
        let (whole, fraction) = digits
            .split_at_checked(point as usize)
            .unwrap_or((&digits, ""));
        text.push_str(whole);
        text.push('.');
        text.push_str(fraction);
    } else if -6 < point && point <= 0 {
        text.push_str("0.");
        text.extend(std::iter::repeat_n('0', (-point) as usize));
        text.push_str(&digits);
    } else {
        let (first, rest) = digits.split_at_checked(1).unwrap_or((&digits, ""));
        text.push_str(first);
        if !rest.is_empty() {
            text.push('.');
            text.push_str(rest);
        }
        text.push('e');
        text.push(if point > 0 { '+' } else { '-' });
        text.push_str(&(point - 1).abs().to_string());
    }
    text
}

/// `number >> 0`: `ToInt32`.
fn to_int32(number: f64) -> i32 {
    if !number.is_finite() {
        return 0;
    }
    let wrapped = number.trunc().rem_euclid(4_294_967_296.0);
    wrapped as u32 as i32
}

/// `numberToLetterSequence(num, baseChar)`, for any number: an index of 0 or less, a
/// fraction and NaN all give the characters JavaScript's arithmetic gives.
fn number_to_letter_sequence(mut number: f64, base: u8, text: &mut String) {
    let mut digits = Vec::new();
    loop {
        number -= 1.0;
        digits.push(number % 26.0);
        // `(num / base) >> 0`: after the first turn the number fits 32 bits, so this ends.
        number = f64::from(to_int32(number / 26.0));
        if number.is_nan() || number <= 0.0 {
            break;
        }
    }
    for digit in digits.into_iter().rev() {
        // `String.fromCharCode`: `ToUint16`, which is 0 for NaN.
        let code = f64::from(base) + digit;
        let code = if code.is_finite() {
            code.trunc().rem_euclid(65_536.0) as u32
        } else {
            0
        };
        text.push(char::from_u32(code).unwrap_or('\u{fffd}'));
    }
}

/// `numberToRoman(num)`, for any number: upstream maps over the characters of the number
/// as a string and indexes two arrays with their positions, so that a number of four
/// digits or more reads past them.
fn number_to_roman(number: f64, text: &mut String) -> Result<(), GaveUp> {
    const I: [&str; 4] = ["I", "X", "C", "M"];
    const V: [&str; 3] = ["V", "L", "D"];
    let characters = number_to_string(number);
    let count = characters.len();
    for (at, character) in characters.chars().enumerate() {
        // The place of the digit, counted from the units.
        let place = count - 1 - at;
        let one = I.get(place);
        match character.to_digit(10) {
            // `(v < 5 ? '' : V[i]) + I[i].repeat(v % 5)`
            Some(digit) if digit % 5 < 4 => {
                // `undefined.repeat` is a TypeError.
                let one = one.ok_or(GaveUp::Throws)?;
                if digit >= 5 {
                    text.push_str(V.get(place).unwrap_or(&"undefined"));
                }
                text.extend(std::iter::repeat_n(*one, (digit % 5) as usize));
            }
            // `I[i] + (v < 5 ? V[i] : I[i + 1])`, also for what is not a digit, which is
            // NaN and therefore not below 5.
            digit => match one {
                Some(one) => {
                    let is_four = digit.is_some_and(|digit| digit < 5);
                    let five_or_ten = if is_four {
                        V.get(place)
                    } else {
                        I.get(place + 1)
                    };
                    text.push_str(one);
                    text.push_str(five_or_ten.unwrap_or(&"undefined"));
                }
                // `undefined + undefined`
                None => text.push_str("NaN"),
            },
        }
    }
    Ok(())
}
