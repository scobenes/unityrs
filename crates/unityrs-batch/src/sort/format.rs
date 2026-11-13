//! Private v1 format: 16-byte header, then u32 inclusive lengths and payloads.
//! Fixed-width numbers are little-endian; payload schemas belong to projections.
use super::{Control, Order, Point};
use crate::{
    phase::{Buffer, Error, check},
    spool::workspace::{File, Sealed, Workspace},
};
use std::{
    io::{self, Read},
    sync::Arc,
};
use unityrs_core::limits::Meter;

pub const MAX_FRAME: usize = 64 * 1024;
pub const MAX_PAYLOAD: usize = MAX_FRAME - 4;
pub(crate) const HEADER: u64 = 16;
const MAGIC: &[u8; 8] = b"URSSORT\0";
const VERSION: u32 = 1;
const OUTPUT: usize = 64 * 1024;

pub struct Frame {
    bytes: Buffer<u8>,
}
impl Frame {
    pub fn new(meter: &Arc<Meter>) -> Result<Self, Error> {
        Ok(Self {
            bytes: Buffer::new(meter, MAX_PAYLOAD)?,
        })
    }
    pub fn set(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.bytes.clear();
        self.bytes.extend(bytes)
    }
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

pub(crate) fn header(tag: u32) -> [u8; 16] {
    let mut header = [0; 16];
    for (slot, bytes) in header.as_chunks_mut::<4>().0.iter_mut().zip([
        *b"URSS",
        *b"ORT\0",
        VERSION.to_le_bytes(),
        tag.to_le_bytes(),
    ]) {
        slot.copy_from_slice(&bytes);
    }
    header
}
pub(crate) fn read_exact(
    read: &mut dyn Read,
    bytes: &mut [u8],
    stopped: &dyn Fn() -> bool,
) -> Result<(), Error> {
    let mut offset = 0;
    while offset < bytes.len() {
        check(stopped)?;
        match read.read(bytes.get_mut(offset..).ok_or_else(Error::internal)?) {
            Ok(0) => return Err(Error::internal()),
            Ok(n) => offset += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
pub(crate) fn check_header(
    read: &mut dyn Read,
    tag: u32,
    stopped: &dyn Fn() -> bool,
) -> Result<(), Error> {
    let mut bytes = [0; 16];
    read_exact(read, &mut bytes, stopped)?;
    if bytes != header(tag) || bytes.get(..8) != Some(MAGIC.as_slice()) {
        return Err(Error::internal());
    }
    Ok(())
}

/// One decoded head; well below the 1 MiB per-input allocation envelope.
/// Descriptor length distinguishes clean EOF from a truncated frame/header.
pub struct Reader<'a> {
    read: Box<dyn Read + Send>,
    remaining: u64,
    head: Buffer<u8>,
    previous: Buffer<u8>,
    populated: bool,
    order: &'a dyn Order,
    length: usize,
}
impl<'a> Reader<'a> {
    pub fn open(
        workspace: &Workspace,
        file: &Sealed,
        order: &'a dyn Order,
        meter: &Arc<Meter>,
        stopped: &dyn Fn() -> bool,
    ) -> Result<Self, Error> {
        let remaining = file
            .length()
            .checked_sub(HEADER)
            .ok_or_else(Error::internal)?;
        let mut read = workspace.open(file, stopped)?;
        check_header(&mut read, order.tag(), stopped)?;
        let mut head = Buffer::new(meter, MAX_PAYLOAD)?;
        head.fill(0);
        Ok(Self {
            read,
            remaining,
            head,
            previous: Buffer::new(meter, MAX_PAYLOAD)?,
            populated: false,
            order,
            length: 0,
        })
    }
    pub fn next(&mut self, stopped: &dyn Fn() -> bool) -> Result<Option<&[u8]>, Error> {
        self.read(stopped, true)
    }
    /// Unsorted pass output, validated by its following external sort.
    pub(crate) fn next_unordered(
        &mut self,
        stopped: &dyn Fn() -> bool,
    ) -> Result<Option<&[u8]>, Error> {
        self.read(stopped, false)
    }
    fn read(&mut self, stopped: &dyn Fn() -> bool, ordered: bool) -> Result<Option<&[u8]>, Error> {
        check(stopped)?;
        if self.remaining == 0 {
            return Ok(None);
        }
        if self.remaining < 4 {
            return Err(Error::internal());
        }
        self.previous.clear();
        self.previous
            .extend(self.head.get(..self.length).ok_or_else(Error::internal)?)?;
        let mut length = [0; 4];
        read_exact(&mut self.read, &mut length, stopped)?;
        let length = u32::from_le_bytes(length);
        let n = usize::try_from(length).map_err(|_| Error::internal())?;
        if !(4..=MAX_FRAME).contains(&n) || u64::from(length) > self.remaining {
            return Err(Error::internal());
        }
        let head = self.head.get_mut(..n - 4).ok_or_else(Error::internal)?;
        read_exact(&mut self.read, head, stopped)?;
        self.order.validate(head)?;
        if ordered && self.populated && super::compare(self.order, &self.previous, head).is_gt() {
            return Err(Error::internal());
        }
        self.populated = true;
        self.remaining -= u64::from(length);
        self.length = n - 4;
        Ok(Some(head))
    }
    pub(crate) fn head(&self) -> &[u8] {
        self.head.get(..self.length).unwrap_or_default()
    }
}

pub(crate) struct Writer {
    file: File,
    bytes: Buffer<u8>,
}
impl Writer {
    pub fn new(
        workspace: &mut Workspace,
        tag: u32,
        meter: &Arc<Meter>,
        control: &Control<'_>,
    ) -> Result<Self, Error> {
        let file = workspace.create_file(control.stopped)?;
        let mut bytes = Buffer::new(meter, OUTPUT)?;
        bytes.extend(&header(tag))?;
        Ok(Self { file, bytes })
    }
    pub fn frame(&mut self, bytes: &[u8], control: &Control<'_>) -> Result<(), Error> {
        let length = bytes
            .len()
            .checked_add(4)
            .filter(|&n| n <= MAX_FRAME)
            .ok_or_else(Error::internal)?;
        if length > OUTPUT - self.bytes.len() {
            self.flush(control)?;
        }
        self.bytes.extend(
            &u32::try_from(length)
                .map_err(|_| Error::internal())?
                .to_le_bytes(),
        )?;
        self.bytes.extend(bytes)
    }
    fn flush(&mut self, control: &Control<'_>) -> Result<(), Error> {
        if !self.bytes.is_empty() {
            control.at(Point::BeforeWrite)?;
            self.file.append(&self.bytes, control.stopped)?;
            self.bytes.clear();
        }
        Ok(())
    }
    pub fn finish(mut self, control: &Control<'_>) -> Result<Sealed, Error> {
        self.flush(control)?;
        Ok(self.file.seal(control.stopped)?)
    }
}
