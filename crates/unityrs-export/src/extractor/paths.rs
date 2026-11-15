// Ported from UnityPy 1.25.2 (MIT): tools/extractor.py. See THIRD_PARTY_LICENSES.
//! Naming from tools/extractor.py:25-78 and tools/extractor.py:81-157.
//! Safety policy: validate before dropping directories; reject parent, NUL,
//! absolute and backslash-bearing paths. Empty slash components are ignored.
use super::Error;
use std::sync::Arc;
use unityrs_core::{
    RawStr,
    limits::{Meter, MeteredVec},
};

/// Validate an untrusted relative output path, including its original prefix.
pub fn validate(path: &[u8]) -> Result<(), Error> {
    if path.starts_with(b"/")
        || path.contains(&0)
        || path.contains(&b'\\')
        || path
            .split(|b| *b == b'/')
            .any(|s| s == b".." || s.contains(&b':'))
    {
        return Err(Error::UnsafePath);
    }
    Ok(())
}
/// Slash split, Python slicing (negative counts included), empty parts omitted.
pub fn container(path: &[u8], ignore: i64, meter: &Arc<Meter>) -> Result<RawStr, Error> {
    validate(path)?;
    meter.count_decode(path.len() as u64)?;
    let count = path.split(|b| *b == b'/').count();
    let start = if ignore < 0 {
        count.saturating_sub(usize::try_from(ignore.unsigned_abs()).unwrap_or(usize::MAX))
    } else {
        usize::try_from(ignore).unwrap_or(usize::MAX)
    };
    let mut out = MeteredVec::new(meter);
    for part in path
        .split(|b| *b == b'/')
        .skip(start)
        .filter(|s| !s.is_empty())
    {
        if !out.is_empty() {
            out.push(b'/')?;
        }
        out.extend_from_slice(part)?;
    }
    if out.is_empty() {
        return Err(Error::UnsafePath);
    }
    Ok(RawStr::charged(out.as_slice(), meter)?)
}
/// POSIX splitext: a leading run of dots alone is part of the stem.
#[must_use]
pub fn split(path: &[u8]) -> (&[u8], &[u8]) {
    let base = path.iter().rposition(|b| *b == b'/').map_or(0, |i| i + 1);
    if let Some(dot) = path.iter().rposition(|b| *b == b'.').filter(|i| *i >= base)
        && path
            .get(base..dot)
            .unwrap_or_default()
            .iter()
            .any(|b| *b != b'.')
    {
        return (
            path.get(..dot).unwrap_or_default(),
            path.get(dot..).unwrap_or_default(),
        );
    }
    (path, b"")
}
pub(super) fn join(a: &[u8], b: &[u8], meter: &Arc<Meter>) -> Result<RawStr, Error> {
    validate(b)?;
    Ok(RawStr::charged_concat(
        &[a, if a.is_empty() { b"" } else { b"/" }, b],
        meter,
    )?)
}
pub(super) fn suffix(stem: &[u8], ext: &[u8], meter: &Arc<Meter>) -> Result<RawStr, Error> {
    let result = RawStr::charged_concat(&[stem, ext], meter)?;
    validate(result.as_bytes())?;
    Ok(result)
}
