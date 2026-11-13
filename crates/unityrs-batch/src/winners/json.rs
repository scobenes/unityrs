//! Bounded public base-row encoding. Prepared text is escaped without recuts.
use crate::{
    phase::{Account, Buffer, Error},
    records::{LINE, Text, text::escaped},
    spool::workspace::{File, Sealed},
};
use std::fmt::{self, Write};

pub(super) struct Json {
    bytes: Buffer<u8>,
    failed: Option<Error>,
}
impl fmt::Write for Json {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.bytes.extend(s.as_bytes()).map_err(|error| {
            self.failed = Some(error);
            fmt::Error
        })
    }
}
impl Json {
    pub fn new(account: &Account) -> Result<Self, Error> {
        Ok(Self {
            bytes: Buffer::new(account.meter(), LINE)?,
            failed: None,
        })
    }
    pub fn start(&mut self, name: &str, value: &str) -> Result<(), Error> {
        self.bytes.clear();
        self.bytes.extend(b"{")?;
        self.text(name, Some(value))
    }
    fn field(&mut self, name: &str) -> Result<(), Error> {
        if self.bytes.len() > 1 {
            self.bytes.push(b',')?;
        }
        self.bytes.push(b'"')?;
        self.bytes.extend(name.as_bytes())?;
        self.bytes.extend(b"\":")
    }
    pub fn text(&mut self, name: &str, value: Option<&str>) -> Result<(), Error> {
        self.field(name)?;
        if let Some(value) = value {
            self.bytes.push(b'"')?;
            escaped(Text::Prepared(value, false), |b| self.bytes.extend(b))?;
            self.bytes.push(b'"')
        } else {
            self.bytes.extend(b"null")
        }
    }
    pub fn value(&mut self, name: &str, value: impl fmt::Display) -> Result<(), Error> {
        self.field(name)?;
        write!(self, "{value}").map_err(|_| self.failed.take().unwrap_or_else(Error::internal))
    }
    pub fn hex(&mut self, name: &str, value: Option<&[u8]>) -> Result<(), Error> {
        self.field(name)?;
        if let Some(value) = value {
            self.bytes.push(b'"')?;
            write!(self, "{}", crate::digest::Hex(value))
                .map_err(|_| self.failed.take().unwrap_or_else(Error::internal))?;
            self.bytes.push(b'"')
        } else {
            self.bytes.extend(b"null")
        }
    }
    pub fn end(&mut self) -> Result<&[u8], Error> {
        self.bytes.extend(b"}\n")?;
        Ok(&self.bytes)
    }
}
/// No file for an empty stream. Pending row buffers drop without flushing.
pub(super) struct Rows {
    file: Option<File>,
    bytes: Buffer<u8>,
}
impl Rows {
    pub fn new(account: &Account) -> Result<Self, Error> {
        Ok(Self {
            file: None,
            bytes: Buffer::new(account.meter(), 64 * 1024)?,
        })
    }
    pub fn put(&mut self, row: &[u8], cx: &mut super::Context<'_>) -> Result<(), Error> {
        if row.len() > LINE {
            return Err(Error::internal());
        }
        if self.file.is_none() {
            self.file = Some(cx.workspace.create_file(cx.stopped)?);
        }
        if self.bytes.len() + row.len() > 64 * 1024 {
            self.flush(cx)?;
        }
        self.bytes.extend(row)
    }
    fn flush(&mut self, cx: &super::Context<'_>) -> Result<(), Error> {
        if !self.bytes.is_empty() {
            crate::sort::Control {
                stopped: cx.stopped,
                hooks: cx.hooks,
            }
            .at(crate::sort::Point::BeforeWrite)?;
            self.file
                .as_mut()
                .ok_or_else(Error::internal)?
                .append(&self.bytes, cx.stopped)?;
            self.bytes.clear();
        }
        Ok(())
    }
    pub fn finish(mut self, cx: &super::Context<'_>) -> Result<Option<Sealed>, Error> {
        self.flush(cx)?;
        self.file
            .map(|f| f.seal(cx.stopped).map_err(Error::from))
            .transpose()
    }
}
