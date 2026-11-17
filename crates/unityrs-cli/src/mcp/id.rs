//! JSON-RPC IDs: exact integrality without a machine-integer range bound,
//! and original wire spelling kept independently of serde's number normalization.
use serde_json::{Value, value::RawValue};

#[derive(Clone)]
pub struct Id(Box<RawValue>);
impl Id {
    pub fn null() -> Self {
        Self(RawValue::NULL.to_owned())
    }
    pub fn from_token(token: &[u8]) -> serde_json::Result<Self> {
        serde_json::from_slice(token).map(Self)
    }
    pub fn text(&self) -> &str {
        self.0.get()
    }
}
pub fn valid(value: &Value) -> bool {
    value.is_string() || value.as_number().is_some_and(|n| integral(n.as_str()))
}
/// The input is a validated JSON number. Removing a decimal point requires
/// enough exponent places (or trailing zeroes) to leave no fractional digits.
/// Even exponents larger than i64 are decided in token-linear time and space.
fn integral(text: &str) -> bool {
    let text = text.strip_prefix('-').unwrap_or(text);
    let (mantissa, exponent) = text.split_once(['e', 'E']).unwrap_or((text, "0"));
    if mantissa.bytes().all(|b| b == b'0' || b == b'.') {
        return true;
    }
    let fraction = mantissa.split_once('.').map_or(0, |(_, s)| s.len());
    let zeroes = mantissa
        .bytes()
        .rev()
        .filter(|b| *b != b'.')
        .take_while(|b| *b == b'0')
        .count();
    match exponent.parse::<i64>() {
        Ok(exponent) => i128::from(exponent) >= fraction as i128 - zeroes as i128,
        Err(_) => !exponent.starts_with('-'),
    }
}
