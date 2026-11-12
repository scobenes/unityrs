//! Python's `repr` of a float, which the dump's `str()` of an index value
//! and the cutter's JSON (`json.dumps` writes a float as its `repr`) both
//! give.

use std::fmt::Write as _;

/// `repr(float)`: the shortest digits that round-trip, positional when the
/// decimal exponent is in -4..16, else `d.ddde+XX` (`PyOS_double_to_string`
/// with mode `'r'`); `nan`, `inf` and `-inf`.
///
/// Of the shortest digits that round-trip, Python's `repr` (David Gay's
/// `dtoa` in mode 0) gives those nearest the double, and of two equally
/// near the even one. Rust's shortest `{:e}` breaks that tie upwards
/// (`1551563132437320.25`: Python `…320.2`, `{:e}` `…320.3`; review round
/// 8), so its length only is taken: the double rounded to that many digits
/// (`{:.*e}`: exact, ties to even) is the nearest of that length, and is
/// Python's when it round-trips. When it does not (a power of two, whose
/// doubles that round-trip reach half as far below it as above, can have
/// its nearest digits below them), `{:e}`'s digits are kept: they are then
/// the nearest that round-trip, as Python's are, with no tie between two
/// (`crates/unityrs-dump/tests/fixtures/dump/index.json` "str" holds ties
/// and such powers, which the tests here repeat).
#[must_use]
pub fn py_float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".to_owned();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf" } else { "-inf" }.to_owned();
    }
    // `{:e}` gives the shortest round-trip digits: "-1.2345e-7".
    let mut sci = format!("{x:e}");
    let len = sci
        .split('e')
        .next()
        .map_or(0, |m| m.bytes().filter(u8::is_ascii_digit).count());
    if len > 1 {
        let nearest = format!("{:.*e}", len - 1, x);
        if nearest.parse::<f64>() == Ok(x) {
            sci = nearest;
        }
    }
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (neg, mant) = mant.strip_prefix('-').map_or((false, mant), |m| (true, m));
    let digits: String = mant.chars().filter(char::is_ascii_digit).collect();
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if (-4..16).contains(&exp) {
        let point = exp + 1; // digits before the decimal point
        if point <= 0 {
            out.push_str("0.");
            for _ in 0..-point {
                out.push('0');
            }
            out.push_str(&digits);
        } else {
            let p = usize::try_from(point).unwrap_or(0);
            if digits.len() <= p {
                out.push_str(&digits);
                for _ in digits.len()..p {
                    out.push('0');
                }
                out.push_str(".0");
            } else {
                out.push_str(digits.get(..p).unwrap_or_default());
                out.push('.');
                out.push_str(digits.get(p..).unwrap_or_default());
            }
        }
    } else {
        out.push_str(digits.get(..1).unwrap_or_default());
        if digits.len() > 1 {
            out.push('.');
            out.push_str(digits.get(1..).unwrap_or_default());
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        let _ = write!(out, "{:02}", exp.unsigned_abs());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::py_float_repr;

    #[test]
    fn floats_repr_as_python() {
        // CPython 3.11's `repr`; the ties and the powers of two are the dump
        // fixture's (`index.json` "str").
        for (x, s) in [
            (4100.0, "4100.0"),
            (1e16, "1e+16"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (-2.5, "-2.5"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.5e300, "1.5e+300"),
            (f64::NAN, "nan"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (5e-324, "5e-324"),
        ] {
            assert_eq!(py_float_repr(x), s, "{x:e}");
        }
    }

    #[test]
    fn ties_go_to_the_even_digits_and_powers_of_two_to_the_nearest() {
        // Doubles written exactly: two shortest digit strings equally near,
        // where CPython takes the even one (`{:e}` broke the tie upwards),
        // and powers of two, whose nearest shortest digits can fail to read
        // back (the dump fixture's `index.json` "str" cases).
        for (exact, want) in [
            ("1234567890123456.0", "1234567890123456.0"),
            ("1551563132437320.25", "1551563132437320.2"),
            ("-1551563132437320.25", "-1551563132437320.2"),
            ("79686618578806.625", "79686618578806.62"),
            ("1125899906842624.25", "1125899906842624.2"),
            ("22620844907483.3125", "22620844907483.312"),
            ("2.98023223876953125e-8", "2.9802322387695312e-08"),
            ("5.9604644775390625e-7", "5.960464477539062e-07"),
            ("9007199254740992.0", "9007199254740992.0"),
        ] {
            let x: f64 = exact.parse().unwrap();
            assert_eq!(py_float_repr(x), want, "{exact}");
        }
    }
}
