//! Exact JSON integer normalization. Work and storage are bounded by token text;
//! huge exponents never cause exponent-sized allocations or loops.
use serde_json::Value;

pub fn unsigned(value: &Value) -> Option<u64> {
    u64::try_from(parse(value)?).ok()
}

pub fn parse(value: &Value) -> Option<i128> {
    let text = value.as_number()?.to_string();
    let (negative, text) = text
        .strip_prefix('-')
        .map_or((false, text.as_str()), |s| (true, s));
    let (mantissa, exponent) = text.split_once(['e', 'E']).unwrap_or((text, "0"));
    let fraction = mantissa.split_once('.').map_or(0, |(_, s)| s.len());
    let digits: String = mantissa.chars().filter(|&c| c != '.').collect();
    let digits = digits.trim_start_matches('0');
    if digits.is_empty() {
        return Some(0);
    }
    let exponent = exponent.parse::<i64>().ok()?;
    let shift = exponent.checked_sub(i64::try_from(fraction).ok()?)?;
    let retained = if shift < 0 {
        let end = digits
            .len()
            .checked_sub(usize::try_from(shift.checked_neg()?).ok()?)?;
        if !digits.get(end..)?.bytes().all(|b| b == b'0') {
            return None;
        }
        digits.get(..end)?
    } else {
        digits
    };
    // No argument accepts more than 20 significant decimal digits. Keeping
    // i128 here also lets range checks diagnose adjacent out-of-range values.
    if retained
        .len()
        .saturating_add(usize::try_from(shift.max(0)).ok()?)
        > 39
    {
        return None;
    }
    let mut number = 0i128;
    for digit in retained.bytes() {
        let digit = i128::from(digit.checked_sub(b'0')?);
        number = number.checked_mul(10)?;
        number = if negative {
            number.checked_sub(digit)?
        } else {
            number.checked_add(digit)?
        };
    }
    for _ in 0..shift.max(0) {
        number = number.checked_mul(10)?;
    }
    Some(number)
}
