//! The connector's allocation-free scans of response bodies, fed chunk by
//! chunk (`docs/plans/batch.md` 2.9): the LIST page's tag scan, which
//! bounds what `object_store`'s parse of a page can hold, and the `<Code>`
//! scan, which records the S3 error code a body names at any status.

/// Markup (a tag, comment, CDATA section, declaration or processing
/// instruction) and a text run each stay within this many bytes.
pub const MARKUP_MAX: u32 = 6 * 1_024;
/// The most start tags of a page: 16 per entry and 64 more.
pub const STARTS_MAX: u32 = 16 * 1_000 + 64;
/// The most `<CommonPrefixes>` of a page.
pub const PREFIXES_MAX: u32 = 1_000;
/// The most start tags after the first `<Contents>` outside a `<Contents>`
/// or a `<CommonPrefixes>`.
pub const AFTER_MAX: u32 = 64;
/// The deepest nesting.
pub const DEPTH_MAX: u8 = 8;
const NAME: usize = 16;

/// A page breaks a limit: its try fails as a 2xx the call returns as an
/// error (`BATCH_SPEC.md` 1.6).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Breach;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Text,
    /// Just after `<`.
    Open,
    /// After `<!`, `matched` bytes of `--` or `[CDATA[` seen.
    Bang {
        dash: bool,
        matched: u8,
    },
    Start,
    Attributes,
    End,
    Comment,
    Cdata,
    Declaration,
    Instruction,
    /// A limit broke; every later byte breaks it too.
    Broken,
}

/// The tag scan's state: counters, a quote byte and a 16-byte name buffer.
/// An empty element `<x/>` counts as a start tag and adds no depth; a
/// quoted attribute value may hold `>`; a text run is the character data
/// between two tags, CDATA included, which comments, declarations and
/// processing instructions do not end, as quick-xml merges it.
#[derive(Clone, Debug)]
pub struct TagScan {
    state: State,
    max_keys: u32,
    markup: u32,
    run: u32,
    depth: u8,
    starts: u32,
    contents: u32,
    prefixes: u32,
    after: u32,
    seen_contents: bool,
    /// The depth of the `<Contents>` or `<CommonPrefixes>` the scan is in.
    inside: Option<u8>,
    name: [u8; NAME],
    name_len: usize,
    name_long: bool,
    quote: u8,
    slash: bool,
    /// Consecutive `-` or `]` before a possible end of a comment or CDATA,
    /// or whether the last byte was `?` in an instruction.
    tail: u8,
    brackets: u32,
}

impl TagScan {
    /// A scan of a page of at most `max_keys` `<Contents>`.
    #[must_use]
    pub const fn new(max_keys: u32) -> Self {
        Self {
            state: State::Text,
            max_keys,
            markup: 0,
            run: 0,
            depth: 0,
            starts: 0,
            contents: 0,
            prefixes: 0,
            after: 0,
            seen_contents: false,
            inside: None,
            name: [0; NAME],
            name_len: 0,
            name_long: false,
            quote: 0,
            slash: false,
            tail: 0,
            brackets: 0,
        }
    }

    /// The next chunk of the body. A breach is final: every later chunk
    /// breaks it too.
    pub fn feed(&mut self, chunk: &[u8]) -> Result<(), Breach> {
        for &b in chunk {
            if self.state == State::Broken || self.byte(b).is_err() {
                self.state = State::Broken;
                return Err(Breach);
            }
        }
        Ok(())
    }

    fn byte(&mut self, b: u8) -> Result<(), Breach> {
        if self.state == State::Text {
            if b == b'<' {
                self.state = State::Open;
                self.markup = 1;
            } else {
                self.run = grow(self.run)?;
            }
            return Ok(());
        }
        self.markup = grow(self.markup)?;
        match self.state {
            State::Open => match b {
                b'/' => {
                    self.run = 0;
                    self.state = State::End;
                }
                b'!' => {
                    self.state = State::Bang {
                        dash: false,
                        matched: 0,
                    };
                }
                b'?' => {
                    self.tail = 0;
                    self.state = State::Instruction;
                }
                _ => {
                    self.run = 0;
                    self.name_len = 0;
                    self.name_long = false;
                    self.slash = false;
                    self.quote = 0;
                    self.state = State::Start;
                    return self.start_name(b);
                }
            },
            State::Bang { dash, matched } => self.bang(b, dash, matched)?,
            State::Start => return self.start_name(b),
            State::Attributes => return self.attributes(b),
            State::End => {
                if b == b'>' {
                    return self.close();
                }
            }
            State::Comment => self.terminator(b, b'-'),
            State::Cdata => {
                // A CDATA section's text belongs to the run around it.
                self.run = grow(self.run)?;
                self.terminator(b, b']');
            }
            State::Declaration => match b {
                b'[' => self.brackets = self.brackets.saturating_add(1),
                b']' => self.brackets = self.brackets.saturating_sub(1),
                b'>' if self.brackets == 0 => self.state = State::Text,
                _ => {}
            },
            State::Instruction => {
                if b == b'>' && self.tail == 1 {
                    self.state = State::Text;
                }
                self.tail = u8::from(b == b'?');
            }
            State::Text | State::Broken => {}
        }
        Ok(())
    }

    fn bang(&mut self, b: u8, dash: bool, matched: u8) -> Result<(), Breach> {
        const CDATA: &[u8] = b"[CDATA[";
        if matched == 0 && b == b'-' {
            self.state = State::Bang {
                dash: true,
                matched: 1,
            };
        } else if dash && b == b'-' {
            self.tail = 0;
            self.state = State::Comment;
        } else if !dash && CDATA.get(usize::from(matched)) == Some(&b) {
            if usize::from(matched) + 1 == CDATA.len() {
                self.tail = 0;
                self.state = State::Cdata;
            } else {
                self.state = State::Bang {
                    dash: false,
                    matched: matched + 1,
                };
            }
        } else {
            // Any other `<!…>`: a declaration up to its `>` outside brackets.
            self.brackets = 0;
            self.state = State::Declaration;
            return self.byte_again(b);
        }
        Ok(())
    }

    /// Re-reads `b` in the state just entered, its markup byte counted once.
    fn byte_again(&mut self, b: u8) -> Result<(), Breach> {
        self.markup -= 1;
        self.byte(b)
    }

    /// `-->` ends a comment, `]]>` a CDATA section.
    fn terminator(&mut self, b: u8, mark: u8) {
        if b == b'>' && self.tail >= 2 {
            self.state = State::Text;
        } else if b == mark {
            self.tail = self.tail.saturating_add(1);
        } else {
            self.tail = 0;
        }
    }

    fn start_name(&mut self, b: u8) -> Result<(), Breach> {
        match b {
            b' ' | b'\t' | b'\r' | b'\n' => self.state = State::Attributes,
            b'/' => {
                self.state = State::Attributes;
                self.slash = true;
            }
            b'>' => return self.open(),
            b':' => {
                // The local name decides, as a namespace prefix does not.
                self.name_len = 0;
                self.name_long = false;
            }
            _ => match self.name.get_mut(self.name_len) {
                Some(slot) => {
                    *slot = b;
                    self.name_len += 1;
                }
                None => self.name_long = true,
            },
        }
        Ok(())
    }

    fn attributes(&mut self, b: u8) -> Result<(), Breach> {
        if self.quote != 0 {
            if b == self.quote {
                self.quote = 0;
            }
            self.slash = false;
            return Ok(());
        }
        match b {
            b'"' | b'\'' => {
                self.quote = b;
                self.slash = false;
            }
            b'>' => return self.open(),
            b'/' => self.slash = true,
            b' ' | b'\t' | b'\r' | b'\n' => {}
            _ => self.slash = false,
        }
        Ok(())
    }

    fn named(&self, name: &[u8]) -> bool {
        !self.name_long && self.name.get(..self.name_len) == Some(name)
    }

    /// A start tag (or an empty element) ends.
    fn open(&mut self) -> Result<(), Breach> {
        self.state = State::Text;
        self.starts += 1;
        if self.starts > STARTS_MAX {
            return Err(Breach);
        }
        let empty = self.slash;
        if !empty {
            self.depth += 1;
            if self.depth > DEPTH_MAX {
                return Err(Breach);
            }
        }
        if self.inside.is_none() {
            if self.named(b"Contents") {
                self.contents += 1;
                self.seen_contents = true;
                if self.contents > self.max_keys {
                    return Err(Breach);
                }
                self.inside = (!empty).then_some(self.depth);
            } else if self.named(b"CommonPrefixes") {
                self.prefixes += 1;
                if self.prefixes > PREFIXES_MAX {
                    return Err(Breach);
                }
                self.inside = (!empty).then_some(self.depth);
            } else if self.seen_contents {
                self.after += 1;
                if self.after > AFTER_MAX {
                    return Err(Breach);
                }
            }
        }
        Ok(())
    }

    /// An end tag ends.
    fn close(&mut self) -> Result<(), Breach> {
        self.state = State::Text;
        if self.depth == 0 {
            return Err(Breach);
        }
        if self.inside == Some(self.depth) {
            self.inside = None;
        }
        self.depth -= 1;
        Ok(())
    }
}

fn grow(n: u32) -> Result<u32, Breach> {
    let n = n + 1;
    if n > MARKUP_MAX { Err(Breach) } else { Ok(n) }
}

/// The longest error code the engine keeps; a longer one names nothing.
pub const CODE_MAX: usize = 32;

/// The text of a body's `<Error><Code>` element, at most [`CODE_MAX`] bytes.
#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub struct Code {
    bytes: [u8; CODE_MAX],
    len: u8,
}

/// The codes the engine acts on (`BATCH_SPEC.md` 1.6's table, and the
/// 2xx words `object_store` retries on), which alone render.
const NAMED: [&str; 4] = ["NoSuchUpload", "ExpiredToken", "InternalError", "SlowDown"];

/// A code the table names renders as itself; any other, as its length, as
/// a server can put anything there.
impl std::fmt::Debug for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match NAMED.iter().find(|name| self.is(name)) {
            Some(name) => write!(f, "Code({name})"),
            None => write!(f, "Code(<{} bytes>)", self.len),
        }
    }
}

impl Code {
    /// Whether the body named no code.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Whether the body named exactly this code.
    #[must_use]
    pub fn is(&self, name: &str) -> bool {
        self.bytes.get(..usize::from(self.len)) == Some(name.as_bytes()) && self.len > 0
    }
}

/// What the code scan is inside.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum At {
    #[default]
    Text,
    /// After `<`.
    Open,
    /// After `<!`: `matched` bytes of `[CDATA[` so far.
    Bang { matched: u8 },
    /// After `<!-`.
    Dash,
    /// A start tag's name, then its attributes.
    Start {
        attributes: bool,
        quote: u8,
        slash: bool,
    },
    /// An end tag's name, then any space before its `>`.
    End { spaced: bool },
    /// A comment; `dashes` the run of `-` just read.
    Comment { dashes: u8 },
    /// A CDATA section; `brackets` the run of `]` just read.
    Cdata { brackets: u8 },
    /// A processing instruction; `question` whether `?` came last.
    Instruction { question: bool },
    /// Any other `<!…>`, with its quotes and brackets.
    Declaration { quote: u8, depth: u8 },
}

/// The longest element name the code scan compares.
const NAME_MAX: usize = 16;

/// Where the code scan is with the root `<Error>`'s `<Code>`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Capture {
    #[default]
    Idle,
    /// Inside it, its text so far plain.
    Capturing,
    /// Closed: the code is found, and the scan reads no further.
    Done,
}

/// Finds the S3 error code of a body (`<Error><Code>…</Code>`) across chunk
/// boundaries, with no allocation. It reads elements alone: a `<Code>` in a
/// comment, a CDATA section, an instruction or an attribute value is no
/// element, and a code counts only as the root `<Error>`'s child, holding
/// plain text and closed by its own `</Code>`.
#[derive(Clone, Copy, Debug, Default)]
pub struct CodeScan {
    at: At,
    name: [u8; NAME_MAX],
    name_len: u8,
    /// The name ran past [`NAME_MAX`]: it is neither name compared.
    name_long: bool,
    depth: u32,
    root_error: bool,
    capture: Capture,
    code: Code,
}

impl CodeScan {
    fn name(&self) -> &[u8] {
        if self.name_long {
            &[]
        } else {
            self.name
                .get(..usize::from(self.name_len))
                .unwrap_or_default()
        }
    }
    fn new_name(&mut self) {
        self.name_len = 0;
        self.name_long = false;
    }
    fn push_name(&mut self, b: u8) {
        if let Some(slot) = self.name.get_mut(usize::from(self.name_len)) {
            *slot = b;
            self.name_len += 1;
        } else {
            self.name_long = true;
        }
    }
    fn capturing(&self) -> bool {
        self.capture == Capture::Capturing
    }
    /// Markup inside a candidate code: it holds more than plain text.
    fn spoil(&mut self) {
        if self.capturing() {
            self.capture = Capture::Idle;
            self.code = Code::default();
        }
    }
    fn started(&mut self, empty: bool) {
        self.spoil();
        if empty {
            return;
        }
        self.depth = self.depth.saturating_add(1);
        if self.depth == 1 {
            self.root_error = self.name() == b"Error";
        } else if self.depth == 2 && self.root_error && self.name() == b"Code" {
            self.capture = Capture::Capturing;
            self.code = Code::default();
        }
    }
    fn ended(&mut self) {
        if self.capturing() && self.name() == b"Code" && self.code.len > 0 {
            self.capture = Capture::Done;
            return;
        }
        self.spoil();
        self.depth = self.depth.saturating_sub(1);
    }
    fn text(&mut self, b: u8) {
        if !self.capturing() {
            return;
        }
        // A code is plain text: an entity or an overlong one is no code.
        if b == b'&' {
            self.spoil();
        } else if let Some(slot) = self.code.bytes.get_mut(usize::from(self.code.len)) {
            *slot = b;
            self.code.len += 1;
        } else {
            self.spoil();
        }
    }

    /// After `<`.
    fn open(&mut self, b: u8) -> At {
        match b {
            b'!' => At::Bang { matched: 0 },
            b'?' => At::Instruction { question: false },
            b'/' => {
                self.new_name();
                At::End { spaced: false }
            }
            _ => {
                self.new_name();
                self.push_name(b);
                At::Start {
                    attributes: false,
                    quote: 0,
                    slash: false,
                }
            }
        }
    }

    /// In a start tag.
    fn start(&mut self, attributes: bool, quote: u8, slash: bool, b: u8) -> At {
        let next = |quote, slash| At::Start {
            attributes,
            quote,
            slash,
        };
        match b {
            _ if quote != 0 => next(if b == quote { 0 } else { quote }, false),
            b'>' => {
                self.started(slash);
                At::Text
            }
            b'"' | b'\'' if attributes => next(b, false),
            b'/' => next(0, true),
            b' ' | b'\t' | b'\r' | b'\n' => At::Start {
                attributes: true,
                quote: 0,
                slash: false,
            },
            _ => {
                if !attributes {
                    self.push_name(b);
                }
                next(0, false)
            }
        }
    }

    /// In an end tag.
    fn end(&mut self, spaced: bool, b: u8) -> At {
        match b {
            b'>' => {
                self.ended();
                At::Text
            }
            b' ' | b'\t' | b'\r' | b'\n' => At::End { spaced: true },
            _ => {
                if spaced {
                    // A name with space inside: no end tag of ours.
                    self.name_long = true;
                }
                self.push_name(b);
                At::End { spaced }
            }
        }
    }

    pub fn feed(&mut self, chunk: &[u8]) {
        const CDATA: &[u8] = b"[CDATA[";
        for &b in chunk {
            if self.capture == Capture::Done {
                return;
            }
            self.at = match self.at {
                At::Text if b == b'<' => At::Open,
                At::Text => {
                    self.text(b);
                    At::Text
                }
                At::Open => self.open(b),
                At::Bang { matched: 0 } if b == b'-' => At::Dash,
                At::Bang { matched } if CDATA.get(usize::from(matched)) == Some(&b) => {
                    if usize::from(matched) + 1 == CDATA.len() {
                        At::Cdata { brackets: 0 }
                    } else {
                        At::Bang {
                            matched: matched + 1,
                        }
                    }
                }
                At::Dash if b == b'-' => At::Comment { dashes: 0 },
                At::Bang { .. } | At::Dash => Self::declaration(0, 0, b),
                At::Start {
                    attributes,
                    quote,
                    slash,
                } => self.start(attributes, quote, slash, b),
                At::End { spaced } => self.end(spaced, b),
                At::Comment { dashes } => match b {
                    b'-' => At::Comment {
                        dashes: dashes.saturating_add(1),
                    },
                    b'>' if dashes >= 2 => At::Text,
                    _ => At::Comment { dashes: 0 },
                },
                At::Cdata { brackets } => match b {
                    b']' => At::Cdata {
                        brackets: brackets.saturating_add(1),
                    },
                    b'>' if brackets >= 2 => At::Text,
                    _ => At::Cdata { brackets: 0 },
                },
                At::Instruction { question } => match b {
                    b'>' if question => At::Text,
                    _ => At::Instruction {
                        question: b == b'?',
                    },
                },
                At::Declaration { quote, depth } => Self::declaration(quote, depth, b),
            };
            // A code holds plain text: no comment, CDATA or instruction.
            if matches!(
                self.at,
                At::Comment { .. }
                    | At::Cdata { .. }
                    | At::Instruction { .. }
                    | At::Declaration { .. }
            ) {
                self.spoil();
            }
        }
    }

    fn declaration(quote: u8, depth: u8, b: u8) -> At {
        match b {
            _ if quote != 0 => At::Declaration {
                quote: if b == quote { 0 } else { quote },
                depth,
            },
            b'"' | b'\'' => At::Declaration { quote: b, depth },
            b'[' => At::Declaration {
                quote: 0,
                depth: depth.saturating_add(1),
            },
            b']' => At::Declaration {
                quote: 0,
                depth: depth.saturating_sub(1),
            },
            b'>' if depth == 0 => At::Text,
            _ => At::Declaration { quote: 0, depth },
        }
    }

    /// The code, once its element closed; none before, or past [`CODE_MAX`].
    #[must_use]
    pub fn code(&self) -> Code {
        if self.capture == Capture::Done {
            self.code
        } else {
            Code::default()
        }
    }
}

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
