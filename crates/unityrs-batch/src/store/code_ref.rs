//! A reference in character data, `&` to `;`, as `object_store`'s parse
//! resolves it: quick-xml 0.41's deserializer takes the five predefined
//! entities and character references (`resolve_reference`, `src/de/mod.rs`
//! 2457 to 2471; `resolve_xml_entity`, `src/escape.rs` 867 to 878), a
//! character reference's number decimal, or hexadecimal after a lowercase
//! `x`, unsigned, neither 0 nor past a `char` (`parse_number` and
//! `from_str_radix`, `src/escape.rs` 2349 to 2372). It resolves them in the
//! root's own text; a nested element it skips unread. Its reader refuses an `&` with no `;` before the next `<` or `&`
//! (`UnclosedReference`, `src/reader/mod.rs` 283 to 323).

/// The largest `char`.
const CHAR_MAX: u32 = 0x10_ffff;

/// What a reference resolves to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Resolved {
    /// A character.
    Char,
    /// Nothing: the deserializer fails on it.
    Fails,
}

/// Where a reference's reading is.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Form {
    /// Just after `&`.
    #[default]
    Start,
    /// An entity's name.
    Named,
    /// After `&#`.
    Hash,
    Decimal,
    /// After `&#x`.
    HexStart,
    Hex,
    /// No reference the deserializer resolves.
    Bad,
}

/// A reference being read, after its `&`.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Ref {
    form: Form,
    /// A character reference's number, held at past [`CHAR_MAX`].
    value: u32,
    /// An entity's name: none predefined is longer.
    name: [u8; 4],
    len: u8,
}

impl Ref {
    /// Reads one byte before the `;`.
    pub(super) fn feed(&mut self, b: u8) {
        self.form = match (self.form, b) {
            (Form::Start, b'#') => Form::Hash,
            (Form::Start | Form::Named, _) => match self.name.get_mut(usize::from(self.len)) {
                Some(slot) => {
                    *slot = b;
                    self.len += 1;
                    Form::Named
                }
                None => Form::Bad,
            },
            (Form::Hash, b'x') => Form::HexStart,
            (Form::Hash | Form::Decimal, b'0'..=b'9') => self.digit(10, b - b'0', Form::Decimal),
            (Form::HexStart | Form::Hex, _) => match char::from(b).to_digit(16) {
                Some(d) => self.digit(16, u8::try_from(d).unwrap_or(0), Form::Hex),
                None => Form::Bad,
            },
            _ => Form::Bad,
        };
    }

    fn digit(&mut self, radix: u32, d: u8, form: Form) -> Form {
        self.value = self
            .value
            .saturating_mul(radix)
            .saturating_add(u32::from(d))
            .min(CHAR_MAX + 1);
        form
    }

    /// The reference ended at its `;`.
    pub(super) fn resolved(&self) -> Resolved {
        match self.form {
            Form::Named => {
                let name = self.name.get(..usize::from(self.len)).unwrap_or_default();
                if matches!(name, b"lt" | b"gt" | b"amp" | b"apos" | b"quot") {
                    Resolved::Char
                } else {
                    Resolved::Fails
                }
            }
            Form::Decimal | Form::Hex => match char::from_u32(self.value) {
                None | Some('\0') => Resolved::Fails,
                Some(_) => Resolved::Char,
            },
            Form::Start | Form::Hash | Form::HexStart | Form::Bad => Resolved::Fails,
        }
    }
}
