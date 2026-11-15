// Ported from UnityPy 1.25.2 (MIT): UnityPy/environment.py. See THIRD_PARTY_LICENSES.
//! Split joining (`environment.py:89-98`), with PARITY section 19.2's
//! terminal-suffix rule. The caller supplies paths; no resolver is walked.

use crate::budget::DecodeBuf;
use crate::error::{Error, LimitKind};
use crate::limits::{Limits, Meter};
use crate::raw::RawStr;
use crate::tables::nd_digit;
use bytes::Bytes;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

/// A terminal `.split` plus Python decimal digits, with a nonempty basename.
/// Earlier occurrences do not consume the rest of the path (Tier B/C).
pub(crate) fn base(path: &[u8]) -> Option<&[u8]> {
    let at = path.windows(6).rposition(|w| w == b".split")?;
    let prefix = path.get(..at)?;
    if prefix.is_empty() || prefix.last().is_some_and(|b| matches!(b, b'/' | b'\\')) {
        return None;
    }
    // The regex's `.*?` cannot cross a newline before a separator; its
    // final non-separator capture may itself contain newlines.
    if let Some(newline) = prefix.iter().position(|&b| b == b'\n')
        && prefix
            .iter()
            .rposition(|b| matches!(b, b'/' | b'\\'))
            .is_some_and(|sep| newline < sep)
    {
        return None;
    }
    let tail = std::str::from_utf8(path.get(at + 6..)?).ok()?;
    (!tail.is_empty() && tail.chars().all(|c| nd_digit(c).is_some())).then_some(prefix)
}

/// A numbered part's name, without formatting or an unpaid path buffer.
pub(crate) fn part(base: &[u8], i: u16, meter: &Arc<Meter>) -> Result<RawStr, Error> {
    let digits = [
        b'0' + u8::try_from((i / 100) % 10).unwrap_or(0),
        b'0' + ((i / 10) % 10) as u8,
        b'0' + (i % 10) as u8,
    ];
    let start = if i >= 100 {
        0
    } else if i >= 10 {
        1
    } else {
        2
    };
    Ok(RawStr::charged_concat(
        &[base, b".split", digits.get(start..).unwrap_or_default()],
        meter,
    )?)
}

/// Borrow a path from its raw name, preserving Unix filesystem bytes.
#[allow(clippy::unnecessary_wraps, reason = "non-Unix path decoding can fail")]
pub(crate) fn path(name: &[u8]) -> Result<&Path, Error> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(Path::new(std::ffi::OsStr::from_bytes(name)))
    }
    #[cfg(not(unix))]
    {
        Ok(Path::new(std::str::from_utf8(name).map_err(|_| {
            Error::Malformed {
                what: "path",
                detail: "invalid path encoding".into(),
            }
        })?))
    }
}

/// The parts form one bounded source. A missing zero is skipped; the first
/// later gap stops the walk, at most 999 parts (`environment.py:91-98`).
pub(crate) fn read(
    base: &[u8],
    root: Option<&Path>,
    limits: &Limits,
    meter: &Arc<Meter>,
) -> Result<Bytes, Error> {
    let mut out = DecodeBuf::new(meter, 0, limits.max_input_bytes, LimitKind::InputBytes)?;
    for i in 0..999 {
        let name = part(base, i, meter)?;
        // fs.exists and fs.open both normalise each generated part name
        // (environment.py:91-98). The base used as a key stays untouched.
        let local = super::paths::local_path(path(name.as_bytes())?, meter)?;
        let p = local.path()?;
        match std::fs::metadata(p) {
            Ok(meta) => {
                super::within_input_bytes((out.len() as u64).saturating_add(meta.len()), limits)?;
                let confined;
                let p = if let Some(root) = root {
                    confined = super::paths::confined_path(p, root)?;
                    confined.as_path()
                } else {
                    p
                };
                let mut file = std::fs::File::open(p)?;
                let mut buf = [0; 8192];
                loop {
                    match file.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => out.extend_from_slice(buf.get(..n).unwrap_or_default())?,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(e) => return Err(e.into()),
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if i != 0 {
                    break;
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out.into_bytes()?)
}

pub(crate) fn read_source_path(
    p: &Path,
    root: Option<&Path>,
    limits: &Limits,
    meter: &Arc<Meter>,
) -> Result<(Bytes, crate::stream::ReaderKind), Error> {
    let name = p.as_os_str().as_encoded_bytes();
    let first = part(name, 0, meter)?;
    if let Some(base) = base(name) {
        Ok((
            read(base, root, limits, meter)?,
            crate::stream::ReaderKind::Memory,
        ))
    } else if !p.exists() && path(first.as_bytes())?.exists() {
        // These are literal os.path.exists checks, not fs.exists
        // (environment.py:119-125).
        Ok((
            read(name, root, limits, meter)?,
            crate::stream::ReaderKind::Memory,
        ))
    } else {
        // Once the literal gate succeeds, str paths use fsspec's fs.open
        // (environment.py:136-137), which normalises the physical path.
        let local = super::paths::local_path(p, meter)?;
        let physical = local.path()?;
        let confined;
        let physical = if let Some(root) = root {
            confined = super::paths::confined_path(physical, root)?;
            confined.as_path()
        } else {
            physical
        };
        Ok((
            super::read_path(physical, limits, meter)?,
            crate::stream::ReaderKind::Stream,
        ))
    }
}
