//! Inspect string previews and byte-bounded, transactional pages.
use super::{Document, Error, Result, cursor, document, name};
use std::{fmt::Write as _, sync::Arc};
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_dump::text::{Sink, Text};

/// Preview at most 128 source bytes, without splitting a UTF-8 code point.
/// Raw prefixes retain the usual hex representation. Lengths count bytes.
pub fn string(out: &mut impl Sink, bytes: &[u8], preview: bool) {
    if !preview || bytes.len() <= 128 {
        name(out, bytes);
        return;
    }
    if !out.count_work(128) {
        return;
    }
    let mut prefix = bytes.get(..128).unwrap_or_default();
    if let Err(e) = std::str::from_utf8(prefix)
        && e.error_len().is_none()
    {
        prefix = prefix.get(..e.valid_up_to()).unwrap_or_default();
    }
    let _ = write!(
        out,
        "{{\"$unityrs\":\"preview\",\"length\":{},\"prefix\":",
        bytes.len()
    );
    name(out, prefix);
    let _ = write!(out, ",\"omitted\":{}}}", bytes.len() - prefix.len());
}

struct Entry {
    header: Document,
    key: cursor::Key,
}
pub struct Page {
    entries: MeteredVec<Entry>,
    bytes: usize,
    cap: usize,
}
impl Page {
    pub fn new(meter: &Arc<Meter>, cap: usize) -> Self {
        Self {
            entries: MeteredVec::new(meter),
            bytes: 0,
            cap,
        }
    }
    pub fn checkpoint(&self) -> (usize, usize) {
        (self.entries.len(), self.bytes)
    }
    pub fn rollback(&mut self, (len, bytes): (usize, usize)) {
        self.entries.truncate(len);
        self.bytes = bytes;
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn push(&mut self, header: Document, key: cursor::Key) -> Result<bool> {
        let bytes = self
            .bytes
            .saturating_add(header.json.len())
            .saturating_add(usize::from(self.len() > 0));
        if bytes > self.cap {
            return Ok(false);
        }
        self.entries.push(Entry { header, key })?;
        self.bytes = bytes;
        Ok(true)
    }
    pub fn finish(
        mut self,
        inputs: &Document,
        total: usize,
        mut more: bool,
        fingerprint: &str,
        cursor_error: Option<Error>,
        meter: &Arc<Meter>,
    ) -> Result<Document> {
        let mut prefix = Text::on(meter);
        prefix.push_str("{\"format\":\"unityrs.inspect\",\"version\":1,\"inputs\":[");
        prefix.push_str(&inputs.json);
        prefix.push_str("],\"objects\":[");
        let prefix = document(prefix)?;
        loop {
            let mut tail = Text::on(meter);
            let _ = write!(tail, "],\"total_objects\":{total},\"next_cursor\":");
            if more {
                let Some(last) = self.entries.as_slice().last() else {
                    return Err(Error::response_cap(
                        "cannot hold input metadata, object addressing and pagination",
                        self.cap,
                    ));
                };
                let token = last.key.token(fingerprint, meter)?;
                name(&mut tail, token.json.as_bytes());
            } else {
                tail.push_str("null");
            }
            tail.push_str("}\n");
            let tail = document(tail)?;
            if prefix
                .json
                .len()
                .saturating_add(self.bytes)
                .saturating_add(tail.json.len())
                <= self.cap
            {
                if more && let Some(error) = cursor_error {
                    return Err(error);
                }
                let mut out = Text::on(meter);
                out.push_str(&prefix.json);
                for (index, entry) in self.entries.as_slice().iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    out.push_str(&entry.header.json);
                }
                out.push_str(&tail.json);
                return document(out);
            }
            let Some(removed) = self.entries.pop() else {
                return Err(Error::response_cap(
                    "cannot hold input metadata and object addressing",
                    self.cap,
                ));
            };
            self.bytes -= removed.header.json.len() + usize::from(self.len() > 0);
            more = true;
        }
    }
}
