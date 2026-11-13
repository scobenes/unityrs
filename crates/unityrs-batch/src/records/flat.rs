//! Allocation-free decoder for the artifact's flat JSON objects. Strings
//! unescape in place (never expand); fixed field slots reject nesting,
//! unknown fields and duplicate names. All integers are checked by consumers.
use crate::phase::Error;
use std::{ops::Range, str::FromStr};

const NAMES: &[&str] = &[
    "type",
    "key",
    "unit",
    "input",
    "serialized_file",
    "kind",
    "path_id",
    "md5",
    "crc32",
    "visible_hash",
    "mode",
    "size",
    "etag",
    "file",
    "cut",
    "archive",
    "path",
    "reason",
    "detail",
    "class",
    "output",
    "sprite",
    "bundle",
    "name",
    "id",
    "status",
    "input_peak",
    "dependency_peak",
    "evictions",
    "export_work",
    "decode_work_peak",
    "ms",
    "skips",
    "unresolved_scripts",
    "spooled_part",
    "claimed_bytes",
];
#[derive(Clone, Copy)]
enum Atom {
    String(usize, usize),
    Number(usize, usize),
    Bool(bool),
    Null,
}
pub(super) struct Fields {
    values: [Option<Atom>; NAMES.len()],
}
impl Fields {
    pub fn parse(bytes: &mut [u8]) -> Result<Self, Error> {
        let mut parser = Parser { bytes, offset: 0 };
        let mut fields = Self {
            values: [None; NAMES.len()],
        };
        parser.token(b'{')?;
        loop {
            parser.whitespace();
            let name = parser.string()?;
            let name = std::str::from_utf8(parser.bytes.get(name).ok_or_else(Error::internal)?)
                .map_err(|_| Error::internal())?;
            let index = NAMES
                .iter()
                .position(|&n| n == name)
                .ok_or_else(Error::internal)?;
            parser.token(b':')?;
            let atom = parser.atom()?;
            if fields
                .values
                .get_mut(index)
                .ok_or_else(Error::internal)?
                .replace(atom)
                .is_some()
            {
                return Err(Error::internal());
            }
            parser.whitespace();
            match parser.byte()? {
                b',' => {}
                b'}' => break,
                _ => return Err(Error::internal()),
            }
        }
        parser.whitespace();
        if parser.offset != parser.bytes.len() {
            return Err(Error::internal());
        }
        Ok(fields)
    }
    fn get(&self, name: &str) -> Result<Atom, Error> {
        let index = NAMES
            .iter()
            .position(|&n| n == name)
            .ok_or_else(Error::internal)?;
        self.values
            .get(index)
            .copied()
            .flatten()
            .ok_or_else(Error::internal)
    }
    pub fn shape(&self, expected: &[&str]) -> Result<(), Error> {
        if self.values.iter().flatten().count() != expected.len() {
            return Err(Error::internal());
        }
        for name in expected {
            self.get(name)?;
        }
        Ok(())
    }
    pub fn text<'a>(&self, bytes: &'a [u8], name: &str) -> Result<&'a str, Error> {
        let Atom::String(start, end) = self.get(name)? else {
            return Err(Error::internal());
        };
        std::str::from_utf8(bytes.get(start..end).ok_or_else(Error::internal)?)
            .map_err(|_| Error::internal())
    }
    pub fn optional_text<'a>(&self, bytes: &'a [u8], name: &str) -> Result<Option<&'a str>, Error> {
        if matches!(self.get(name)?, Atom::Null) {
            Ok(None)
        } else {
            self.text(bytes, name).map(Some)
        }
    }
    pub fn number<T: FromStr>(&self, bytes: &[u8], name: &str) -> Result<T, Error> {
        let Atom::Number(start, end) = self.get(name)? else {
            return Err(Error::internal());
        };
        std::str::from_utf8(bytes.get(start..end).ok_or_else(Error::internal)?)
            .map_err(|_| Error::internal())?
            .parse()
            .map_err(|_| Error::internal())
    }
    pub fn optional_number<T: FromStr>(
        &self,
        bytes: &[u8],
        name: &str,
    ) -> Result<Option<T>, Error> {
        if matches!(self.get(name)?, Atom::Null) {
            Ok(None)
        } else {
            self.number(bytes, name).map(Some)
        }
    }
    pub fn boolean(&self, name: &str) -> Result<bool, Error> {
        match self.get(name)? {
            Atom::Bool(b) => Ok(b),
            _ => Err(Error::internal()),
        }
    }
}
struct Parser<'a> {
    bytes: &'a mut [u8],
    offset: usize,
}
impl Parser<'_> {
    fn byte(&mut self) -> Result<u8, Error> {
        let b = self
            .bytes
            .get(self.offset)
            .copied()
            .ok_or_else(Error::internal)?;
        self.offset += 1;
        Ok(b)
    }
    fn whitespace(&mut self) {
        while self
            .bytes
            .get(self.offset)
            .is_some_and(|b| matches!(b, b' ' | b'\r' | b'\n' | b'\t'))
        {
            self.offset += 1;
        }
    }
    fn token(&mut self, token: u8) -> Result<(), Error> {
        self.whitespace();
        if self.byte()? == token {
            Ok(())
        } else {
            Err(Error::internal())
        }
    }
    fn literal(&mut self, bytes: &[u8]) -> Result<(), Error> {
        for &b in bytes {
            if self.byte()? != b {
                return Err(Error::internal());
            }
        }
        Ok(())
    }
    fn atom(&mut self) -> Result<Atom, Error> {
        self.whitespace();
        match self
            .bytes
            .get(self.offset)
            .copied()
            .ok_or_else(Error::internal)?
        {
            b'"' => {
                let r = self.string()?;
                Ok(Atom::String(r.start, r.end))
            }
            b't' => {
                self.literal(b"true")?;
                Ok(Atom::Bool(true))
            }
            b'f' => {
                self.literal(b"false")?;
                Ok(Atom::Bool(false))
            }
            b'n' => {
                self.literal(b"null")?;
                Ok(Atom::Null)
            }
            b'-' | b'0'..=b'9' => {
                let start = self.offset;
                if self.bytes.get(self.offset) == Some(&b'-') {
                    self.offset += 1;
                }
                match self.byte()? {
                    b'0' => {}
                    b'1'..=b'9' => {
                        while self.bytes.get(self.offset).is_some_and(u8::is_ascii_digit) {
                            self.offset += 1;
                        }
                    }
                    _ => return Err(Error::internal()),
                }
                Ok(Atom::Number(start, self.offset))
            }
            _ => Err(Error::internal()),
        }
    }
    fn hex(&mut self) -> Result<u32, Error> {
        let mut value = 0;
        for _ in 0..4 {
            let n = match self.byte()? {
                b @ b'0'..=b'9' => b - b'0',
                b @ b'a'..=b'f' => b - b'a' + 10,
                b @ b'A'..=b'F' => b - b'A' + 10,
                _ => return Err(Error::internal()),
            };
            value = value * 16 + u32::from(n);
        }
        Ok(value)
    }
    fn string(&mut self) -> Result<Range<usize>, Error> {
        if self.byte()? != b'"' {
            return Err(Error::internal());
        }
        let start = self.offset;
        let mut end = start;
        loop {
            let b = self.byte()?;
            let mut unicode = [0; 4];
            let single;
            let bytes = match b {
                b'"' => break,
                0..=31 => return Err(Error::internal()),
                b'\\' => {
                    single = match self.byte()? {
                        b'"' => b'"',
                        b'\\' => b'\\',
                        b'/' => b'/',
                        b'b' => 8,
                        b'f' => 12,
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        b'u' => {
                            let mut code = self.hex()?;
                            if (0xd800..=0xdbff).contains(&code) {
                                self.literal(b"\\u")?;
                                let low = self.hex()?;
                                if !(0xdc00..=0xdfff).contains(&low) {
                                    return Err(Error::internal());
                                }
                                code = 0x1_0000 + (code - 0xd800) * 1024 + low - 0xdc00;
                            }
                            let text = char::from_u32(code)
                                .ok_or_else(Error::internal)?
                                .encode_utf8(&mut unicode);
                            for b in text.bytes() {
                                *self.bytes.get_mut(end).ok_or_else(Error::internal)? = b;
                                end += 1;
                            }
                            continue;
                        }
                        _ => return Err(Error::internal()),
                    };
                    std::slice::from_ref(&single)
                }
                b => {
                    single = b;
                    std::slice::from_ref(&single)
                }
            };
            for &b in bytes {
                *self.bytes.get_mut(end).ok_or_else(Error::internal)? = b;
                end += 1;
            }
        }
        std::str::from_utf8(self.bytes.get(start..end).ok_or_else(Error::internal)?)
            .map_err(|_| Error::internal())?;
        Ok(start..end)
    }
}
