//! Bounded stack formatting: the core's Python float-repr algorithm without
//! heap strings, plus decimal integers. A scientific double needs at most
//! 24 bytes; the final Python spelling fits in 32. No input-sized allocation.
use super::pyjson::Sink;
use std::fmt::{self, Write};

#[derive(Clone, Copy)]
struct Text {
    bytes: [u8; 64],
    len: usize,
}
impl Text {
    fn new() -> Self {
        Self {
            bytes: [0; 64],
            len: 0,
        }
    }
    fn as_str(&self) -> &str {
        std::str::from_utf8(self.bytes.get(..self.len).unwrap_or_default()).unwrap_or_default()
    }
}
impl Write for Text {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let end = self.len.checked_add(text.len()).ok_or(fmt::Error)?;
        let dst = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
        dst.copy_from_slice(text.as_bytes());
        self.len = end;
        Ok(())
    }
}
pub(super) fn int(i: i128, out: &mut impl Sink) {
    let mut text = Text::new();
    let _ = write!(text, "{i}");
    out.put(text.as_str().as_bytes());
}
pub(super) fn float(x: f64, out: &mut impl Sink) {
    if x.is_nan() {
        out.put(b"NaN");
        return;
    }
    if x.is_infinite() {
        out.put(if x < 0. { b"-Infinity" } else { b"Infinity" });
        return;
    }
    let mut sci = Text::new();
    let _ = write!(sci, "{x:e}");
    let len = sci
        .as_str()
        .split('e')
        .next()
        .map_or(0, |m| m.bytes().filter(u8::is_ascii_digit).count());
    if len > 1 {
        let mut nearest = Text::new();
        let _ = write!(nearest, "{:.*e}", len - 1, x);
        if nearest.as_str().parse::<f64>() == Ok(x) {
            sci = nearest;
        }
    }
    let (mant, exp) = sci.as_str().split_once('e').unwrap_or((sci.as_str(), "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (neg, mant) = mant.strip_prefix('-').map_or((false, mant), |m| (true, m));
    let mut digits = Text::new();
    for c in mant.chars().filter(char::is_ascii_digit) {
        let _ = digits.write_char(c);
    }
    let digits = digits.as_str();
    if neg {
        out.put(b"-");
    }
    if (-4..16).contains(&exp) {
        let point = exp + 1;
        if point <= 0 {
            out.put(b"0.");
            for _ in 0..-point {
                out.put(b"0");
            }
            out.put(digits.as_bytes());
        } else {
            let p = usize::try_from(point).unwrap_or(0);
            if digits.len() <= p {
                out.put(digits.as_bytes());
                for _ in digits.len()..p {
                    out.put(b"0");
                }
                out.put(b".0");
            } else {
                out.put(digits.get(..p).unwrap_or_default().as_bytes());
                out.put(b".");
                out.put(digits.get(p..).unwrap_or_default().as_bytes());
            }
        }
    } else {
        out.put(digits.get(..1).unwrap_or_default().as_bytes());
        if digits.len() > 1 {
            out.put(b".");
            out.put(digits.get(1..).unwrap_or_default().as_bytes());
        }
        out.put(if exp < 0 { b"e-" } else { b"e+" });
        let mut text = Text::new();
        let _ = write!(text, "{:02}", exp.unsigned_abs());
        out.put(text.as_str().as_bytes());
    }
}
