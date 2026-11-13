//! Recognize only the identity envelope, including future manifest versions.
//! The caller owns the bounded read buffer; parsing allocates nothing, even
//! for escaped strings or unknown nested values. No general JSON tree is built.

pub const HEAD_LIMIT: usize = 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unreadable {
    TooLarge,
    NotAManifest,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub attempt: u32,
    pub write_seq: u64,
}

/// Require all seven typed identity fields and the expected `run_id`.
/// Duplicate identity fields, malformed JSON and mismatched run ids refuse
/// overwrite. Unknown fields are syntax-checked and discarded.
pub fn recognize(bytes: &[u8], expected_run: &str) -> Result<Identity, Unreadable> {
    if bytes.len() > HEAD_LIMIT {
        return Err(Unreadable::TooLarge);
    }
    std::str::from_utf8(bytes).map_err(|_| Unreadable::NotAManifest)?;
    parse(bytes, expected_run).ok_or(Unreadable::NotAManifest)
}
fn parse(bytes: &[u8], expected: &str) -> Option<Identity> {
    let mut p = Parser { bytes, at: 0 };
    p.take(b'{')?;
    let mut seen = 0_u8;
    let mut attempt = 0;
    let mut write_seq = 0;
    loop {
        let name = p.string()?;
        let index = [
            "manifest_version",
            "run_id",
            "attempt",
            "write_seq",
            "binary_version",
            "spec_version",
            "spec_sha256",
        ]
        .iter()
        .position(|s| name.equals(s));
        p.take(b':')?;
        if let Some(i) = index {
            let bit = 1 << i;
            if seen & bit != 0 {
                return None;
            }
            seen |= bit;
            match i {
                0 | 5 => {
                    if p.integer()? == 0 {
                        return None;
                    }
                }
                1 => {
                    if !p.string()?.equals(expected) {
                        return None;
                    }
                }
                2 => {
                    attempt = u32::try_from(p.integer()?).ok()?;
                    if attempt == 0 || attempt > i32::MAX.unsigned_abs() {
                        return None;
                    }
                }
                3 => write_seq = p.integer()?,
                4 => {
                    p.string()?;
                }
                6 => {
                    let mut count = 0;
                    let mut valid = true;
                    p.string()?.each(|b| {
                        count += 1;
                        valid &= b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
                    })?;
                    if count != 64 || !valid {
                        return None;
                    }
                }
                _ => return None,
            }
        } else {
            p.value(2)?;
        }
        if p.next() == Some(b'}') {
            p.at += 1;
            break;
        }
        p.take(b',')?;
    }
    if seen != 0x7f || p.next().is_some() {
        return None;
    }
    Some(Identity { attempt, write_seq })
}
struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Parser<'a> {
    fn next(&mut self) -> Option<u8> {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| matches!(b, b' ' | b'\n' | b'\r' | b'\t'))
        {
            self.at += 1;
        }
        self.bytes.get(self.at).copied()
    }
    fn take(&mut self, b: u8) -> Option<()> {
        if self.next()? != b {
            return None;
        }
        self.at += 1;
        Some(())
    }
    fn string(&mut self) -> Option<JsonString<'a>> {
        self.take(b'"')?;
        let start = self.at;
        loop {
            let b = *self.bytes.get(self.at)?;
            if b == b'"' {
                let text = JsonString(self.bytes.get(start..self.at)?);
                text.each(|_| {})?;
                self.at += 1;
                return Some(text);
            }
            if b < 0x20 {
                return None;
            }
            if b == b'\\' {
                self.at += 1;
                self.bytes.get(self.at)?;
            }
            self.at += 1;
        }
    }
    fn number(&mut self) -> Option<&'a [u8]> {
        self.next()?;
        let start = self.at;
        if self.bytes.get(self.at) == Some(&b'-') {
            self.at += 1;
        }
        match self.bytes.get(self.at)? {
            b'0' => self.at += 1,
            b'1'..=b'9' => {
                self.at += 1;
                self.digits();
            }
            _ => return None,
        }
        if self.bytes.get(self.at) == Some(&b'.') {
            self.at += 1;
            if self.digits() == 0 {
                return None;
            }
        }
        if self
            .bytes
            .get(self.at)
            .is_some_and(|b| matches!(b, b'e' | b'E'))
        {
            self.at += 1;
            if self
                .bytes
                .get(self.at)
                .is_some_and(|b| matches!(b, b'+' | b'-'))
            {
                self.at += 1;
            }
            if self.digits() == 0 {
                return None;
            }
        }
        self.bytes.get(start..self.at)
    }
    fn digits(&mut self) -> usize {
        let start = self.at;
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        self.at - start
    }
    fn integer(&mut self) -> Option<u64> {
        self.number()?.iter().try_fold(0_u64, |n, b| {
            if !b.is_ascii_digit() {
                return None;
            }
            n.checked_mul(10)?.checked_add(u64::from(b - b'0'))
        })
    }
    fn value(&mut self, depth: usize) -> Option<()> {
        if depth >= 128 {
            return None;
        }
        match self.next()? {
            b'"' => {
                self.string()?;
            }
            b'{' | b'[' => {
                let object = self.next()? == b'{';
                let close = if object { b'}' } else { b']' };
                self.at += 1;
                if self.next() == Some(close) {
                    self.at += 1;
                    return Some(());
                }
                loop {
                    if object {
                        self.string()?;
                        self.take(b':')?;
                    }
                    self.value(depth + 1)?;
                    if self.next() == Some(close) {
                        self.at += 1;
                        break;
                    }
                    self.take(b',')?;
                }
            }
            b'n' => self.literal(b"null")?,
            b't' => self.literal(b"true")?,
            b'f' => self.literal(b"false")?,
            _ => {
                self.number()?;
            }
        }
        Some(())
    }
    fn literal(&mut self, word: &[u8]) -> Option<()> {
        if self.bytes.get(self.at..self.at + word.len())? != word {
            return None;
        }
        self.at += word.len();
        Some(())
    }
}
#[derive(Clone, Copy)]
struct JsonString<'a>(&'a [u8]);
impl JsonString<'_> {
    fn equals(self, expected: &str) -> bool {
        let mut iter = expected.bytes();
        let mut equal = true;
        self.each(|b| equal &= iter.next() == Some(b)).is_some() && equal && iter.next().is_none()
    }
    fn each(self, mut emit: impl FnMut(u8)) -> Option<()> {
        let mut bytes = self.0.iter().copied();
        while let Some(b) = bytes.next() {
            if b != b'\\' {
                emit(b);
                continue;
            }
            match bytes.next()? {
                b'"' => emit(b'"'),
                b'\\' => emit(b'\\'),
                b'/' => emit(b'/'),
                b'b' => emit(8),
                b'f' => emit(12),
                b'n' => emit(b'\n'),
                b'r' => emit(b'\r'),
                b't' => emit(b'\t'),
                b'u' => {
                    let first = hex4(&mut bytes)?;
                    let scalar = if (0xd800..=0xdbff).contains(&first) {
                        if bytes.next()? != b'\\' || bytes.next()? != b'u' {
                            return None;
                        }
                        let second = hex4(&mut bytes)?;
                        if !(0xdc00..=0xdfff).contains(&second) {
                            return None;
                        }
                        0x1_0000 + (first - 0xd800) * 1024 + second - 0xdc00
                    } else {
                        first
                    };
                    let ch = char::from_u32(scalar)?;
                    let mut buf = [0; 4];
                    for b in ch.encode_utf8(&mut buf).bytes() {
                        emit(b);
                    }
                }
                _ => return None,
            }
        }
        Some(())
    }
}
fn hex4(bytes: &mut impl Iterator<Item = u8>) -> Option<u32> {
    (0..4).try_fold(0, |n, _| {
        Some(n * 16 + char::from(bytes.next()?).to_digit(16)?)
    })
}
#[cfg(test)]
#[path = "head_tests.rs"]
mod tests;
