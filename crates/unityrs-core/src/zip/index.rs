// Ported from CPython 3.11 (PSF-2.0): Lib/zipfile.py. See THIRD_PARTY_LICENSES.
//! EOCD/ZIP64 discovery and central extras. Sorting stops on work refusal.
use super::bad;
use crate::{Error, limits::Meter};
use std::cmp::Ordering;

pub(super) fn item<T>(data: &[T], at: usize) -> Result<&T, Error> {
    data.get(at)
        .ok_or_else(|| bad("index outside archive table"))
}

pub(super) fn span(data: &[u8], at: usize, n: usize) -> Result<&[u8], Error> {
    data.get(at..at.checked_add(n).ok_or_else(|| bad("offset overflow"))?)
        .ok_or_else(|| bad("record outside archive"))
}
pub(super) fn u16le(data: &[u8], at: usize) -> Result<u16, Error> {
    Ok(u16::from_le_bytes(
        span(data, at, 2)?
            .try_into()
            .map_err(|_| bad("short u16"))?,
    ))
}
pub(super) fn u32le(data: &[u8], at: usize) -> Result<u32, Error> {
    Ok(u32::from_le_bytes(
        span(data, at, 4)?
            .try_into()
            .map_err(|_| bad("short u32"))?,
    ))
}
fn u64le(data: &[u8], at: usize) -> Result<u64, Error> {
    Ok(u64::from_le_bytes(
        span(data, at, 8)?
            .try_into()
            .map_err(|_| bad("short u64"))?,
    ))
}

pub(super) fn extra(mut data: &[u8], values: &mut [u64; 3]) -> Result<(), Error> {
    while data.len() >= 4 {
        let tag = u16le(data, 0)?;
        let len = usize::from(u16le(data, 2)?);
        let mut field = span(data, 4, len)?;
        if tag == 1 {
            for (i, value) in values.iter_mut().enumerate() {
                if *value == u64::from(u32::MAX) || i == 0 && *value == u64::MAX {
                    *value = u64le(field, 0)?;
                    field = field.get(8..).ok_or_else(|| bad("short ZIP64 extra"))?;
                }
            }
        }
        data = data.get(4 + len..).ok_or_else(|| bad("short extra"))?;
    }
    Ok(())
}

pub(super) fn directory(data: &[u8], meter: &Meter) -> Result<(usize, usize, i128), Error> {
    let tail = data.len().saturating_sub(22);
    let end = if data.get(tail..tail + 4) == Some(b"PK\x05\x06")
        && data.get(data.len().saturating_sub(2)..) == Some(&[0, 0])
    {
        tail
    } else {
        let mut found = None;
        for at in (data.len().saturating_sub(65_558)..data.len().saturating_sub(3)).rev() {
            meter.count_decode(1)?;
            if data.get(at..at + 4) == Some(b"PK\x05\x06") {
                found = Some(at);
                break;
            }
        }
        found.ok_or_else(|| bad("missing end of central directory"))?
    };
    span(data, end, 22)?;
    meter.count_decode(22)?;
    let mut size = u64::from(u32le(data, end + 12)?);
    let mut offset = u64::from(u32le(data, end + 16)?);
    let mut stop = end;
    if let Some(locator) = end.checked_sub(20)
        && data.get(locator..locator + 4) == Some(b"PK\x06\x07")
    {
        meter.count_decode(76)?;
        if u32le(data, locator + 4)? != 0 || u32le(data, locator + 16)? > 1 {
            return Err(bad("ZIP64 multi-disk unsupported"));
        }
        // CPython 3.11.18 first tries the locator's absolute reloff. If
        // that has no signature, try the fixed record before the locator
        // (prepended archive data, with no ZIP64 extensible data).
        let fixed = locator
            .checked_sub(56)
            .ok_or_else(|| bad("Corrupt zip64 end of central directory locator"))?;
        let reloff = u64le(data, locator + 8)?;
        let mut record = usize::try_from(reloff)
            .ok()
            .filter(|&n| n <= fixed)
            .ok_or_else(|| bad("Corrupt zip64 end of central directory locator"))?;
        let mut extrasz = fixed - record;
        if span(data, record, 4)? != b"PK\x06\x06" && record != fixed {
            record = fixed;
            extrasz = 0;
        }
        let h = span(data, record, 56)?;
        if h.get(..4) != Some(b"PK\x06\x06") {
            return Err(bad("Zip64 end of central directory record not found"));
        }
        size = u64le(h, 40)?;
        offset = u64le(h, 48)?;
        if offset.checked_add(size) != Some(reloff)
            || u64le(h, 4)?.checked_add(12) != 56_u64.checked_add(extrasz as u64)
        {
            return Err(bad("Corrupt zip64 end of central directory record"));
        }
        stop = fixed
            .checked_sub(extrasz)
            .ok_or_else(|| bad("Corrupt zip64 end of central directory record"))?;
    }
    let start = stop
        .checked_sub(usize::try_from(size).map_err(|_| bad("directory overflow"))?)
        .ok_or_else(|| bad("bad directory offset"))?;
    Ok((start, stop, start as i128 - i128::from(offset)))
}

/// In-place heapsort with fallible comparisons: no allocations and no work
/// continues after a comparison refuses its allowance.
pub(super) fn sort<T>(
    data: &mut [T],
    mut cmp: impl FnMut(&T, &T) -> Result<Ordering, Error>,
) -> Result<(), Error> {
    fn sift<T>(
        data: &mut [T],
        mut root: usize,
        end: usize,
        cmp: &mut impl FnMut(&T, &T) -> Result<Ordering, Error>,
    ) -> Result<(), Error> {
        while root < end / 2 {
            let mut child = root * 2 + 1;
            if child + 1 < end && cmp(item(data, child)?, item(data, child + 1)?)? == Ordering::Less
            {
                child += 1;
            }
            if cmp(item(data, root)?, item(data, child)?)? != Ordering::Less {
                break;
            }
            data.swap(root, child);
            root = child;
        }
        Ok(())
    }
    for root in (0..data.len() / 2).rev() {
        sift(data, root, data.len(), &mut cmp)?;
    }
    for end in (1..data.len()).rev() {
        data.swap(0, end);
        sift(data, 0, end, &mut cmp)?;
    }
    Ok(())
}
