//! Explicit stored ZIP encoding. Layout owns all extra-field and end decisions.
use crate::{
    phase::{Buffer, Error},
    winners::layout::{End, Entry, ZIP32},
};

fn word(out: &mut Buffer<u8>, n: u16) -> Result<(), Error> {
    out.extend(&n.to_le_bytes())
}
fn long(out: &mut Buffer<u8>, n: u32) -> Result<(), Error> {
    out.extend(&n.to_le_bytes())
}
fn wide(out: &mut Buffer<u8>, n: u64) -> Result<(), Error> {
    out.extend(&n.to_le_bytes())
}
fn classic(n: u64) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}
fn short(n: u64) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}
fn version(extra: u64) -> u16 {
    if extra == 0 { 20 } else { 45 }
}
fn name_length(name: &str) -> Result<u16, Error> {
    u16::try_from(name.len()).map_err(|_| Error::internal())
}
fn common(out: &mut Buffer<u8>, crc: u32, size: u64) -> Result<(), Error> {
    for n in [0x0800, 0, 0, 0x0021] {
        word(out, n)?;
    }
    long(out, crc)?;
    long(out, classic(size))?;
    long(out, classic(size))
}
fn extra(out: &mut Buffer<u8>, size: u64, e: Entry, central: bool) -> Result<(), Error> {
    let length = if central {
        e.central_extra
    } else {
        e.local_extra
    };
    if length > 0 {
        word(out, 1)?;
        word(
            out,
            u16::try_from(length - 4).map_err(|_| Error::internal())?,
        )?;
        if e.local_extra > 0 {
            wide(out, size)?;
            wide(out, size)?;
        }
        if central && e.offset >= ZIP32 {
            wide(out, e.offset)?;
        }
    }
    Ok(())
}
pub(super) fn local(
    out: &mut Buffer<u8>,
    name: &str,
    size: u64,
    crc: u32,
    e: Entry,
) -> Result<(), Error> {
    out.clear();
    long(out, 0x0403_4b50)?;
    word(out, version(e.local_extra))?;
    common(out, crc, size)?;
    word(out, name_length(name)?)?;
    word(out, short(e.local_extra))?;
    out.extend(name.as_bytes())?;
    extra(out, size, e, false)
}
pub(super) fn central(
    out: &mut Buffer<u8>,
    name: &str,
    size: u64,
    crc: u32,
    e: Entry,
) -> Result<(), Error> {
    out.clear();
    long(out, 0x0201_4b50)?;
    word(out, 20)?;
    word(out, version(e.central_extra))?;
    common(out, crc, size)?;
    word(out, name_length(name)?)?;
    word(out, short(e.central_extra))?;
    for _ in 0..3 {
        word(out, 0)?;
    } // comment length, disk, internal attributes
    long(out, 0)?; // external attributes
    long(out, classic(e.offset))?;
    out.extend(name.as_bytes())?;
    extra(out, size, e, true)
}
pub(super) fn end(out: &mut Buffer<u8>, end: End) -> Result<(), Error> {
    out.clear();
    if end.zip64 {
        long(out, 0x0606_4b50)?;
        wide(out, 44)?;
        word(out, 45)?;
        word(out, 45)?;
        long(out, 0)?;
        long(out, 0)?;
        for n in [end.count, end.count, end.central_size, end.central_offset] {
            wide(out, n)?;
        }
        long(out, 0x0706_4b50)?;
        long(out, 0)?;
        wide(
            out,
            end.central_offset
                .checked_add(end.central_size)
                .ok_or_else(Error::internal)?,
        )?;
        long(out, 1)?;
    }
    long(out, 0x0605_4b50)?;
    word(out, 0)?;
    word(out, 0)?;
    word(out, short(end.count))?;
    word(out, short(end.count))?;
    long(out, classic(end.central_size))?;
    long(out, classic(end.central_offset))?;
    word(out, 0)
}

#[cfg(test)]
#[path = "header_tests.rs"]
mod tests;
