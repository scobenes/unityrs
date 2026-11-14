//! The root start tag's attributes, read as quick-xml 0.41's `Attributes`
//! iterator reads them (`src/events/attributes.rs`, `IterState::next`, 1222
//! to 1360), which `object_store`'s parse runs over the root's start tag
//! before anything inside it (`src/de/map.rs`, `next_key_seed`, line 260),
//! failing on its first error; a nested element's it never reads. An attribute is a key, `=` and a
//! quoted value, whitespace allowed around the `=`, none needed between
//! attributes; a key repeated is refused too (`check_for_duplicates`,
//! 1135 to 1154).

use super::is_space;

/// Keys compared for repeats, and the bytes of each; past them, a repeat
/// goes unseen, and the answer is left to `object_store`'s parse.
const KEYS: usize = 4;
const KEY_MAX: usize = 16;

/// Where the attribute reading is.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum At {
    /// Between attributes.
    #[default]
    Gap,
    /// In a key.
    Key,
    /// In whitespace after a key.
    AfterKey,
    /// After `=`, before the value's quote.
    Eq,
    /// In a value, until its closing quote.
    Value { quote: u8 },
}

/// One key, as read.
#[derive(Clone, Copy, Debug, Default)]
struct Key {
    bytes: [u8; KEY_MAX],
    len: u8,
    /// Past [`KEY_MAX`] bytes: compared with none.
    long: bool,
}

impl Key {
    fn push(&mut self, b: u8) {
        if let Some(slot) = self.bytes.get_mut(usize::from(self.len)) {
            *slot = b;
            self.len += 1;
        } else {
            self.long = true;
        }
    }
    fn get(&self) -> Option<&[u8]> {
        if self.long {
            None
        } else {
            self.bytes.get(..usize::from(self.len))
        }
    }
}

/// The root start tag's attributes, fed the bytes after its name.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Attrs {
    at: At,
    key: Key,
    keys: [Key; KEYS],
    kept: u8,
    /// A `/` held back: the one that makes the tag empty is no attribute's.
    slash: bool,
    failed: bool,
    /// A key went uncompared: past [`KEY_MAX`] bytes, or [`KEYS`] keys.
    unchecked: bool,
}

impl Attrs {
    /// Reads one byte of the tag, after its name.
    pub(super) fn feed(&mut self, b: u8) {
        if std::mem::take(&mut self.slash) {
            self.step(b'/');
        }
        if b == b'/' && !matches!(self.at, At::Value { .. }) {
            self.slash = true;
        } else {
            self.step(b);
        }
    }

    /// The tag ended: whether its attributes are ones the parse takes.
    /// A `/` just before the end makes the tag empty and is dropped
    /// (`emit_start`).
    pub(super) fn ended(&mut self, empty: bool) -> bool {
        if std::mem::take(&mut self.slash) && !empty {
            self.step(b'/');
        }
        // A key with no value, or a value with no end, is refused.
        !self.failed && self.at == At::Gap
    }

    fn step(&mut self, b: u8) {
        if self.failed {
            return;
        }
        self.at = match self.at {
            At::Gap if is_space(b) => At::Gap,
            // A key's first byte is its own, even `=`.
            At::Gap => {
                self.key = Key::default();
                self.key.push(b);
                At::Key
            }
            At::Key | At::AfterKey if b == b'=' => self.keyed(),
            At::Key | At::AfterKey if is_space(b) => At::AfterKey,
            At::Key => {
                self.key.push(b);
                At::Key
            }
            At::Eq if is_space(b) => At::Eq,
            At::Eq if matches!(b, b'"' | b'\'') => At::Value { quote: b },
            // A key with no `=` (`ExpectedEq`), a value unquoted
            // (`UnquotedValue`).
            At::AfterKey | At::Eq => {
                self.failed = true;
                At::Gap
            }
            At::Value { quote } if b == quote => At::Gap,
            at @ At::Value { .. } => at,
        };
    }

    /// A key read to its `=`: refused if one before it is the same.
    fn keyed(&mut self) -> At {
        let kept = self.keys.get(..usize::from(self.kept)).unwrap_or_default();
        let Some(key) = self.key.get() else {
            self.unchecked = true;
            return At::Eq;
        };
        if kept.iter().any(|seen| seen.get() == Some(key)) {
            self.failed = true;
        } else if let Some(slot) = self.keys.get_mut(usize::from(self.kept)) {
            *slot = self.key;
            self.kept += 1;
        } else {
            self.unchecked = true;
        }
        At::Eq
    }

    /// Whether a key went uncompared, a repeat of it left to the parse.
    #[cfg(test)]
    pub(super) const fn unchecked(&self) -> bool {
        self.unchecked
    }
}
