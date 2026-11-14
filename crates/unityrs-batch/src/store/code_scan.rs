//! The `<Code>` scan (`docs/plans/batch.md` 2.9). It reads a body's markup
//! as quick-xml 0.41's reader does, the one `object_store`'s parse uses: a
//! tag ends at a `>` outside quotes, its name at the first whitespace; a
//! comment at `-->`, a CDATA section at `]]>`, an instruction at `?>`; and a
//! `<!DOCTYPE …>` as its DTD parser reads it (`src/parser/dtd.rs`), past
//! quoted literals, and past the comments, instructions and declarations of
//! an internal subset, which ends at its first `]` outside them. Markup that
//! reader refuses (`<!x`, `<!-x`, `<![x`, a DOCTYPE with no name, `<?>`)
//! leaves the scan [`malformed`](CodeScan::malformed).

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

/// A body's root element, as the code scan read it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Root {
    /// No start tag has closed yet.
    #[default]
    None,
    /// `<Error>`: an S3 error, whatever the status.
    Error,
    /// Any other element.
    Other,
}

/// What follows `<` inside an internal subset, as quick-xml tells it apart.
const SUBSET_MARKUP: [&[u8]; 5] = [b"!--", b"!ELEMENT", b"!ENTITY", b"!ATTLIST", b"!NOTATION"];

/// The bytes after `<` in a subset the scan holds until it can tell.
const WINDOW: usize = 9;

/// A start tag so far.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Tag {
    /// The open quote, or 0.
    quote: u8,
    /// Whitespace has ended the name.
    named: bool,
    /// The byte just read was `/`.
    slash: bool,
}

/// What the code scan is inside.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum At {
    #[default]
    Text,
    /// After `<`.
    Open,
    /// After `<!`.
    Bang,
    /// After `<!-`.
    Dash,
    /// After `<![`: `matched` bytes of `CDATA[` so far.
    CdataOpen {
        matched: u8,
    },
    Start(Tag),
    /// An end tag after `</`; `spaced` once whitespace followed its name.
    End {
        quote: u8,
        spaced: bool,
    },
    /// A comment; `dashes` the run of `-` just read.
    Comment {
        dashes: u8,
    },
    /// A CDATA section; `brackets` the run of `]` just read.
    Cdata {
        brackets: u8,
    },
    /// An instruction after `<?`; `question` whether `?` came last, `empty`
    /// while nothing followed `<?`.
    Instruction {
        question: bool,
        empty: bool,
    },
    /// A DOCTYPE before its internal subset, with its open quote.
    Doctype {
        quote: u8,
    },
    /// Inside the internal subset.
    Subset,
    /// After `<` in the subset, the bytes read so far.
    SubsetMarkup {
        window: [u8; WINDOW],
        len: u8,
    },
    /// An instruction in the subset, after `<?`.
    SubsetInstruction {
        question: bool,
    },
    /// A comment in the subset.
    SubsetComment {
        dashes: u8,
    },
    /// `<!ELEMENT` or other markup in the subset: to the next `>`.
    SubsetDecl,
    /// `<!ENTITY`, `<!ATTLIST` or `<!NOTATION`: to a `>` outside quotes.
    SubsetQuoted {
        quote: u8,
    },
    /// After the subset's `]`: to the next `>`.
    AfterSubset,
}

impl At {
    /// A DOCTYPE's states.
    const fn in_doctype(self) -> bool {
        matches!(
            self,
            Self::Doctype { .. }
                | Self::Subset
                | Self::SubsetMarkup { .. }
                | Self::SubsetInstruction { .. }
                | Self::SubsetComment { .. }
                | Self::SubsetDecl
                | Self::SubsetQuoted { .. }
                | Self::AfterSubset
        )
    }
    /// Markup a code cannot hold: it holds plain text.
    const fn spoils(self) -> bool {
        matches!(
            self,
            Self::Comment { .. } | Self::Cdata { .. } | Self::Instruction { .. }
        ) || self.in_doctype()
    }
}

/// The longest element name the code scan compares.
const NAME_MAX: usize = 16;

/// quick-xml's whitespace (`src/utils.rs`).
const fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\r' | b'\n' | b'\t')
}

/// Where the code scan is with the root `<Error>`'s `<Code>`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Capture {
    #[default]
    Idle,
    /// Inside it, its text so far plain.
    Capturing,
    /// Closed: the code is found.
    Done,
}

/// Finds the S3 error code of a body (`<Error><Code>…</Code>`) across chunk
/// boundaries, with no allocation. It reads elements alone: a `<Code>` in a
/// comment, a CDATA section, an instruction, a DOCTYPE or an attribute
/// value is no element, and a code counts only as the root `<Error>`'s
/// child, holding plain text and closed by its own `</Code>`.
#[derive(Clone, Copy, Debug, Default)]
pub struct CodeScan {
    at: At,
    name: [u8; NAME_MAX],
    name_len: u8,
    /// The name ran past [`NAME_MAX`], or held a space: it is neither name
    /// compared.
    name_long: bool,
    depth: u32,
    root: Root,
    capture: Capture,
    code: Code,
    /// Bytes of `DOCTYPE` (any case) read after `<!`.
    keyword: u8,
    /// A DOCTYPE's name: anything but whitespace after its keyword.
    doctype_named: bool,
    /// Markup quick-xml's reader refuses.
    malformed: bool,
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
        if self.depth == 0 && self.root == Root::None {
            self.root = if self.name() == b"Error" {
                Root::Error
            } else {
                Root::Other
            };
        }
        if empty {
            return;
        }
        self.depth = self.depth.saturating_add(1);
        if self.depth == 2
            && self.root == Root::Error
            && self.name() == b"Code"
            && self.capture == Capture::Idle
        {
            self.capture = Capture::Capturing;
            self.code = Code::default();
        }
    }
    fn ended(&mut self) {
        if self.capturing() && self.name() == b"Code" && self.code.len > 0 {
            self.capture = Capture::Done;
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
            b'!' => At::Bang,
            b'?' => At::Instruction {
                question: true,
                empty: true,
            },
            b'/' => {
                self.new_name();
                At::End {
                    quote: 0,
                    spaced: false,
                }
            }
            _ => {
                self.new_name();
                self.start(Tag::default(), b)
            }
        }
    }

    /// After `<!`: quick-xml's `BangType`.
    fn bang(&mut self, b: u8) -> At {
        match b {
            b'-' => At::Dash,
            b'[' => At::CdataOpen { matched: 0 },
            b'D' | b'd' => {
                self.keyword = 1;
                self.doctype_named = false;
                At::Doctype { quote: 0 }
            }
            _ => self.refused(),
        }
    }

    /// Markup quick-xml's reader refuses: its parse fails there.
    fn refused(&mut self) -> At {
        self.malformed = true;
        At::Text
    }

    /// In a start tag: its name runs to the first whitespace, quotes and
    /// all, and it ends at a `>` outside quotes (`ElementParser`).
    fn start(&mut self, tag: Tag, b: u8) -> At {
        let mut tag = tag;
        if tag.quote == 0 && b == b'>' {
            let empty = tag.slash;
            if empty && !tag.named && !self.name_long {
                self.name_len = self.name_len.saturating_sub(1);
            }
            self.started(empty);
            return At::Text;
        }
        if !tag.named {
            if is_space(b) {
                tag.named = true;
            } else {
                self.push_name(b);
            }
        }
        if tag.quote != 0 {
            if b == tag.quote {
                tag.quote = 0;
            }
        } else if matches!(b, b'"' | b'\'') {
            tag.quote = b;
        }
        tag.slash = b == b'/';
        At::Start(tag)
    }

    /// In an end tag: its name is all of it, trailing whitespace trimmed.
    fn end(&mut self, quote: u8, spaced: bool, b: u8) -> At {
        if quote == 0 && b == b'>' {
            self.ended();
            return At::Text;
        }
        let mut spaced = spaced;
        if is_space(b) {
            spaced = true;
        } else {
            if spaced {
                // A name with space inside: no end tag of ours.
                self.name_long = true;
            }
            self.push_name(b);
        }
        let quote = match quote {
            0 if matches!(b, b'"' | b'\'') => b,
            q if q == b => 0,
            q => q,
        };
        At::End { quote, spaced }
    }

    /// In a DOCTYPE (`DtdParser`).
    fn doctype(&mut self, at: At, b: u8) -> At {
        let next = match at {
            At::Doctype { quote } if quote != 0 => At::Doctype {
                quote: if b == quote { 0 } else { quote },
            },
            At::Doctype { .. } => match b {
                b'"' | b'\'' => At::Doctype { quote: b },
                b'[' => At::Subset,
                b'>' => At::Text,
                _ => At::Doctype { quote: 0 },
            },
            At::Subset => match b {
                b']' => At::AfterSubset,
                b'<' => At::SubsetMarkup {
                    window: [0; WINDOW],
                    len: 0,
                },
                _ => At::Subset,
            },
            At::SubsetMarkup { window, len } => Self::subset_markup(window, len, b),
            At::SubsetInstruction { question } => {
                if b == b'>' && question {
                    At::Subset
                } else {
                    At::SubsetInstruction {
                        question: b == b'?',
                    }
                }
            }
            At::SubsetComment { dashes } => match b {
                b'-' => At::SubsetComment {
                    dashes: dashes.saturating_add(1),
                },
                b'>' if dashes >= 2 => At::Subset,
                _ => At::SubsetComment { dashes: 0 },
            },
            At::SubsetDecl => {
                if b == b'>' {
                    At::Subset
                } else {
                    At::SubsetDecl
                }
            }
            At::SubsetQuoted { quote } => match (quote, b) {
                (0, b'>') => At::Subset,
                (0, b'"' | b'\'') => At::SubsetQuoted { quote: b },
                (q, _) if q == b => At::SubsetQuoted { quote: 0 },
                (q, _) => At::SubsetQuoted { quote: q },
            },
            At::AfterSubset => {
                if b == b'>' {
                    At::Text
                } else {
                    At::AfterSubset
                }
            }
            _ => at,
        };
        if next == At::Text {
            // `<!DOCTYPE`, any case, then a name (`emit_bang`).
            if usize::from(self.keyword) < b"DOCTYPE".len() || !self.doctype_named {
                self.malformed = true;
            }
        } else if let Some(&want) = b"DOCTYPE".get(usize::from(self.keyword)) {
            if b.eq_ignore_ascii_case(&want) {
                self.keyword += 1;
            } else {
                self.malformed = true;
                self.keyword = u8::MAX;
            }
        } else if !is_space(b) {
            self.doctype_named = true;
        }
        next
    }

    /// After `<` in a subset: quick-xml's `DtdParser::switch`.
    fn subset_markup(window: [u8; WINDOW], len: u8, b: u8) -> At {
        let mut window = window;
        if let Some(slot) = window.get_mut(usize::from(len)) {
            *slot = b;
        }
        let len = len.saturating_add(1);
        let seen = window.get(..usize::from(len)).unwrap_or_default();
        if seen.first() == Some(&b'?') {
            return At::SubsetInstruction { question: false };
        }
        match SUBSET_MARKUP.iter().position(|m| *m == seen) {
            Some(0) => return At::SubsetComment { dashes: 0 },
            Some(1) => return At::SubsetDecl,
            Some(_) => return At::SubsetQuoted { quote: 0 },
            None => {}
        }
        if SUBSET_MARKUP.iter().any(|m| m.starts_with(seen)) {
            At::SubsetMarkup { window, len }
        } else if b == b'>' {
            At::Subset
        } else {
            At::SubsetDecl
        }
    }

    pub fn feed(&mut self, chunk: &[u8]) {
        for &b in chunk {
            if self.capture == Capture::Done {
                return;
            }
            self.step(b);
        }
    }

    /// As [`feed`](Self::feed), past a found code too, handing `text` each
    /// byte and whether it is character data: in no tag, comment, CDATA
    /// section, instruction or DOCTYPE.
    pub fn feed_text(&mut self, chunk: &[u8], mut text: impl FnMut(u8, bool)) {
        for &b in chunk {
            let is_text = self.step(b);
            text(b, is_text);
        }
    }

    /// The root element, once its start tag closed.
    #[must_use]
    pub const fn root(&self) -> Root {
        self.root
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

    /// Whether the body held markup quick-xml's reader refuses, on which
    /// `object_store`'s parse fails.
    #[must_use]
    pub const fn malformed(&self) -> bool {
        self.malformed
    }

    /// Reads one byte; whether it is character data.
    fn step(&mut self, b: u8) -> bool {
        const CDATA: &[u8] = b"CDATA[";
        let text = self.at == At::Text && b != b'<';
        self.at = match self.at {
            At::Text if b == b'<' => At::Open,
            At::Text => {
                self.text(b);
                At::Text
            }
            At::Open => self.open(b),
            At::Bang => self.bang(b),
            At::Dash if b == b'-' => At::Comment { dashes: 0 },
            At::CdataOpen { matched } if CDATA.get(usize::from(matched)) == Some(&b) => {
                if usize::from(matched) + 1 == CDATA.len() {
                    At::Cdata { brackets: 0 }
                } else {
                    At::CdataOpen {
                        matched: matched + 1,
                    }
                }
            }
            At::Dash | At::CdataOpen { .. } => self.refused(),
            At::Start(tag) => self.start(tag, b),
            At::End { quote, spaced } => self.end(quote, spaced, b),
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
            At::Instruction { question, empty } => {
                if b == b'>' && question {
                    // `<?>` is no instruction (`emit_question_mark`).
                    if empty {
                        self.malformed = true;
                    }
                    At::Text
                } else {
                    At::Instruction {
                        question: b == b'?',
                        empty: false,
                    }
                }
            }
            at => self.doctype(at, b),
        };
        if self.at.spoils() {
            self.spoil();
        }
        text
    }
}

#[cfg(test)]
#[path = "code_scan_tests.rs"]
mod tests;
