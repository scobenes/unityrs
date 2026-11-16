// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/ShaderConverter.py. See THIRD_PARTY_LICENSES.
//! Big-endian blobs and script replacement: export/ShaderConverter.py:41-50,
//! export/ShaderConverter.py:543-568. Invalid layouts are Tier U; a valid
//! string script bypasses UnityPy's `bytes(str)` `TypeError` (Tier C).
use super::{Error, Output, Program, Unsupported, View, bytes};
use std::sync::Arc;
use unityrs_core::{
    Value,
    budget::DecodeBuf,
    codec::{Codec, Lz4, decode_block},
    error::LimitKind,
    limits::{Meter, MeteredVec},
};
const MARKER: &str = "GpuProgramIndex ";

pub(super) fn export(
    root: View<'_>,
    version: (u32, u32),
    meter: &Arc<Meter>,
    cap: u64,
) -> Result<MeteredVec<u8>, Error> {
    let refusal = if version == (5, 3) {
        Unsupported::BigEndian53Blob
    } else {
        Unsupported::BigEndian54Blob
    };
    let map = |error| match error {
        Error::Charge(_) | Error::Core(_) => error,
        _ => Error::Unsupported(refusal),
    };
    let input = bytes(root.field("m_SubProgramBlob")?.value, meter).map_err(map)?;
    let size = root
        .field("decompressedSize")
        .and_then(View::int)
        .map_err(map)?;
    let size = u32::try_from(size).map_err(|_| Error::Unsupported(refusal))?;
    meter.count_decode((input.len() as u64).saturating_mul(2))?;
    let mut buf = DecodeBuf::new(meter, 0, cap, LimitKind::BlockOutput)?;
    decode_block(Codec::Lz4, &Lz4, &input, size, &mut buf, cap)?;
    let program = Program::legacy(buf, meter).map_err(map)?;
    let script = root.optional("m_Script")?.ok_or(Error::NoneBytes)?;
    let converted;
    let script = if let Value::Str(s) = script.value {
        s.as_bytes()
    } else {
        converted = bytes(script.value, meter)?;
        converted.as_ref()
    };
    // UTF-8, marker/line searches, whitespace trimming and decimal syntax.
    // Each digit also searches the fixed Unicode Nd table (at most 7 steps).
    meter.count_decode((script.len() as u64).saturating_mul(16))?;
    let mut rest = std::str::from_utf8(script).map_err(|_| super::utf8_error(script, meter))?;
    let mut out = Output::new(meter);
    out.text(super::HEADER)?;
    while let Some(start) = rest.find(MARKER) {
        out.text(&rest[..start])?;
        rest = &rest[start..];
        let end = rest.find('\n').unwrap_or(rest.len());
        let number = &rest[MARKER.len()..end];
        // Python's .+ leaves a marker with no following character alone.
        if number.is_empty() {
            out.text(MARKER)?;
        } else {
            let index = index(number, program.subprogram_count())
                .ok_or(Error::Unsupported(Unsupported::GpuProgramIndex))?;
            program.write(index, &mut out)?;
        }
        rest = &rest[end..];
    }
    out.text(rest)?;
    Ok(out.bytes)
}

fn index(number: &str, capacity: usize) -> Option<i64> {
    let number =
        number.trim_matches(|c: char| c.is_whitespace() && !('\u{1c}'..='\u{1f}').contains(&c));
    let (negative, number) = if let Some(n) = number.strip_prefix('-') {
        (true, n)
    } else {
        (false, number.strip_prefix('+').unwrap_or(number))
    };
    let mut digits = 0;
    let mut previous_digit = false;
    let mut value = 0u64;
    for c in number.chars() {
        if c == '_' {
            if !previous_digit {
                return None;
            }
            previous_digit = false;
        } else {
            let digit = unityrs_core::tables::nd_digit(c)?;
            digits += 1;
            if digits > 4300 {
                return None;
            }
            // Saturation preserves syntax validation even for huge values.
            value = value.saturating_mul(10).saturating_add(u64::from(digit));
            previous_digit = true;
        }
    }
    if !previous_digit {
        return None;
    }
    let value = i64::try_from(value).ok()?;
    let value = if negative { -value } else { value };
    let count = i64::try_from(capacity).ok()?;
    (value >= -count && value < count).then_some(value)
}
