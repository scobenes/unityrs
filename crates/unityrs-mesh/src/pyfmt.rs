//! CPython-compatible precision formats, using Rust's exact ties-to-even
//! decimal rounding. No shortest-roundtrip conversion or heap allocation.
use std::fmt::{self, Write};

pub use unityrs_meshdecode::Component as PyNum;

/// Stack space for .4f of any binary64 (sign, 309 digits, dot, 4 digits).
pub struct PyFmtBuf {
    bytes: [u8; 320],
    len: usize,
}
const _: () = crate::send_sync::<PyFmtBuf>();
impl Default for PyFmtBuf {
    fn default() -> Self {
        Self {
            bytes: [0; 320],
            len: 0,
        }
    }
}
impl Write for PyFmtBuf {
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
impl PyFmtBuf {
    fn text(&self) -> &str {
        // Only ASCII formatting writes this buffer.
        std::str::from_utf8(self.bytes.get(..self.len).unwrap_or_default()).unwrap_or_default()
    }
}
/// Python's format(x, '.4f'), including unsigned NaNs and signed zero.
pub fn f4(x: f64, buf: &mut PyFmtBuf) -> &str {
    buf.len = 0;
    if x.is_nan() {
        let _ = buf.write_str("nan");
    } else {
        let _ = write!(buf, "{x:.4}");
    }
    buf.text()
}
/// Python's format(x, '.9G'). An int converts to binary64 as `CPython` does.
#[allow(
    clippy::if_not_else,
    reason = "scientific form first matches the precision rule"
)]
pub fn g9(x: PyNum, buf: &mut PyFmtBuf) -> &str {
    let x = x.as_f64();
    buf.len = 0;
    if x.is_nan() {
        let _ = buf.write_str("NAN");
    } else if x.is_infinite() {
        let _ = buf.write_str(if x.is_sign_negative() { "-INF" } else { "INF" });
    } else {
        let mut sci = PyFmtBuf::default();
        let _ = write!(sci, "{:.8e}", x.abs());
        let (mant, exp) = sci.text().split_once('e').unwrap_or(("0", "0"));
        let exp: i32 = exp.parse().unwrap_or(0);
        let mut digits = [0u8; 9];
        let mut n = 0;
        for d in mant.bytes().filter(u8::is_ascii_digit) {
            if let Some(slot) = digits.get_mut(n) {
                *slot = d;
                n += 1;
            }
        }
        while n > 1 && digits.get(n - 1) == Some(&b'0') {
            n -= 1;
        }
        if x.is_sign_negative() {
            let _ = buf.write_char('-');
        }
        let d = std::str::from_utf8(digits.get(..n).unwrap_or_default()).unwrap_or("0");
        if !(-4..9).contains(&exp) {
            let _ = buf.write_str(d.get(..1).unwrap_or("0"));
            if n > 1 {
                let _ = buf.write_char('.');
                let _ = buf.write_str(d.get(1..).unwrap_or_default());
            }
            let _ = write!(
                buf,
                "E{}{:02}",
                if exp < 0 { '-' } else { '+' },
                exp.unsigned_abs()
            );
        } else {
            let point = exp + 1;
            if point <= 0 {
                let _ = buf.write_str("0.");
                for _ in 0..-point {
                    let _ = buf.write_char('0');
                }
                let _ = buf.write_str(d);
            } else {
                let point = usize::try_from(point).unwrap_or(0);
                for i in 0..point.max(n) {
                    if i == point {
                        let _ = buf.write_char('.');
                    }
                    let _ = buf.write_char(char::from(
                        digits.get(i).copied().filter(|_| i < n).unwrap_or(b'0'),
                    ));
                }
            }
        }
    }
    buf.text()
}
