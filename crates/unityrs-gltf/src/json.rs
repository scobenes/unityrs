//! The same serializer measures and emits; no format operation grows storage.
use crate::{GltfError, validate};
use std::fmt::{self, Write};
use unityrs_core::limits::{Meter, MeteredVec};

pub(crate) struct Json<'a> {
    pub len: usize,
    out: Option<&'a mut MeteredVec<u8>>,
    meter: &'a Meter,
}
impl<'a> Json<'a> {
    pub fn new(meter: &'a Meter, out: Option<&'a mut MeteredVec<u8>>) -> Self {
        Self { len: 0, out, meter }
    }
    pub fn work(&self, n: usize) -> Result<(), GltfError> {
        Ok(self.meter.count_decode(n as u64)?)
    }
    pub fn raw(&mut self, s: &str) -> Result<(), GltfError> {
        self.meter.count_decode(s.len() as u64 + 1)?;
        self.len = self
            .len
            .checked_add(s.len())
            .ok_or_else(|| validate::invalid("JSON length overflow"))?;
        if let Some(out) = &mut self.out {
            out.extend_from_slice(s.as_bytes())?;
        }
        Ok(())
    }
    pub fn number(&mut self, value: impl fmt::Display) -> Result<(), GltfError> {
        self.meter.count_decode(384)?;
        // Largest finite f64 in Display's fixed representation needs < 330 bytes.
        let mut buf = Number {
            bytes: [0; 384],
            len: 0,
        };
        write!(buf, "{value}").map_err(|_| validate::invalid("number formatting"))?;
        let bytes = buf
            .bytes
            .get(..buf.len)
            .ok_or_else(|| validate::invalid("number buffer"))?;
        self.raw(std::str::from_utf8(bytes).map_err(|_| validate::invalid("number UTF-8"))?)
    }
    pub fn float(&mut self, x: f64) -> Result<(), GltfError> {
        if !x.is_finite() {
            return Err(validate::invalid("non-finite JSON number"));
        }
        self.number(if x == 0. { 0. } else { x })
    }
    pub fn string(&mut self, bytes: &[u8]) -> Result<(), GltfError> {
        self.meter.count_decode(bytes.len() as u64)?;
        let s = std::str::from_utf8(bytes).map_err(|_| validate::invalid("text UTF-8"))?;
        self.raw("\"")?;
        for c in s.chars() {
            match c {
                '"' => self.raw("\\\"")?,
                '\\' => self.raw("\\\\")?,
                '\u{00}'..='\u{1f}' => {
                    let mut buf = Number {
                        bytes: [0; 384],
                        len: 0,
                    };
                    write!(buf, "\\u{:04x}", u32::from(c))
                        .map_err(|_| validate::invalid("escape formatting"))?;
                    self.raw(
                        std::str::from_utf8(
                            buf.bytes
                                .get(..buf.len)
                                .ok_or_else(|| validate::invalid("escape buffer"))?,
                        )
                        .map_err(|_| validate::invalid("escape UTF-8"))?,
                    )?;
                }
                _ => {
                    let mut buf = [0; 4];
                    self.raw(c.encode_utf8(&mut buf))?;
                }
            }
        }
        self.raw("\"")
    }
    pub fn indices(&mut self, values: &[usize]) -> Result<(), GltfError> {
        self.raw("[")?;
        for (i, v) in values.iter().enumerate() {
            if i > 0 {
                self.raw(",")?;
            }
            self.number(v)?;
        }
        self.raw("]")
    }
    pub fn floats(&mut self, values: &[f64]) -> Result<(), GltfError> {
        self.raw("[")?;
        for (i, &v) in values.iter().enumerate() {
            if i > 0 {
                self.raw(",")?;
            }
            self.float(v)?;
        }
        self.raw("]")
    }
}
struct Number {
    bytes: [u8; 384],
    len: usize,
}
impl Write for Number {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(fmt::Error)?;
        self.bytes
            .get_mut(self.len..end)
            .ok_or(fmt::Error)?
            .copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}
