// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/ShaderConverter.py. See THIRD_PARTY_LICENSES.
//! Bounded numeric formatting for export/ShaderConverter.py:314-372.
use super::Error;
use crate::pyjson::{self, Sink};
use std::{
    fmt::{self, Write},
    sync::Arc,
};
use unityrs_core::limits::{ChargeError, Meter, MeteredVec};

pub(super) struct Output<'a> {
    pub bytes: MeteredVec<u8>,
    pub int_properties: bool,
    meter: &'a Arc<Meter>,
    error: Option<ChargeError>,
}
impl<'a> Output<'a> {
    pub fn new(meter: &'a Arc<Meter>) -> Self {
        Self {
            bytes: MeteredVec::new(meter),
            int_properties: true,
            meter,
            error: None,
        }
    }
    pub fn text(&mut self, text: impl AsRef<[u8]>) -> Result<(), Error> {
        self.put(text.as_ref());
        self.check()?;
        Ok(())
    }
    pub fn work(&self, bytes: u64) -> Result<(), Error> {
        self.meter.count_decode(bytes)?;
        Ok(())
    }
    pub fn utf8_error(&self, bytes: &[u8]) -> Error {
        super::utf8_error(bytes, self.meter)
    }
    pub fn int(&mut self, n: i128) -> Result<(), Error> {
        pyjson::int(n, self);
        self.check()?;
        Ok(())
    }
    pub fn integer_property(&mut self, n: f64) -> Result<(), Error> {
        if !n.is_finite() || n.fract() != 0. {
            return self.float(n, false);
        }
        if n == 0. {
            return self.int(0);
        }
        // Fixed decimal formatting avoids saturating large values through an
        // integer cast. The stack buffer needs no allocation; text charges the
        // resulting bytes before growing the output, as for other numbers.
        let mut fixed = Stack::default();
        write!(fixed, "{n:.0}").map_err(|_| Error::Field("integer formatting"))?;
        self.text(fixed.str())
    }
    pub fn float(&mut self, n: f64, general: bool) -> Result<(), Error> {
        if n.is_nan() {
            return self.text("nan");
        }
        if n.is_infinite() {
            return self.text(if n.is_sign_negative() { "-inf" } else { "inf" });
        }
        if !general {
            pyjson::float(n, self);
            self.check()?;
            return Ok(());
        }
        let mut sci = Stack::default();
        write!(sci, "{n:.5e}").map_err(|_| Error::Field("float formatting"))?;
        let (mant, exp) = sci
            .str()
            .split_once('e')
            .ok_or(Error::Field("float exponent"))?;
        let exp: i32 = exp.parse().map_err(|_| Error::Field("float exponent"))?;
        let mant = mant.trim_end_matches('0').trim_end_matches('.');
        if !(-4..6).contains(&exp) {
            self.text(mant)?;
            self.text("e")?;
            self.text(if exp < 0 { "-" } else { "+" })?;
            if exp.abs() < 10 {
                self.text("0")?;
            }
            return self.int(i128::from(exp.abs()));
        }
        let mut fixed = Stack::default();
        let places = usize::try_from((5 - exp).max(0)).unwrap_or(0);
        write!(fixed, "{n:.places$}").map_err(|_| Error::Field("float formatting"))?;
        let mut text = fixed.str();
        if text.contains('.') {
            text = text.trim_end_matches('0').trim_end_matches('.');
        }
        self.text(text)
    }
}
impl Sink for Output<'_> {
    fn put(&mut self, bytes: &[u8]) {
        if self.error.is_none() {
            self.error = self
                .meter
                .count_decode(bytes.len() as u64)
                .and_then(|()| self.bytes.extend_from_slice(bytes))
                .err();
        }
    }
    fn check(&self) -> Result<(), ChargeError> {
        self.error.clone().map_or(Ok(()), Err)
    }
}
struct Stack {
    // A finite f64 in fixed decimal needs at most 309 digits and a sign.
    data: [u8; 384],
    len: usize,
}
impl Default for Stack {
    fn default() -> Self {
        Self {
            data: [0; 384],
            len: 0,
        }
    }
}
impl Stack {
    fn str(&self) -> &str {
        std::str::from_utf8(self.data.get(..self.len).unwrap_or_default()).unwrap_or_default()
    }
}
impl Write for Stack {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let end = self.len.checked_add(s.len()).ok_or(fmt::Error)?;
        self.data
            .get_mut(self.len..end)
            .ok_or(fmt::Error)?
            .copy_from_slice(s.as_bytes());
        self.len = end;
        Ok(())
    }
}
