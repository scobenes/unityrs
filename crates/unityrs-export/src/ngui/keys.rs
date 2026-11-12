//! Key templates, `safe()` and the key rules (`docs/BATCH_SPEC.md` 2.1 to
//! 2.3; `docs/plans/extract.md` E27, E28).
//!
//! A [`Template`] is parsed once, refusing an unknown variable or one its
//! kind does not allow (validation's refusal, exit 78, is the engine's). Its
//! expansion is bytes, a plain variable written verbatim (a name's stored
//! bytes) and a `safe_` one through [`safe_into`]; [`check`] then names the
//! first rule of 2.3 the key breaks, or gives it back as UTF-8, and
//! [`check_prefixed`] checks it again after the sink's prefix, as 2.3 says.
//! An expansion stops at [`STOP`] bytes (`docs/plans/batch.md` 2.6 step 5),
//! written into a buffer of that capacity the caller charged, each value
//! read only as far as the room left (review round 2's S3), so a key past
//! [`KEY_MAX`] is read no further and [`check`] decides it on what it holds
//! (2.3's rule for a key past its bound). `prefix_conflict` and
//! `archive_conflict` are phase 6's (4.6). [`KeyTable`] is the attempt's one
//! key table, charged to `M_X`.

use std::sync::Arc;

use unityrs_core::limits::{Charge, Meter, heap_cost};

use super::classify::Class;
use super::table::{self, Set};

/// An output kind a template expands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TemplateKind {
    /// `outputs.textures`' key.
    Texture,
    /// `outputs.sprites`' key.
    Sprite,
    /// `atlases.sprite_key`.
    AtlasSprite,
    /// `atlases.json_key`.
    AtlasJson,
}

impl TemplateKind {
    /// A `members` entry's `kind` (1.6) for this kind, as its outputs'
    /// kind names it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Texture => "texture",
            Self::Sprite => "sprite",
            Self::AtlasSprite => "atlas_sprite",
            Self::AtlasJson => "atlas_json",
        }
    }
}

/// A template variable (2.1's table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Var {
    /// `{input}`.
    Input,
    /// `{safe_input}`.
    SafeInput,
    /// `{type}`.
    Type,
    /// `{name}`.
    Name,
    /// `{safe_name}`.
    SafeName,
    /// `{path_id}`.
    PathId,
    /// `{atlas}`.
    Atlas,
    /// `{safe_atlas}`.
    SafeAtlas,
    /// `{sprite}`.
    Sprite,
    /// `{safe_sprite}`.
    SafeSprite,
}

impl Var {
    /// A `safe_` variable's plain one.
    #[must_use]
    pub fn plain(self) -> Option<Self> {
        Some(match self {
            Self::SafeInput => Self::Input,
            Self::SafeName => Self::Name,
            Self::SafeAtlas => Self::Atlas,
            Self::SafeSprite => Self::Sprite,
            _ => return None,
        })
    }

    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "input" => Self::Input,
            "safe_input" => Self::SafeInput,
            "type" => Self::Type,
            "name" => Self::Name,
            "safe_name" => Self::SafeName,
            "path_id" => Self::PathId,
            "atlas" => Self::Atlas,
            "safe_atlas" => Self::SafeAtlas,
            "sprite" => Self::Sprite,
            "safe_sprite" => Self::SafeSprite,
            _ => return None,
        })
    }

    /// Whether 2.1's table allows it in a template of `kind`.
    #[must_use]
    pub fn allowed(self, kind: TemplateKind) -> bool {
        match self {
            Self::Input | Self::SafeInput | Self::PathId => true,
            Self::Type | Self::Name | Self::SafeName => {
                matches!(kind, TemplateKind::Texture | TemplateKind::Sprite)
            }
            Self::Atlas | Self::SafeAtlas => {
                matches!(kind, TemplateKind::AtlasSprite | TemplateKind::AtlasJson)
            }
            Self::Sprite | Self::SafeSprite => kind == TemplateKind::AtlasSprite,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Piece {
    Lit(String),
    Var(Var),
}

/// A parsed template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pieces: Vec<Piece>,
}

/// Why a template is refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TemplateError {
    /// An empty template (the grammar's `1*`).
    #[error("an empty template")]
    Empty,
    /// A `{` with no `}`, or a lone `}`.
    #[error("an unmatched brace at byte {0}")]
    Brace(usize),
    /// A variable 2.1 does not define.
    #[error("unknown variable {0:?}")]
    Unknown(String),
    /// A variable 2.1 does not allow in this kind's templates.
    #[error("variable {0:?} is not allowed here")]
    NotAllowed(String),
}

/// A piece of a template's text as 2.1's grammar reads it, borrowed from
/// the text: a run of literal characters, the one brace `{{` or `}}`
/// writes, or a variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token<'t> {
    /// Literal text, written as it is.
    Lit(&'t str),
    /// A variable 2.1 allows in the template's kind.
    Var(Var),
}

/// The tokens of `text`, a template of `kind`, in order: the one reading
/// of 2.1's grammar, which [`Template::parse`] and [`check_template`]
/// share. It allocates nothing but an error's name, and ends after the
/// first error. An empty text has no token (the grammar's `1*` is the
/// caller's).
#[must_use]
pub fn tokens(text: &str, kind: TemplateKind) -> Tokens<'_> {
    Tokens {
        text,
        kind,
        at: 0,
        done: false,
    }
}

/// [`tokens`]' iterator.
#[derive(Clone, Debug)]
pub struct Tokens<'t> {
    text: &'t str,
    kind: TemplateKind,
    at: usize,
    done: bool,
}

impl<'t> Iterator for Tokens<'t> {
    type Item = Result<Token<'t>, TemplateError>;

    fn next(&mut self) -> Option<Self::Item> {
        let at = self.at;
        let rest = self.text.get(at..).unwrap_or_default();
        if self.done || rest.is_empty() {
            return None;
        }
        let token = if rest.starts_with("{{") || rest.starts_with("}}") {
            self.at += 2;
            Ok(Token::Lit(rest.get(..1).unwrap_or_default()))
        } else if let Some(after) = rest.strip_prefix('{') {
            match after.find('}') {
                None => Err(TemplateError::Brace(at)),
                Some(end) => {
                    let name = after.get(..end).unwrap_or_default();
                    self.at += end + 2;
                    match Var::parse(name) {
                        None => Err(TemplateError::Unknown(name.to_owned())),
                        Some(var) if !var.allowed(self.kind) => {
                            Err(TemplateError::NotAllowed(name.to_owned()))
                        }
                        Some(var) => Ok(Token::Var(var)),
                    }
                }
            }
        } else if rest.starts_with('}') {
            Err(TemplateError::Brace(at))
        } else {
            let end = rest.find(['{', '}']).unwrap_or(rest.len());
            self.at += end;
            Ok(Token::Lit(rest.get(..end).unwrap_or_default()))
        };
        self.done = token.is_err();
        Some(token)
    }
}

impl Template {
    /// Parses `text` for a template of `kind` ([`tokens`]).
    pub fn parse(text: &str, kind: TemplateKind) -> Result<Self, TemplateError> {
        if text.is_empty() {
            return Err(TemplateError::Empty);
        }
        let mut pieces = Vec::new();
        let mut lit = String::new();
        for token in tokens(text, kind) {
            match token? {
                Token::Lit(s) => lit.push_str(s),
                Token::Var(var) => {
                    if !lit.is_empty() {
                        pieces.push(Piece::Lit(std::mem::take(&mut lit)));
                    }
                    pieces.push(Piece::Var(var));
                }
            }
        }
        if !lit.is_empty() {
            pieces.push(Piece::Lit(lit));
        }
        Ok(Self { pieces })
    }

    /// The expansion into `out` (cleared first): each variable's value from
    /// `value` (its raw bytes; a `safe_` variable's is [`safe`] of its plain
    /// one's, made as it is written), stopping at [`STOP`] bytes: what
    /// follows decides nothing ([`check`]), so it is never made. No value
    /// is copied whole: each is written up to the room left, so `out` never
    /// holds more than [`STOP`] bytes, and a caller that gave it that
    /// capacity allocates nothing here.
    ///
    /// Gives how many of `out`'s last bytes are the start of a code point
    /// the stop cut, one the whole expansion holds past the stop
    /// ([`cut_tail`]): a text of the expansion drops them, where an invalid
    /// byte is one code point (review round 3's P3). The code point may
    /// span pieces: the bytes before the stop are read joined with the
    /// expansion's next ones, of the piece cut and those after it (review
    /// round 4's A3).
    pub fn expand<'v>(&self, value: impl Fn(Var) -> &'v [u8], out: &mut Vec<u8>) -> usize {
        out.clear();
        for (k, p) in self.pieces.iter().enumerate() {
            let room = STOP.saturating_sub(out.len());
            if room == 0 {
                return self.tail(out, &[], k, &value);
            }
            // A `safe_` value is ASCII: the stop cuts no code point of it.
            let src = match p {
                Piece::Lit(s) => s.as_bytes(),
                Piece::Var(v) => match v.plain() {
                    Some(plain) => {
                        safe_into(value(plain), room, out);
                        continue;
                    }
                    None => value(*v),
                },
            };
            out.extend(src.iter().take(room));
            if src.len() > room {
                let rest = src.get(room..).unwrap_or_default();
                return self.tail(out, rest, k + 1, &value);
            }
        }
        0
    }

    /// [`cut_tail`] at the stop: `out`'s last three bytes joined with the
    /// expansion's next three, `rest` of the piece cut, then the pieces from
    /// `next` on. A `safe_` piece is ASCII, so no code point continues into
    /// it: the bytes ahead end there.
    fn tail<'v>(
        &self,
        out: &[u8],
        rest: &[u8],
        next: usize,
        value: &impl Fn(Var) -> &'v [u8],
    ) -> usize {
        let head = out.get(out.len().saturating_sub(3)..).unwrap_or_default();
        let later = self
            .pieces
            .get(next..)
            .unwrap_or_default()
            .iter()
            .map_while(|p| match p {
                Piece::Lit(s) => Some(s.as_bytes()),
                Piece::Var(v) => v.plain().is_none().then(|| value(*v)),
            })
            .flatten();
        let ahead = rest.iter().chain(later).take(3);
        let mut window = [0u8; 6];
        let mut len = 0;
        for (slot, b) in window.iter_mut().zip(head.iter().chain(ahead)) {
            *slot = *b;
            len += 1;
        }
        cut_tail(window.get(..len).unwrap_or_default(), head.len())
    }
}

/// How many of `src`'s first `at` bytes start a code point that `src`
/// holds whole past `at`: none when `at` falls on a code point boundary,
/// or the bytes there are no valid code point's start.
#[must_use]
pub fn cut_tail(src: &[u8], at: usize) -> usize {
    let head = src.get(..at).unwrap_or_default();
    for back in 1..=head.len().min(3) {
        let i = head.len() - back;
        let Some(&b) = head.get(i) else {
            return 0;
        };
        if b & 0xC0 == 0x80 {
            continue;
        }
        let width = match b {
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => 0,
        };
        let whole = src
            .get(i..i + width)
            .is_some_and(|c| std::str::from_utf8(c).is_ok());
        return if width > back && whole { back } else { 0 };
    }
    0
}

/// [`safe`] of `bytes` written into `out`, at most `room` bytes of it: the
/// same bytes as the first `room` of [`safe`]'s, made a code point at a
/// time, with no copy of `bytes`. A run of `.` is written only once a code
/// point other than `.` follows it (a trailing run is stripped), and a
/// value of nothing but `.` is `unnamed`.
pub fn safe_into(bytes: &[u8], room: usize, out: &mut Vec<u8>) {
    let start = out.len();
    let lead = bytes.iter().take_while(|b| **b == b'.').count();
    let rest = bytes.get(lead..).unwrap_or_default();
    if rest.is_empty() {
        out.extend(b"unnamed".iter().take(room));
        return;
    }
    let mut dots = 0usize;
    let put = |c: u8, out: &mut Vec<u8>| {
        if out.len() - start < room {
            out.push(c);
            true
        } else {
            false
        }
    };
    for chunk in rest.utf8_chunks() {
        let valid = chunk.valid().chars().map(Some);
        let invalid = chunk.invalid().iter().map(|_| None);
        for c in valid.chain(invalid) {
            let mapped = match c {
                Some('.') => {
                    dots += 1;
                    continue;
                }
                Some(c) if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') => c as u8,
                _ => b'_',
            };
            // A run of `.` followed by another code point is kept.
            for _ in 0..std::mem::take(&mut dots) {
                if !put(b'.', out) {
                    return;
                }
            }
            if !put(mapped, out) {
                return;
            }
        }
    }
}

/// `safe()` (2.2): per code point, an invalid UTF-8 byte counting as one,
/// every one outside `[A-Za-z0-9._-]` becomes `_`; then every leading and
/// trailing `.` is stripped; an empty result is `unnamed`.
#[must_use]
pub fn safe(bytes: &[u8]) -> String {
    let mut out = Vec::new();
    safe_into(bytes, usize::MAX, &mut out);
    String::from_utf8(out).unwrap_or_default()
}

/// The rules of 2.3 an attempt checks, in the table's order, which their
/// order as values keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum KeyRule {
    /// `empty`.
    Empty,
    /// `not_utf8`.
    NotUtf8,
    /// `control_char`.
    ControlChar,
    /// `leading_slash`.
    LeadingSlash,
    /// `trailing_slash`.
    TrailingSlash,
    /// `empty_segment`.
    EmptySegment,
    /// `dot_segment`.
    DotSegment,
    /// `segment_too_long`.
    SegmentTooLong,
    /// `too_long`.
    TooLong,
    /// `reserved_prefix`.
    ReservedPrefix,
    /// `reserved_name`.
    ReservedName,
}

impl KeyRule {
    /// Its name in `unwritable_key(<rule>)`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::NotUtf8 => "not_utf8",
            Self::ControlChar => "control_char",
            Self::LeadingSlash => "leading_slash",
            Self::TrailingSlash => "trailing_slash",
            Self::EmptySegment => "empty_segment",
            Self::DotSegment => "dot_segment",
            Self::SegmentTooLong => "segment_too_long",
            Self::TooLong => "too_long",
            Self::ReservedPrefix => "reserved_prefix",
            Self::ReservedName => "reserved_name",
        }
    }
}

/// The longest segment (`NAME_MAX`).
pub const SEGMENT_MAX: usize = 255;
/// The longest key.
pub const KEY_MAX: usize = 1024;
/// Where an expansion stops: one byte past [`KEY_MAX`], which is enough to
/// know the key is `too_long`.
pub const STOP: usize = KEY_MAX + 1;

/// The key 2.3's `reserved_prefix` refuses, and every key under it.
pub const RESERVED_PREFIX: &str = "_unityrs";
/// What 2.3's `reserved_name` refuses a segment to start with.
pub const RESERVED_NAME: &str = ".unityrs-tmp-";

/// Whether 2.3's `dot_segment` refuses `segment`.
fn dot_segment(segment: &str) -> bool {
    segment == "." || segment == ".."
}

/// Which of 2.3's rules apply: every one to a key, all but
/// `reserved_prefix` and `reserved_name` to an archive member path (4.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rules {
    Key,
    Member,
}

/// The key, or the first rule of 2.3 it breaks (but `prefix_conflict` and
/// `archive_conflict`, which phase 6 decides).
///
/// Only the first [`STOP`] bytes are read. A key past [`KEY_MAX`] is
/// `too_long` unless a rule before it in 2.3's order breaks within them;
/// a rule that needs the key's end is not applied to it (`trailing_slash`,
/// `dot_segment` of its last segment, and `not_utf8` of a code point the
/// stop cuts), and a break past byte 1,025, a `control_char` among them, is
/// not seen: the key is `too_long` (2.3). A segment's length is its bytes
/// as read, a code point the stop cuts among them (review round 2's C4). So
/// the answer is the same however much of the key was made.
pub fn check(key: &[u8]) -> Result<&str, KeyRule> {
    rules(key, Rules::Key)
}

/// An archive member path's check (4.7): [`check`] without
/// `reserved_prefix` and `reserved_name`.
pub fn check_member(path: &[u8]) -> Result<&str, KeyRule> {
    rules(path, Rules::Member)
}

/// 2.3's two checks: the key as given, then after `prefix` (the sink's),
/// the first rule either breaks naming the refusal. The two are joined in
/// `scratch`, at most [`STOP`] bytes of them, so a `scratch` of that
/// capacity allocates nothing.
pub fn check_prefixed<'k>(
    key: &'k [u8],
    prefix: &str,
    scratch: &mut Vec<u8>,
) -> Result<&'k str, KeyRule> {
    let checked = check(key)?;
    if !prefix.is_empty() {
        scratch.clear();
        scratch.extend(prefix.as_bytes().iter().take(STOP));
        let room = STOP.saturating_sub(scratch.len());
        scratch.extend(key.iter().take(room));
        check(scratch)?;
    }
    Ok(checked)
}

fn rules(key: &[u8], which: Rules) -> Result<&str, KeyRule> {
    if key.is_empty() {
        return Err(KeyRule::Empty);
    }
    let long = key.len() > KEY_MAX;
    let read = key.get(..STOP).unwrap_or(key);
    let text = match std::str::from_utf8(read) {
        Ok(k) => k,
        // A code point the stop cuts is past what is read.
        Err(e) if long && e.error_len().is_none() => read
            .get(..e.valid_up_to())
            .and_then(|v| std::str::from_utf8(v).ok())
            .ok_or(KeyRule::NotUtf8)?,
        Err(_) => return Err(KeyRule::NotUtf8),
    };
    if text.chars().any(|c| c < ' ' || c == '\u{7f}') {
        return Err(KeyRule::ControlChar);
    }
    if text.starts_with('/') {
        return Err(KeyRule::LeadingSlash);
    }
    if !long && text.ends_with('/') {
        return Err(KeyRule::TrailingSlash);
    }
    if text.contains("//") {
        return Err(KeyRule::EmptySegment);
    }
    let segments = text.split('/');
    let settled = if long {
        segments.clone().count().saturating_sub(1)
    } else {
        usize::MAX
    };
    if segments.take(settled).any(dot_segment) {
        return Err(KeyRule::DotSegment);
    }
    // Measured on the bytes read: a code point the stop cuts still counts.
    if read.split(|b| *b == b'/').any(|s| s.len() > SEGMENT_MAX) {
        return Err(KeyRule::SegmentTooLong);
    }
    if long {
        return Err(KeyRule::TooLong);
    }
    if which == Rules::Key {
        if text == RESERVED_PREFIX
            || text
                .strip_prefix(RESERVED_PREFIX)
                .is_some_and(|r| r.starts_with('/'))
        {
            return Err(KeyRule::ReservedPrefix);
        }
        if text.split('/').any(|s| s.starts_with(RESERVED_NAME)) {
            return Err(KeyRule::ReservedName);
        }
    }
    Ok(text)
}

/// What one key costs its table beside the table's buckets: the string,
/// at `heap_cost`.
#[must_use]
pub fn entry_cost(len: usize) -> u64 {
    heap_cost(u64::try_from(len).unwrap_or(u64::MAX))
}

/// The attempt's key table (4.4 step 3): one for every kind's stage, the
/// engine's to make and hand to each attempt, every key charged to `M_X`
/// as it is taken (`docs/plans/batch.md` 2.6 step 5): its string, and the
/// set's buckets before they grow ([`Set`]).
#[derive(Debug)]
pub struct KeyTable {
    keys: Set<Box<str>>,
    strings: Charge,
}

impl KeyTable {
    /// An empty table charging `meter`.
    #[must_use]
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            keys: Set::new(meter),
            strings: Charge::zero(meter),
        }
    }

    /// Whether `key` is taken.
    #[must_use]
    pub fn contains(&self, key: &str) -> bool {
        self.keys.contains(key)
    }

    /// Takes `key`: false when it was taken already; a refused charge (its
    /// string's, or the set's growth) takes nothing.
    pub fn take(&mut self, key: &str) -> Result<bool, Class> {
        if self.keys.contains(key) {
            return Ok(false);
        }
        self.keys.reserve_one()?;
        self.strings
            .grow(entry_cost(key.len()))
            .map_err(|e| table::refused(&e))?;
        self.keys.insert(Box::from(key))
    }

    /// How many keys it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

mod literal;

pub use literal::{TemplateRefusal, check_member_template, check_template};

#[cfg(test)]
mod tests;
