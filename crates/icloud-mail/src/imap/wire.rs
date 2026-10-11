//! What goes over an IMAP connection: how a command is written, and how a
//! response is read into a tree of tokens.
//!
//! Servers do not follow the grammar of RFC 3501 to the letter, and a strict
//! parser fails a whole listing over one odd header. This is a port of the
//! lenient reader and writer of imapflow 2.2.4 (`handler/imap-parser.js`,
//! `parser-instance.js`, `token-parser.js` and `imap-compiler.js`), which the
//! Node app relied on: what they accept, this accepts.

use std::fmt;

/// One value of a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Token {
    Atom {
        value: String,
        /// What follows in square brackets: a response code, or the section of `BODY[...]`.
        section: Option<Vec<Token>>,
        /// The `<0.1024>` after a section.
        partial: Option<Vec<u64>>,
    },
    String(String),
    Literal(Vec<u8>),
    Sequence(String),
    /// The text for people that ends a status response.
    Text(String),
    Nil,
    List(Vec<Token>),
}

impl Token {
    /// `typeof entry.value === 'string' ? entry.value : undefined`
    pub(crate) fn string_value(&self) -> Option<&str> {
        match self {
            Token::Atom { value, .. }
            | Token::String(value)
            | Token::Sequence(value)
            | Token::Text(value) => Some(value),
            Token::Literal(_) | Token::Nil | Token::List(_) => None,
        }
    }

    /// The value as text, also when the server sent it as a literal.
    pub(crate) fn text(&self) -> Option<String> {
        match self {
            Token::Literal(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
            other => other.string_value().map(str::to_owned),
        }
    }

    /// The value as the bytes the server sent.
    pub(crate) fn bytes(&self) -> Option<Vec<u8>> {
        match self {
            Token::Literal(bytes) => Some(bytes.clone()),
            other => other.string_value().map(|value| value.as_bytes().to_vec()),
        }
    }

    pub(crate) fn list(&self) -> Option<&[Token]> {
        match self {
            Token::List(items) => Some(items),
            _ => None,
        }
    }
}

/// `getStringList`: the values of a list that are text.
pub(crate) fn string_list(token: Option<&Token>) -> Vec<String> {
    token
        .and_then(Token::list)
        .map(|items| {
            items
                .iter()
                .filter_map(Token::string_value)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// One response of the server: a line, with the literals it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Response {
    /// `*`, `+`, or the tag of the command it completes.
    pub tag: String,
    pub command: String,
    pub attributes: Vec<Token>,
}

impl Response {
    /// The response code: `[UIDVALIDITY 3]` gives its tokens.
    pub(crate) fn code(&self) -> Option<&[Token]> {
        match self.attributes.first() {
            Some(Token::Atom {
                value,
                section: Some(section),
                ..
            }) if value.is_empty() => Some(section),
            _ => None,
        }
    }

    /// The name of the response code, in upper case.
    pub(crate) fn code_name(&self) -> Option<String> {
        self.code()?
            .first()?
            .string_value()
            .map(|name| name.trim().to_uppercase())
    }

    /// `getTextValues(...).map(trim).join(' ')`: what the server says in words.
    pub(crate) fn text(&self) -> String {
        let texts: Vec<&str> = self
            .attributes
            .iter()
            .filter_map(|attribute| match attribute {
                Token::Text(text) => Some(text.trim()),
                _ => None,
            })
            .collect();
        texts.join(" ")
    }
}

/// Why a response could not be read. The code is the one imapflow gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParseError {
    pub code: &'static str,
    /// The tag, when it was read before the response stopped making sense.
    pub tag: Option<String>,
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the server sent a response that cannot be read ({})",
            self.code
        )
    }
}

fn error(code: &'static str) -> ParseError {
    ParseError { code, tag: None }
}

fn is_ctl(character: char) -> bool {
    character <= '\u{1f}' || character == '\u{7f}'
}

/// `ATOM-CHAR`: any ASCII character but the ones that mean something else.
fn is_atom_char(character: char) -> bool {
    ('\u{01}'..='\u{7f}').contains(&character)
        && !matches!(
            character,
            '(' | ')' | '{' | ' ' | '%' | '*' | '"' | '\\' | ']'
        )
        && !is_ctl(character)
}

fn is_tag_char(character: char) -> bool {
    (is_atom_char(character) || character == ']') && character != '+'
}

fn is_command_char(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '-'
}

/// ECMAScript's `\s`.
fn is_space(character: char) -> bool {
    mymcps_vine::js::is_whitespace(character)
}

fn is_status(command: &str) -> bool {
    ["OK", "NO", "BAD", "BYE", "PREAUTH"].contains(&command.to_uppercase().as_str())
}

/// Read one response. `payload` is its line without the line break, and
/// `literals` the literals the line announced, in order.
pub(crate) fn parse_response(
    payload: &[u8],
    literals: Vec<Vec<u8>>,
) -> Result<Response, ParseError> {
    // Some servers put null bytes before a line.
    let payload = match payload.iter().position(|byte| *byte != 0) {
        Some(start) => &payload[start..],
        None => {
            return Ok(Response {
                tag: "*".into(),
                command: "BAD".into(),
                attributes: Vec::new(),
            });
        }
    };
    let input: Vec<char> = String::from_utf8_lossy(payload).chars().collect();
    let mut reader = LineReader {
        input: &input,
        position: 0,
    };

    let tag = reader
        .element(|character| is_tag_char(character) || character == '*' || character == '+')?;
    let tagged = |mut failure: ParseError| {
        failure.tag = Some(tag.clone());
        failure
    };
    reader.space(&tag).map_err(tagged)?;

    let mut human_readable = None;
    let mut command = String::new();
    let mut end = input.len();
    if tag == "+" {
        human_readable = Some(trimmed(reader.rest()));
        reader.position = input.len();
    } else {
        command = reader.element(is_command_char).map_err(tagged)?;
        if is_status(&command) {
            let rest = reader.rest();
            let spaces = rest
                .iter()
                .take_while(|character| is_space(**character))
                .count();
            if spaces > 0 && rest.get(spaces) == Some(&'[') {
                // The response code ends at the bracket that closes the first one.
                let mut nesting = 1;
                let mut close = None;
                for (index, character) in rest.iter().enumerate().skip(spaces + 1) {
                    match character {
                        '[' => nesting += 1,
                        ']' => nesting -= 1,
                        _ => {}
                    }
                    if nesting == 0 {
                        close = Some(index);
                        break;
                    }
                }
                let close = close.or_else(|| {
                    rest.iter()
                        .skip(spaces + 1)
                        .position(|character| *character == ']')
                        .map(|index| index + spaces + 1)
                });
                if let Some(close) = close {
                    human_readable = Some(trimmed(&rest[close + 1..]));
                    end = reader.position + close + 1;
                }
            } else {
                human_readable = Some(trimmed(rest));
                end = reader.position;
            }
        }
        if ["UID", "AUTHENTICATE"].contains(&command.to_uppercase().as_str()) {
            reader.space(&tag).map_err(tagged)?;
            command = format!(
                "{command} {}",
                reader.element(is_command_char).map_err(tagged)?
            );
        }
    }

    let mut attributes = Vec::new();
    let remainder = &input[reader.position.min(end)..end];
    if !trimmed(remainder).is_empty() {
        // One space, then the attributes.
        match remainder.first() {
            Some(' ') => {}
            _ => return Err(tagged(error("ParserError5"))),
        }
        let remainder = &remainder[1..];
        if remainder
            .first()
            .is_some_and(|character| is_space(*character))
        {
            return Err(tagged(error("ParserError7")));
        }
        attributes = Tokenizer::new(remainder, &command, literals)
            .attributes()
            .map_err(tagged)?;
    }
    if let Some(text) = human_readable.filter(|text| !text.is_empty()) {
        attributes.push(Token::Text(text));
    }
    Ok(Response {
        tag,
        command,
        attributes,
    })
}

fn trimmed(characters: &[char]) -> String {
    let text: String = characters.iter().collect();
    mymcps_vine::js::trim(&text).to_owned()
}

struct LineReader<'a> {
    input: &'a [char],
    position: usize,
}

impl LineReader<'_> {
    fn rest(&self) -> &[char] {
        &self.input[self.position..]
    }

    /// `getElement`: the characters up to the next space, all of which must be allowed.
    fn element(&mut self, allowed: impl Fn(char) -> bool) -> Result<String, ParseError> {
        let rest = self.rest();
        if rest.first().is_some_and(|character| is_space(*character)) {
            return Err(error("ParserError1"));
        }
        let length = rest
            .iter()
            .take_while(|character| !is_space(**character))
            .count();
        if length == 0 {
            return Err(error("ParserError3"));
        }
        let element: String = rest[..length].iter().collect();
        if !element.chars().all(allowed) {
            return Err(error("ParserError2"));
        }
        self.position += length;
        Ok(element)
    }

    /// `getSpace`
    fn space(&mut self, tag: &str) -> Result<(), ParseError> {
        match self.rest().first() {
            None if tag == "+" && self.position == 1 => Ok(()),
            None => Err(error("ParserError4")),
            Some(' ') => {
                self.position += 1;
                Ok(())
            }
            Some(_) => Err(error("ParserError5")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Tree,
    Atom,
    String,
    List,
    Section,
    Partial,
    Literal,
    Sequence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Normal,
    Atom,
    String,
    Partial,
    Literal,
    Sequence,
}

struct Node {
    kind: Kind,
    value: String,
    literal: Option<Vec<u8>>,
    literal_length: Option<String>,
    is_closed: bool,
    depth: usize,
    parent: Option<usize>,
    children: Vec<usize>,
}

/// A response nests a few lists deep. More is not mail, and would overflow the stack.
const MAX_NODE_DEPTH: usize = 25;

/// `TokenParser`: reads the attributes of a response one character at a time.
struct Tokenizer<'a> {
    input: &'a [char],
    command: &'a str,
    literals: std::vec::IntoIter<Vec<u8>>,
    nodes: Vec<Node>,
    current: usize,
    state: State,
}

impl<'a> Tokenizer<'a> {
    fn new(input: &'a [char], command: &'a str, literals: Vec<Vec<u8>>) -> Self {
        let tree = Node {
            kind: Kind::Tree,
            value: String::new(),
            literal: None,
            literal_length: None,
            is_closed: true,
            depth: 0,
            parent: None,
            children: Vec::new(),
        };
        Self {
            input,
            command,
            literals: literals.into_iter(),
            nodes: vec![tree],
            current: 0,
            state: State::Normal,
        }
    }

    fn create(&mut self, parent: usize, kind: Kind) -> Result<usize, ParseError> {
        let depth = self.nodes[parent].depth + 1;
        if depth > MAX_NODE_DEPTH {
            return Err(error("MAX_IMAP_NESTING_REACHED"));
        }
        let index = self.nodes.len();
        self.nodes.push(Node {
            kind,
            value: String::new(),
            literal: None,
            literal_length: None,
            is_closed: true,
            depth,
            parent: Some(parent),
            children: Vec::new(),
        });
        self.nodes[parent].children.push(index);
        Ok(index)
    }

    fn parent(&self, node: usize) -> usize {
        self.nodes[node].parent.unwrap_or(0)
    }

    fn at(&self, index: usize) -> Option<char> {
        self.input.get(index).copied()
    }

    fn start_atom(&mut self, character: char) -> Result<(), ParseError> {
        if !is_atom_char(character)
            && character != '\\'
            && character != '%'
            && (character as u32) < 0x80
        {
            return Err(error("ParserError13"));
        }
        self.start_atom_unchecked(character)
    }

    fn start_atom_unchecked(&mut self, character: char) -> Result<(), ParseError> {
        self.current = self.create(self.current, Kind::Atom)?;
        self.nodes[self.current].value.push(character);
        self.state = State::Atom;
        Ok(())
    }

    /// Close the node being read and go back to its parent.
    fn close(&mut self) {
        self.nodes[self.current].is_closed = true;
        self.current = self.parent(self.current);
    }

    fn process(&mut self) -> Result<(), ParseError> {
        let length = self.input.len();
        let mut index = 0;
        let mut expects_literal8 = false;
        // `checkSP`
        let skip_spaces = |index: &mut usize, input: &[char]| {
            while input.get(*index + 1) == Some(&' ') {
                *index += 1;
            }
        };

        while index < length {
            let mut character = self.input[index];
            match self.state {
                State::Normal => match character {
                    '"' => {
                        self.current = self.create(self.current, Kind::String)?;
                        self.nodes[self.current].is_closed = false;
                        self.state = State::String;
                    }
                    '(' => {
                        self.current = self.create(self.current, Kind::List)?;
                        self.nodes[self.current].is_closed = false;
                    }
                    ')' => {
                        if self.nodes[self.current].kind != Kind::List {
                            return Err(error("ParserError10"));
                        }
                        self.close();
                        skip_spaces(&mut index, self.input);
                    }
                    ']' => {
                        if self.nodes[self.current].kind != Kind::Section {
                            return Err(error("ParserError11"));
                        }
                        self.close();
                        skip_spaces(&mut index, self.input);
                    }
                    '<' => {
                        if index == 0 || self.input[index - 1] != ']' {
                            self.start_atom_unchecked(character)?;
                        } else {
                            self.current = self.create(self.current, Kind::Partial)?;
                            self.nodes[self.current].is_closed = false;
                            self.state = State::Partial;
                        }
                    }
                    '~' => match self.at(index + 1) {
                        Some('{') => expects_literal8 = true,
                        Some(next) if is_atom_char(next) => self.start_atom_unchecked(character)?,
                        _ => return Err(error("ParserError12")),
                    },
                    '{' => {
                        self.current = self.create(self.current, Kind::Literal)?;
                        self.nodes[self.current].is_closed = false;
                        self.state = State::Literal;
                        // A literal8 is read like any other.
                        expects_literal8 = false;
                    }
                    '*' => {
                        self.current = self.create(self.current, Kind::Sequence)?;
                        self.nodes[self.current].value.push(character);
                        self.nodes[self.current].is_closed = false;
                        self.state = State::Sequence;
                    }
                    ' ' => {}
                    '[' if is_status(self.command) && self.current == 0 => {
                        // The response code of a status response: an atom without a name.
                        self.current = self.create(self.current, Kind::Atom)?;
                        self.current = self.create(self.current, Kind::Section)?;
                        self.nodes[self.current].is_closed = false;
                        let next: String = self.input[index + 1..].iter().take(9).collect();
                        if next.to_uppercase() == "REFERRAL " {
                            // What follows is a URL, which has characters of its own.
                            let name = self.create(self.current, Kind::Atom)?;
                            self.nodes[name].value = "REFERRAL".to_owned();
                            let url = self.create(self.current, Kind::Atom)?;
                            let start = index + 10;
                            let mut depth = 0;
                            index = start;
                            while index < length {
                                match self.input[index] {
                                    '[' => depth += 1,
                                    ']' => {
                                        if depth == 0 {
                                            break;
                                        }
                                        depth -= 1;
                                    }
                                    _ => {}
                                }
                                index += 1;
                            }
                            self.nodes[url].value =
                                self.input[start.min(index)..index].iter().collect();
                            self.close();
                            skip_spaces(&mut index, self.input);
                        }
                    }
                    _ => self.start_atom(character)?,
                },
                State::Atom => 'atom: {
                    if character == ' ' {
                        self.current = self.parent(self.current);
                        self.state = State::Normal;
                        break 'atom;
                    }
                    let parent = self.parent(self.current);
                    let parent_kind = self.nodes[parent].kind;
                    if (character == ')' && parent_kind == Kind::List)
                        || (character == ']' && parent_kind == Kind::Section)
                    {
                        self.current = parent;
                        self.close();
                        self.state = State::Normal;
                        skip_spaces(&mut index, self.input);
                        break 'atom;
                    }
                    if character == '\\' {
                        // Flags written without a space between them.
                        let value = &self.nodes[self.current].value;
                        let next = self.at(index + 1);
                        if parent_kind == Kind::List
                            && value.chars().count() > 1
                            && value.starts_with('\\')
                            && next.is_some_and(|next| is_atom_char(next) || next == '*')
                        {
                            self.current = parent;
                            self.start_atom(character)?;
                            break 'atom;
                        }
                    }
                    let is_number = {
                        let value = &self.nodes[self.current].value;
                        !value.is_empty() && value.chars().all(|digit| digit.is_ascii_digit())
                    };
                    if (character == ',' || character == ':') && is_number {
                        self.nodes[self.current].kind = Kind::Sequence;
                        self.nodes[self.current].is_closed = true;
                        self.state = State::Sequence;
                    }
                    if character == '['
                        && ["BODY", "BODY.PEEK", "BINARY", "BINARY.PEEK"]
                            .contains(&self.nodes[self.current].value.to_uppercase().as_str())
                    {
                        self.current = self.create(parent, Kind::Section)?;
                        self.nodes[self.current].is_closed = false;
                        self.state = State::Normal;
                        break 'atom;
                    }
                    let value = &self.nodes[self.current].value;
                    if !is_atom_char(character)
                        && (character as u32) < 0x80
                        && character != ']'
                        && !(character == '*' && value == "\\")
                        && !["NO", "BAD", "OK"].contains(&self.command)
                    {
                        return Err(error("ParserError16"));
                    }
                    if value == "\\*" {
                        return Err(error("ParserError17"));
                    }
                    self.nodes[self.current].value.push(character);
                }
                State::String => {
                    if character == '"' {
                        self.close();
                        self.state = State::Normal;
                        skip_spaces(&mut index, self.input);
                    } else {
                        if character == '\\' {
                            index += 1;
                            character = self.at(index).ok_or_else(|| error("ParserError18"))?;
                        }
                        self.nodes[self.current].value.push(character);
                    }
                }
                State::Partial => {
                    let value = &self.nodes[self.current].value;
                    if character == '>' {
                        if value.ends_with('.') {
                            return Err(error("ParserError19"));
                        }
                        self.close();
                        self.state = State::Normal;
                        skip_spaces(&mut index, self.input);
                    } else {
                        if character == '.' && (value.is_empty() || value.contains('.')) {
                            return Err(error("ParserError20"));
                        }
                        if !character.is_ascii_digit() && character != '.' {
                            return Err(error("ParserError21"));
                        }
                        if (value == "0" || value.ends_with(".0")) && character != '.' {
                            return Err(error("ParserError22"));
                        }
                        self.nodes[self.current].value.push(character);
                    }
                }
                State::Literal => {
                    if character == '}' {
                        let Some(announced) = self.nodes[self.current].literal_length.clone()
                        else {
                            return Err(error("ParserError23"));
                        };
                        if self.at(index + 1) == Some('\n') {
                            index += 1;
                        } else if self.at(index + 1) == Some('\r')
                            && self.at(index + 2) == Some('\n')
                        {
                            index += 2;
                        } else {
                            return Err(error("ParserError24"));
                        }
                        // An empty literal may come without its bytes.
                        let literal = self.literals.next();
                        if announced != "0" && literal.is_none() {
                            return Err(error("ParserError9"));
                        }
                        self.nodes[self.current].literal = Some(literal.unwrap_or_default());
                        self.close();
                        self.state = State::Normal;
                        skip_spaces(&mut index, self.input);
                    } else {
                        if !character.is_ascii_digit() {
                            return Err(error("ParserError25"));
                        }
                        let announced = self.nodes[self.current]
                            .literal_length
                            .get_or_insert_with(String::new);
                        if announced == "0" {
                            return Err(error("ParserError26"));
                        }
                        announced.push(character);
                    }
                }
                State::Sequence => 'sequence: {
                    let value = &self.nodes[self.current].value;
                    let last = value.chars().next_back();
                    let before_last = value.chars().rev().nth(1);
                    let ends_with_number_or_star =
                        last.is_some_and(|last| last.is_ascii_digit() || last == '*');
                    if character == ' ' {
                        if !ends_with_number_or_star {
                            return Err(error("ParserError27"));
                        }
                        if value != "*" && last == Some('*') && before_last != Some(':') {
                            return Err(error("ParserError28"));
                        }
                        self.close();
                        self.state = State::Normal;
                        break 'sequence;
                    }
                    let parent = self.parent(self.current);
                    if character == ']' && self.nodes[parent].kind == Kind::Section {
                        self.current = parent;
                        self.close();
                        self.state = State::Normal;
                        skip_spaces(&mut index, self.input);
                        break 'sequence;
                    }
                    match character {
                        ':' => {
                            if !ends_with_number_or_star {
                                return Err(error("ParserError29"));
                            }
                        }
                        '*' => {
                            if !matches!(last, Some(',' | ':')) {
                                return Err(error("ParserError30"));
                            }
                        }
                        ',' => {
                            if !ends_with_number_or_star {
                                return Err(error("ParserError31"));
                            }
                            if last == Some('*') && before_last != Some(':') {
                                return Err(error("ParserError32"));
                            }
                        }
                        digit if digit.is_ascii_digit() => {
                            if last == Some('*') {
                                return Err(error("ParserError34"));
                            }
                        }
                        _ => return Err(error("ParserError33")),
                    }
                    self.nodes[self.current].value.push(character);
                }
            }
            index += 1;
        }
        let _ = expects_literal8;
        Ok(())
    }

    /// `getAttributes`: the tree of nodes as tokens.
    fn attributes(mut self) -> Result<Vec<Token>, ParseError> {
        self.process()?;
        let mut attributes = Vec::new();
        self.walk(0, &mut attributes)?;
        Ok(attributes)
    }

    fn walk(&mut self, index: usize, branch: &mut Vec<Token>) -> Result<(), ParseError> {
        let node = &mut self.nodes[index];
        if !node.is_closed && node.kind == Kind::Sequence && node.value == "*" {
            node.is_closed = true;
            node.kind = Kind::Atom;
        }
        if !node.is_closed {
            return Err(error("ParserError9"));
        }
        let kind = node.kind;
        let children = std::mem::take(&mut node.children);
        let value = std::mem::take(&mut node.value);
        let literal = node.literal.take();

        match kind {
            Kind::Tree => {}
            Kind::Literal => branch.push(Token::Literal(literal.unwrap_or_default())),
            Kind::String => branch.push(Token::String(value)),
            Kind::Sequence => branch.push(Token::Sequence(value)),
            Kind::Atom if value.eq_ignore_ascii_case("NIL") => branch.push(Token::Nil),
            Kind::Atom => branch.push(Token::Atom {
                value,
                section: None,
                partial: None,
            }),
            Kind::List => {
                let mut items = Vec::new();
                for child in children {
                    self.walk(child, &mut items)?;
                }
                branch.push(Token::List(items));
                return Ok(());
            }
            Kind::Section => {
                let mut items = Vec::new();
                for child in children {
                    self.walk(child, &mut items)?;
                }
                return match branch.last_mut() {
                    Some(Token::Atom { section, .. }) => {
                        *section = Some(items);
                        Ok(())
                    }
                    _ => Err(error("ParserError11")),
                };
            }
            Kind::Partial => {
                return match branch.last_mut() {
                    Some(Token::Atom { partial, .. }) => {
                        *partial = Some(
                            value
                                .split('.')
                                .map(|number| number.parse().unwrap_or(0))
                                .collect(),
                        );
                        Ok(())
                    }
                    _ => Err(error("ParserError21")),
                };
            }
        }
        for child in children {
            self.walk(child, branch)?;
        }
        Ok(())
    }
}

/// One argument of a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Arg {
    /// Written as it is when it can be, quoted otherwise.
    Atom(String),
    /// An atom followed by a section and, maybe, a range: `BODY.PEEK[1.2]<0.65536>`.
    Section {
        name: String,
        section: Vec<Arg>,
        partial: Option<Vec<u64>>,
    },
    /// Always quoted.
    String(String),
    /// A set of message numbers, written as it is once checked.
    Sequence(String),
    Literal {
        data: Vec<u8>,
        is_literal8: bool,
    },
    List(Vec<Arg>),
}

impl Arg {
    pub(crate) fn atom(value: impl Into<String>) -> Self {
        Arg::Atom(value.into())
    }
}

/// Why a command cannot be written. Nothing was sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CompileError {
    #[error("Unquotable character in IMAP string value")]
    UnquotableString,
    #[error("Line terminator in IMAP token")]
    LineBreakInToken,
    #[error("Invalid sequence set value")]
    MalformedSequenceSet,
}

fn quote_string(value: &str) -> Result<String, CompileError> {
    if value.contains(['\r', '\n', '\0']) {
        return Err(CompileError::UnquotableString);
    }
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        if character == '"' || character == '\\' {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    Ok(quoted)
}

/// `/^(\d+|\*)(:(\d+|\*))?$/` for each part between commas, or `$`.
fn is_valid_sequence_set(value: &str) -> bool {
    let is_number = |text: &str| {
        text == "*" || (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
    };
    value == "$"
        || value.split(',').all(|part| match part.split_once(':') {
            Some((first, last)) => is_number(first) && is_number(last),
            None => is_number(part),
        })
}

/// Literals of this size or less are sent without waiting for the server,
/// when it takes them that way.
const MAX_NON_SYNCHRONIZING_LITERAL: usize = 4096;

struct Compiler {
    parts: Vec<Vec<u8>>,
    current: Vec<u8>,
    after_literal: bool,
    literal_minus: bool,
}

impl Compiler {
    fn text(&mut self, text: &str) -> Result<(), CompileError> {
        if text.contains(['\r', '\n']) {
            return Err(CompileError::LineBreakInToken);
        }
        self.current.extend_from_slice(text.as_bytes());
        Ok(())
    }

    fn walk(&mut self, argument: &Arg, in_sub_array: bool) -> Result<(), CompileError> {
        let needs_space =
            self.after_literal || !matches!(self.current.last(), None | Some(b'(' | b'<' | b'['));
        if needs_space && !in_sub_array {
            self.current.push(b' ');
        }
        self.after_literal = false;

        match argument {
            Arg::List(items) => {
                self.current.push(b'(');
                let mut sub_array = items.len() > 1 && matches!(items.first(), Some(Arg::List(_)));
                for item in items {
                    if sub_array && !matches!(item, Arg::List(_)) {
                        sub_array = false;
                    }
                    self.walk(item, sub_array)?;
                }
                self.current.push(b')');
            }
            Arg::Literal { data, is_literal8 } => {
                let is_non_synchronizing =
                    self.literal_minus && data.len() <= MAX_NON_SYNCHRONIZING_LITERAL;
                let marker = format!(
                    "{}{{{}{}}}\r\n",
                    if *is_literal8 { "~" } else { "" },
                    data.len(),
                    if is_non_synchronizing { "+" } else { "" }
                );
                self.current.extend_from_slice(marker.as_bytes());
                if !is_non_synchronizing {
                    // The rest is sent once the server asks for it.
                    self.parts.push(std::mem::take(&mut self.current));
                }
                self.current.extend_from_slice(data);
                self.after_literal = true;
            }
            Arg::String(value) => {
                let quoted = quote_string(value)?;
                self.text(&quoted)?;
            }
            Arg::Sequence(value) => {
                if !value.is_empty() && !is_valid_sequence_set(value) {
                    return Err(CompileError::MalformedSequenceSet);
                }
                self.current.extend_from_slice(value.as_bytes());
            }
            Arg::Atom(value) => self.atom(value)?,
            Arg::Section {
                name,
                section,
                partial,
            } => {
                if !name.is_empty() {
                    self.atom(name)?;
                }
                self.current.push(b'[');
                for item in section {
                    self.walk(item, false)?;
                }
                self.current.push(b']');
                if let Some(partial) = partial {
                    let range: Vec<String> = partial.iter().map(u64::to_string).collect();
                    self.current
                        .extend_from_slice(format!("<{}>", range.join(".")).as_bytes());
                }
            }
        }
        Ok(())
    }

    fn atom(&mut self, value: &str) -> Result<(), CompileError> {
        let checked = value.strip_prefix('\\').unwrap_or(value);
        if value.is_empty() || !checked.chars().all(is_atom_char) {
            let quoted = quote_string(value)?;
            self.text(&quoted)
        } else {
            self.text(value)
        }
    }
}

/// Write a command as the pieces to send: all but the last end where the
/// server must agree to take a literal before the next is sent. The last
/// piece ends the command and comes with its line break.
pub(crate) fn compile(
    tag: &str,
    command: &str,
    arguments: &[Arg],
    literal_minus: bool,
) -> Result<Vec<Vec<u8>>, CompileError> {
    let mut compiler = Compiler {
        parts: Vec::new(),
        current: Vec::new(),
        after_literal: false,
        literal_minus,
    };
    compiler.text(tag)?;
    compiler.current.push(b' ');
    compiler.text(command)?;
    for argument in arguments {
        compiler.walk(argument, false)?;
    }
    compiler.current.extend_from_slice(b"\r\n");
    compiler.parts.push(compiler.current);
    Ok(compiler.parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atom(value: &str) -> Token {
        Token::Atom {
            value: value.into(),
            section: None,
            partial: None,
        }
    }

    fn parse(line: &str) -> Response {
        parse_response(line.as_bytes(), Vec::new()).unwrap()
    }

    #[test]
    fn reads_status_responses_with_their_code_and_text() {
        let greeting = parse("* OK [CAPABILITY IMAP4rev1 AUTH=PLAIN] iCloud ready");
        assert_eq!(
            (greeting.tag.as_str(), greeting.command.as_str()),
            ("*", "OK")
        );
        assert_eq!(greeting.code_name().as_deref(), Some("CAPABILITY"));
        assert_eq!(
            greeting.code().unwrap(),
            [atom("CAPABILITY"), atom("IMAP4rev1"), atom("AUTH=PLAIN")]
        );
        assert_eq!(greeting.text(), "iCloud ready");

        let refused = parse("A3 NO [AUTHENTICATIONFAILED] Authentication failed.");
        assert_eq!(
            (refused.tag.as_str(), refused.command.as_str()),
            ("A3", "NO")
        );
        assert_eq!(refused.code_name().as_deref(), Some("AUTHENTICATIONFAILED"));
        assert_eq!(refused.text(), "Authentication failed.");

        let plain = parse("7 BAD Could not parse command");
        assert_eq!(plain.code(), None);
        assert_eq!(plain.text(), "Could not parse command");

        let flags = parse(r"* OK [PERMANENTFLAGS (\Deleted \Seen \*)] Flags permitted.");
        assert_eq!(
            flags.code().unwrap(),
            [
                atom("PERMANENTFLAGS"),
                Token::List(vec![atom("\\Deleted"), atom("\\Seen"), atom("\\*")])
            ]
        );

        let appended = parse("5 OK [APPENDUID 1700000000 101] APPEND completed");
        assert_eq!(
            appended.code().unwrap(),
            [atom("APPENDUID"), atom("1700000000"), atom("101")]
        );
        let moved = parse("* OK [COPYUID 7 12,14 1:2] Moved");
        assert_eq!(
            moved.code().unwrap(),
            [
                atom("COPYUID"),
                atom("7"),
                Token::Sequence("12,14".into()),
                Token::Sequence("1:2".into())
            ]
        );

        let continuation = parse("+ go ahead");
        assert_eq!(
            (continuation.tag.as_str(), continuation.command.as_str()),
            ("+", "")
        );
        assert_eq!(continuation.text(), "go ahead");
        assert_eq!(parse("+").attributes, []);
    }

    #[test]
    fn reads_lists_strings_literals_and_sections() {
        let listed = parse(r#"* LIST (\HasNoChildren \Sent) "/" "Sent Messages""#);
        assert_eq!(listed.command, "LIST");
        assert_eq!(
            listed.attributes,
            [
                Token::List(vec![atom("\\HasNoChildren"), atom("\\Sent")]),
                Token::String("/".into()),
                Token::String("Sent Messages".into()),
            ]
        );

        let escaped = parse(r#"* LIST () NIL "a \"quoted\" \\ name""#);
        assert_eq!(
            escaped.attributes,
            [
                Token::List(vec![]),
                Token::Nil,
                Token::String(r#"a "quoted" \ name"#.into())
            ]
        );

        let fetched = parse_response(
            b"* 3 FETCH (UID 14 FLAGS (\\Seen\\Flagged) BODY[1.2]<0> {5}\r\n BODY[HEADER.FIELDS (References)] {0}\r\n)",
            vec![b"hello".to_vec()],
        )
        .unwrap();
        assert_eq!(fetched.command, "3");
        assert_eq!(
            fetched.attributes,
            [
                atom("FETCH"),
                Token::List(vec![
                    atom("UID"),
                    atom("14"),
                    atom("FLAGS"),
                    Token::List(vec![atom("\\Seen"), atom("\\Flagged")]),
                    Token::Atom {
                        value: "BODY".into(),
                        section: Some(vec![atom("1.2")]),
                        partial: Some(vec![0])
                    },
                    Token::Literal(b"hello".to_vec()),
                    Token::Atom {
                        value: "BODY".into(),
                        section: Some(vec![
                            atom("HEADER.FIELDS"),
                            Token::List(vec![atom("References")])
                        ]),
                        partial: None,
                    },
                    Token::Literal(Vec::new()),
                ]),
            ]
        );

        let found = parse("* SEARCH 11 14");
        assert_eq!(found.attributes, [atom("11"), atom("14")]);
        let counted = parse("* 3 EXISTS");
        assert_eq!(
            (counted.command.as_str(), counted.attributes),
            ("3", vec![atom("EXISTS")])
        );
    }

    #[test]
    fn reads_bytes_that_are_not_utf8_without_failing() {
        let response = parse_response(b"* LIST () \"/\" \"Entw\xfcrfe\"", Vec::new()).unwrap();
        assert_eq!(
            response.attributes[2],
            Token::String("Entw\u{fffd}rfe".into())
        );
        assert_eq!(
            parse_response(b"\0\0* 1 EXISTS", Vec::new())
                .unwrap()
                .command,
            "1"
        );
        assert_eq!(parse_response(b"\0\0", Vec::new()).unwrap().command, "BAD");
    }

    #[test]
    fn refuses_what_is_not_a_response() {
        let code = |line: &[u8]| parse_response(line, Vec::new()).unwrap_err().code;
        assert_eq!(code(b"* LIST (\\Sent \"/\" x"), "ParserError9");
        assert_eq!(code(b"* LIST ) x"), "ParserError10");
        assert_eq!(code(b"* SEARCH \"unterminated"), "ParserError9");
        assert_eq!(code(b" * OK"), "ParserError1");
        assert_eq!(code(b"*"), "ParserError4");
        let nested = format!("* X {}", "(".repeat(40));
        assert_eq!(code(nested.as_bytes()), "MAX_IMAP_NESTING_REACHED");
        // The tag is kept, so that the command it answers can be failed.
        assert_eq!(
            parse_response(b"A7 OK [x", Vec::new())
                .unwrap_err()
                .tag
                .as_deref(),
            Some("A7")
        );
    }

    #[test]
    fn writes_atoms_strings_sections_and_literals() {
        let written = |arguments: &[Arg], literal_minus: bool| {
            compile("A1", "UID FETCH", arguments, literal_minus)
                .unwrap()
                .into_iter()
                .map(|part| String::from_utf8(part).unwrap())
                .collect::<Vec<_>>()
        };

        assert_eq!(
            written(
                &[
                    Arg::Sequence("11,14".into()),
                    Arg::List(vec![
                        Arg::atom("UID"),
                        Arg::Section {
                            name: "BODY.PEEK".into(),
                            section: vec![
                                Arg::atom("HEADER.FIELDS"),
                                Arg::List(vec![Arg::atom("references")])
                            ],
                            partial: None,
                        },
                        Arg::Section {
                            name: "BODY.PEEK".into(),
                            section: vec![Arg::atom("1.2")],
                            partial: Some(vec![0, 65536])
                        },
                    ]),
                ],
                false
            ),
            [
                "A1 UID FETCH 11,14 (UID BODY.PEEK[HEADER.FIELDS (references)] BODY.PEEK[1.2]<0.65536>)\r\n"
            ]
        );
        assert_eq!(
            written(
                &[
                    Arg::atom("Sent Messages"),
                    Arg::atom("INBOX"),
                    Arg::atom(""),
                    Arg::atom("\\Seen"),
                    Arg::String("a\"b\\".into())
                ],
                false
            ),
            ["A1 UID FETCH \"Sent Messages\" INBOX \"\" \\Seen \"a\\\"b\\\\\"\r\n"]
        );
        // A long literal waits for the server. A short one does not, when the server allows it.
        assert_eq!(
            written(
                &[
                    Arg::atom("Drafts"),
                    Arg::Literal {
                        data: b"Subject: x\r\n\r\nHi".to_vec(),
                        is_literal8: false
                    }
                ],
                false
            ),
            ["A1 UID FETCH Drafts {16}\r\n", "Subject: x\r\n\r\nHi\r\n"]
        );
        assert_eq!(
            written(
                &[
                    Arg::atom("FROM"),
                    Arg::Literal {
                        data: "André".as_bytes().to_vec(),
                        is_literal8: false
                    },
                    Arg::atom("UNSEEN")
                ],
                true
            ),
            ["A1 UID FETCH FROM {6+}\r\nAndré UNSEEN\r\n"]
        );

        let refused = |argument: Arg| compile("A1", "SELECT", &[argument], false).unwrap_err();
        assert_eq!(
            refused(Arg::atom("INBOX\r\nA2 DELETE INBOX")),
            CompileError::UnquotableString
        );
        assert_eq!(
            refused(Arg::String("a\0b".into())),
            CompileError::UnquotableString
        );
        assert_eq!(
            refused(Arg::Sequence("1:2 3".into())),
            CompileError::MalformedSequenceSet
        );
    }
}
