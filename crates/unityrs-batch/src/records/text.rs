//! Streaming JSON escaping and the source-byte cut rule (7.3).
/// Maximum source bytes, before invalid bytes expand to replacement chars.
pub const CUT: usize = 1024;

/// Borrowed text. `Prepared` is for classifier/export strings already cut
/// and repaired: applying the source-byte cut twice would lose information.
#[derive(Clone, Copy, Debug)]
pub enum Text<'a> {
    /// Bytes from a bundle or a validated spec field.
    Raw(&'a [u8]),
    /// One combined detail (code, space, entry path), each part ending at
    /// a character boundary; the parts share one source-byte allowance.
    Joined(&'a [&'a [u8]]),
    /// Already repaired UTF-8 plus its inherited cut flag. The record
    /// encoder enforces the line bound even for malformed prepared text.
    Prepared(&'a str, bool),
}
impl<'a> From<&'a str> for Text<'a> {
    fn from(value: &'a str) -> Self {
        Self::Raw(value.as_bytes())
    }
}
impl Text<'_> {
    /// Visit code points without a temporary string; return whether cut.
    pub fn walk<E>(self, mut put: impl FnMut(char) -> Result<(), E>) -> Result<bool, E> {
        let one;
        let parts = match self {
            Self::Raw(raw) => {
                one = [raw];
                &one[..]
            }
            Self::Joined(parts) => parts,
            Self::Prepared(text, cut) => {
                for c in text.chars() {
                    put(c)?;
                }
                return Ok(cut);
            }
        };
        let mut used = 0;
        for part in parts {
            for chunk in part.utf8_chunks() {
                for c in chunk.valid().chars() {
                    if used + c.len_utf8() > CUT {
                        return Ok(true);
                    }
                    used += c.len_utf8();
                    put(c)?;
                }
                for _ in chunk.invalid() {
                    if used == CUT {
                        return Ok(true);
                    }
                    used += 1;
                    put(char::REPLACEMENT_CHARACTER)?;
                }
            }
        }
        Ok(false)
    }
}
/// JSON string contents, with no surrounding quotes. Every invalid source
/// byte becomes its own U+FFFD; control bytes take at most six output bytes.
pub fn escaped<E>(text: Text<'_>, mut put: impl FnMut(&[u8]) -> Result<(), E>) -> Result<bool, E> {
    text.walk(|c| match c {
        '"' => put(b"\\\""),
        '\\' => put(b"\\\\"),
        c if c < ' ' => {
            let n = u32::from(c);
            let hex = b"0123456789abcdef";
            let hi = hex.get((n / 16) as usize).copied().unwrap_or(b'0');
            let lo = hex.get((n % 16) as usize).copied().unwrap_or(b'0');
            put(&[b'\\', b'u', b'0', b'0', hi, lo])
        }
        c => put(c.encode_utf8(&mut [0; 4]).as_bytes()),
    })
}

#[cfg(test)]
mod tests {
    use super::{Text, escaped};
    fn render(t: Text<'_>) -> (String, bool) {
        let mut bytes = Vec::new();
        let cut = escaped(t, |s| {
            bytes.extend_from_slice(s);
            Ok::<_, ()>(())
        })
        .unwrap();
        (String::from_utf8(bytes).unwrap(), cut)
    }
    #[test]
    fn control_quotes_and_invalid_bytes_are_json_contents() {
        assert_eq!(
            render(Text::Raw(b"\0\n\"\\\xff\xf0\x80")),
            ("\\u0000\\u000a\\\"\\\\���".into(), false)
        );
    }
    #[test]
    fn a_truncated_multibyte_sequence_replaces_each_invalid_source_byte() {
        assert_eq!(render(Text::Raw(b"\xf0\x9f\x92")), ("���".into(), false));
        let mut bytes = vec![b'a'; 1023];
        bytes.extend_from_slice(b"\xe2\x82");
        assert_eq!(
            render(Text::Raw(&bytes)),
            (format!("{}�", "a".repeat(1023)), true)
        );
    }
    #[test]
    fn multibyte_and_each_invalid_byte_obey_source_cut() {
        let mut b = vec![b'a'; 1023];
        b.extend_from_slice("é".as_bytes());
        assert_eq!(render(Text::Raw(&b)), ("a".repeat(1023), true));
        assert_eq!(render(Text::Raw(&[0xff; 1024])), ("�".repeat(1024), false));
        assert_eq!(render(Text::Raw(&[0xff; 1025])), ("�".repeat(1024), true));
    }
    #[test]
    fn joined_details_share_one_allowance_and_prepared_text_is_not_recut() {
        let path = [b'x'; 1024];
        let parts: &[&[u8]] = &[b"read", b" ", &path];
        assert_eq!(
            render(Text::Joined(parts)),
            (format!("read {}", "x".repeat(1019)), true)
        );
        let prepared = "�".repeat(1024);
        assert_eq!(
            render(Text::Prepared(&prepared, true)),
            (prepared.clone(), true)
        );
    }
}
