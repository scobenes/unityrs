//! Durable upload identities, without networking, cancellation or sink policy.
//!
//! The trusted run directory is prepared by spool startup. One shared `Journal`
//! serializes mutations; each operation opens the current path, so a rename
//! never leaves a later append writing an unlinked inode. Buffers are inline.
use super::{Error, path};
mod mutation;
use mutation::{FileIo, Host, JournalIo};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

const LINE_LIMIT: usize = 16 * 1024;

/// An exact upload identity. Endpoints are compared as supplied, including
/// the distinction between `None` and an empty string.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Entry<'a> {
    pub endpoint: Option<&'a str>,
    pub bucket: &'a str,
    pub key: &'a str,
    pub upload_id: &'a str,
}
impl std::fmt::Debug for Entry<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("endpoint_bytes", &self.endpoint.map(str::len))
            .field("bucket_bytes", &self.bucket.len())
            .field("key_bytes", &self.key.len())
            .field("upload_id_bytes", &self.upload_id.len())
            .finish()
    }
}

/// A journal owned by a trusted, already prepared run directory.
pub struct Journal {
    directory: PathBuf,
    mutation: Mutex<bool>,
}

impl Journal {
    /// Retain the directory fallibly; no file is created until an append.
    pub fn new(directory: &Path) -> Result<Self, Error> {
        let mut owned = PathBuf::new();
        owned
            .try_reserve_exact(directory.as_os_str().len())
            .map_err(|_| Error::HostMemory)?;
        owned.push(directory);
        let mutation = Mutex::new(false);
        // Prime any platform's lazy mutex storage before running is written.
        drop(mutation.lock().unwrap_or_else(PoisonError::into_inner));
        Ok(Self {
            directory: owned,
            mutation,
        })
    }

    /// Append and sync before returning. A missing final newline is repaired
    /// before the new identity, preserving a torn line as a separate line.
    pub fn append(&self, entry: Entry<'_>) -> Result<(), Error> {
        self.append_using(entry, &Host)
    }

    fn append_using(&self, entry: Entry<'_>, io: &impl JournalIo) -> Result<(), Error> {
        let mut encoded = [0; LINE_LIMIT];
        let len = encode(entry, &mut encoded)?;
        let guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        if *guard {
            return Err(Error::Stopped);
        }
        let journal = path::join(&self.directory, Path::new("journal"))?;
        let mut file = match io.open(&journal, false) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => io.open(&journal, true)?,
            Err(e) => return Err(e.into()),
        };
        if file.length()? != 0 {
            file.seek(SeekFrom::End(-1))?;
            let mut last = [0];
            file.read_exact(&mut last)?;
            if last != *b"\n" {
                file.write_all(b"\n")?;
            }
        }
        file.write_all(encoded.get(..len).ok_or(Error::Internal)?)?;
        file.write_all(b"\n")?;
        file.sync_data()?;
        // An earlier create may have failed before syncing its directory.
        // Reopening an existing name proves no durability, so every successful
        // append covers that name after its data sync, under the same mutex.
        io.sync_directory(&self.directory)?;
        Ok(())
    }

    /// Remove every exact match after the caller has completed or aborted it.
    /// All other bytes survive, including malformed and oversized lines.
    pub fn remove(&self, entry: Entry<'_>) -> Result<(), Error> {
        self.remove_using(entry, &Host)
    }

    fn remove_using(&self, entry: Entry<'_>, io: &impl JournalIo) -> Result<(), Error> {
        let guard = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        if *guard {
            return Err(Error::Stopped);
        }
        self.rewrite(entry, io)
    }
    /// Nonblocking close under the same mutation lock. Once closed, a late
    /// controller completion cannot remove an identity the finisher reads.
    pub fn try_close(&self) -> bool {
        match self.mutation.try_lock() {
            Ok(mut closed) => {
                *closed = true;
                true
            }
            Err(std::sync::TryLockError::Poisoned(e)) => {
                *e.into_inner() = true;
                true
            }
            Err(std::sync::TryLockError::WouldBlock) => false,
        }
    }
    /// The finisher's acknowledged abort, after close excludes all controller
    /// mutations. No mutex is held across a read or network operation.
    pub fn remove_closed(&self, entry: Entry<'_>) -> Result<(), Error> {
        let closed = self.mutation.lock().unwrap_or_else(PoisonError::into_inner);
        if !*closed {
            return Err(Error::Internal);
        }
        self.rewrite(entry, &Host)
    }
    fn rewrite(&self, entry: Entry<'_>, io: &impl JournalIo) -> Result<(), Error> {
        let journal = path::join(&self.directory, Path::new("journal"))?;
        let Some(input) = open_read(&journal)? else {
            return Ok(());
        };
        let temporary = path::join(&self.directory, Path::new("journal.tmp"))?;
        let mut output = match io.open(&temporary, false) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => io.open(&temporary, true)?,
            Err(e) => return Err(e.into()),
        };
        output.truncate()?;
        let mut input = Lines::new(Some(input));
        let mut line = [0; LINE_LIMIT];
        while let Some(part) = input.line(&mut line, Some(&mut output))? {
            if part.overlong {
                continue;
            }
            let bytes = line.get(..part.len).ok_or(Error::Internal)?;
            if decode(bytes).is_some_and(|record| record.entry() == entry) {
                continue;
            }
            output.write_all(bytes)?;
            if part.newline {
                output.write_all(b"\n")?;
            }
        }
        output.sync_data()?;
        drop(output);
        io.rename(&temporary, &journal)?;
        io.sync_directory(&self.directory)?;
        Ok(())
    }

    /// Open one bounded snapshot by path. No lock is held while reading it;
    /// the caller controls when recovery and mutations may overlap.
    pub fn reader(&self) -> Result<Reader, Error> {
        let journal = path::join(&self.directory, Path::new("journal"))?;
        Ok(Reader {
            lines: Lines::new(open_read(&journal)?),
        })
    }
}

fn open_read(path: &Path) -> Result<Option<File>, Error> {
    match crate::sys::spool_file(path, false) {
        Ok(file) if file.metadata()?.is_file() => Ok(Some(file)),
        Ok(_) => Err(io::Error::from(io::ErrorKind::InvalidData).into()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// A bounded recovery reader; malformed or oversized lines are skipped.
pub struct Reader {
    lines: Lines,
}
impl Reader {
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<Record>, Error> {
        let mut line = [0; LINE_LIMIT];
        while let Some(part) = self.lines.line(&mut line, None)? {
            if !part.overlong
                && let Some(record) = decode(line.get(..part.len).ok_or(Error::Internal)?)
            {
                return Ok(Some(record));
            }
        }
        Ok(None)
    }
}

struct Text<const N: usize> {
    bytes: [u8; N],
    len: usize,
}
impl<const N: usize> Text<N> {
    fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }
    fn push(&mut self, bytes: &[u8]) -> Option<()> {
        let end = self.len.checked_add(bytes.len())?;
        self.bytes.get_mut(self.len..end)?.copy_from_slice(bytes);
        self.len = end;
        Some(())
    }
    fn text(&self) -> &str {
        // Every field is UTF-8 checked before a Record escapes the decoder.
        std::str::from_utf8(self.bytes.get(..self.len).unwrap_or_default()).unwrap_or_default()
    }
}

/// A decoded identity whose storage is independent of the number of uploads.
pub struct Record {
    endpoint: Text<1024>,
    has_endpoint: bool,
    bucket: Text<255>,
    key: Text<1024>,
    upload_id: Text<1024>,
}
impl std::fmt::Debug for Record {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.entry().fmt(f)
    }
}
impl Record {
    #[must_use]
    pub fn entry(&self) -> Entry<'_> {
        Entry {
            endpoint: self.has_endpoint.then(|| self.endpoint.text()),
            bucket: self.bucket.text(),
            key: self.key.text(),
            upload_id: self.upload_id.text(),
        }
    }
}

fn encode(entry: Entry<'_>, output: &mut [u8]) -> Result<usize, Error> {
    if entry.endpoint.is_some_and(|s| s.len() > 1024)
        || entry.bucket.len() > 255
        || entry.key.len() > 1024
        || entry.upload_id.len() > 1024
    {
        return Err(Error::Internal);
    }
    let length = output.len();
    let mut output = output;
    let result = (|| -> io::Result<()> {
        output.write_all(b"[")?;
        if let Some(endpoint) = entry.endpoint {
            quote(&mut output, endpoint)?;
        } else {
            output.write_all(b"null")?;
        }
        for text in [entry.bucket, entry.key, entry.upload_id] {
            output.write_all(b",")?;
            quote(&mut output, text)?;
        }
        output.write_all(b"]")
    })();
    result.map_err(|_| Error::Internal)?;
    Ok(length - output.len())
}

fn quote(output: &mut impl Write, text: &str) -> io::Result<()> {
    fn hex(n: u8) -> u8 {
        if n < 10 { b'0' + n } else { b'a' + n - 10 }
    }
    output.write_all(b"\"")?;
    for byte in text.bytes() {
        match byte {
            b'"' => output.write_all(b"\\\"")?,
            b'\\' => output.write_all(b"\\\\")?,
            0..=31 => {
                output.write_all(&[b'\\', b'u', b'0', b'0', hex(byte >> 4), hex(byte & 15)])?;
            }
            _ => output.write_all(&[byte])?,
        }
    }
    output.write_all(b"\"")
}

struct Json<'a> {
    rest: &'a [u8],
}
impl Json<'_> {
    fn byte(&mut self) -> Option<u8> {
        let (&byte, rest) = self.rest.split_first()?;
        self.rest = rest;
        Some(byte)
    }
    fn whitespace(&mut self) {
        while self
            .rest
            .first()
            .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
        {
            self.rest = self.rest.get(1..).unwrap_or_default();
        }
    }
    fn token(&mut self, byte: u8) -> Option<()> {
        self.whitespace();
        (self.byte()? == byte).then_some(())
    }
    fn hex(&mut self) -> Option<u32> {
        let mut value = 0;
        for _ in 0..4 {
            value = value * 16 + char::from(self.byte()?).to_digit(16)?;
        }
        Some(value)
    }
    fn string<const N: usize>(&mut self, out: &mut Text<N>) -> Option<()> {
        self.token(b'"')?;
        loop {
            match self.byte()? {
                b'"' => {
                    std::str::from_utf8(out.bytes.get(..out.len)?).ok()?;
                    return Some(());
                }
                0..=31 => return None,
                b'\\' => match self.byte()? {
                    b'"' => out.push(b"\"")?,
                    b'\\' => out.push(b"\\")?,
                    b'/' => out.push(b"/")?,
                    b'b' => out.push(&[8])?,
                    b'f' => out.push(&[12])?,
                    b'n' => out.push(b"\n")?,
                    b'r' => out.push(b"\r")?,
                    b't' => out.push(b"\t")?,
                    b'u' => {
                        let mut value = self.hex()?;
                        if (0xd800..=0xdbff).contains(&value) {
                            if self.byte()? != b'\\' || self.byte()? != b'u' {
                                return None;
                            }
                            let low = self.hex()?;
                            if !(0xdc00..=0xdfff).contains(&low) {
                                return None;
                            }
                            value = 0x10000 + (value - 0xd800) * 1024 + low - 0xdc00;
                        }
                        let mut bytes = [0; 4];
                        out.push(char::from_u32(value)?.encode_utf8(&mut bytes).as_bytes())?;
                    }
                    _ => return None,
                },
                byte => out.push(&[byte])?,
            }
        }
    }
}

fn decode(line: &[u8]) -> Option<Record> {
    let mut json = Json { rest: line };
    let mut record = Record {
        endpoint: Text::new(),
        has_endpoint: false,
        bucket: Text::new(),
        key: Text::new(),
        upload_id: Text::new(),
    };
    json.token(b'[')?;
    json.whitespace();
    if let Some(rest) = json.rest.strip_prefix(b"null") {
        json.rest = rest;
    } else {
        json.string(&mut record.endpoint)?;
        record.has_endpoint = true;
    }
    json.token(b',')?;
    json.string(&mut record.bucket)?;
    json.token(b',')?;
    json.string(&mut record.key)?;
    json.token(b',')?;
    json.string(&mut record.upload_id)?;
    json.token(b']')?;
    json.whitespace();
    json.rest.is_empty().then_some(record)
}

struct Part {
    len: usize,
    newline: bool,
    overlong: bool,
}
struct Lines {
    file: Option<File>,
    buffer: [u8; 4096],
    start: usize,
    end: usize,
}

impl Lines {
    fn new(file: Option<File>) -> Self {
        Self {
            file,
            buffer: [0; 4096],
            start: 0,
            end: 0,
        }
    }

    // Oversized raw lines optionally flow directly to the rewrite once they
    // cannot match an identity. No allocation or total-line counter grows.
    fn line(
        &mut self,
        line: &mut [u8; LINE_LIMIT],
        mut overflow: Option<&mut dyn Write>,
    ) -> Result<Option<Part>, Error> {
        let mut len = 0;
        let mut overlong = false;
        loop {
            if self.start == self.end {
                let Some(file) = &mut self.file else {
                    return Ok(None);
                };
                self.end = loop {
                    match file.read(&mut self.buffer) {
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                        result => break result?,
                    }
                };
                self.start = 0;
                if self.end == 0 {
                    return Ok((len != 0 || overlong).then_some(Part {
                        len,
                        newline: false,
                        overlong,
                    }));
                }
            }
            let bytes = self
                .buffer
                .get(self.start..self.end)
                .ok_or(Error::Internal)?;
            let newline = bytes.iter().position(|&byte| byte == b'\n');
            let count = newline.unwrap_or(bytes.len());
            let bytes = bytes.get(..count).ok_or(Error::Internal)?;
            if !overlong && bytes.len() <= LINE_LIMIT - len {
                line.get_mut(len..len + bytes.len())
                    .ok_or(Error::Internal)?
                    .copy_from_slice(bytes);
                len += bytes.len();
            } else {
                if let Some(output) = &mut overflow {
                    if !overlong {
                        output.write_all(line.get(..len).ok_or(Error::Internal)?)?;
                    }
                    output.write_all(bytes)?;
                }
                overlong = true;
            }
            self.start += count + usize::from(newline.is_some());
            if newline.is_some() {
                if overlong && let Some(output) = &mut overflow {
                    output.write_all(b"\n")?;
                }
                return Ok(Some(Part {
                    len,
                    newline: true,
                    overlong,
                }));
            }
        }
    }
}

#[cfg(test)]
mod fault_tests;
#[cfg(test)]
mod tests {
    use super::{Entry, Journal};
    #[test]
    fn mutations_recover_a_poisoned_mutex() {
        let scratch = unityrs_testkit::scratch::Scratch::new("journal-poison");
        let journal = Journal::new(&scratch).unwrap();
        let first = Entry {
            endpoint: None,
            bucket: "bucket",
            key: "first",
            upload_id: "id",
        };
        journal.append(first).unwrap();
        let panic = std::panic::catch_unwind(|| {
            let _guard = journal.mutation.lock().unwrap();
            panic!("synthetic interrupted mutation");
        });
        assert!(panic.is_err());
        let second = Entry {
            key: "second",
            ..first
        };
        journal.append(second).unwrap();
        journal.remove(first).unwrap();
        let mut reader = journal.reader().unwrap();
        assert_eq!(reader.next().unwrap().unwrap().entry(), second);
        assert!(reader.next().unwrap().is_none());
    }
}
