//! A counting pass and a bounded writing pass; neither can grow a buffer.
use super::{Exception, Refusal};
pub(crate) struct Sink<'a> {
    bytes: Option<&'a mut [u8]>,
    len: usize,
    failed: bool,
}
impl<'a> Sink<'a> {
    pub fn count() -> Self {
        Self {
            bytes: None,
            len: 0,
            failed: false,
        }
    }
    pub fn write(bytes: &'a mut [u8]) -> Self {
        Self {
            bytes: Some(bytes),
            len: 0,
            failed: false,
        }
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn put(&mut self, bytes: &[u8]) {
        let Some(end) = self.len.checked_add(bytes.len()) else {
            self.failed = true;
            return;
        };
        if let Some(out) = &mut self.bytes {
            if let Some(slot) = out.get_mut(self.len..end) {
                slot.copy_from_slice(bytes);
            } else {
                self.failed = true;
            }
        }
        self.len = end;
    }
    pub fn zeros(&mut self, n: usize) {
        let Some(end) = self.len.checked_add(n) else {
            self.failed = true;
            return;
        };
        if let Some(out) = &mut self.bytes {
            if let Some(slot) = out.get_mut(self.len..end) {
                slot.fill(0);
            } else {
                self.failed = true;
            }
        }
        self.len = end;
    }
    pub fn byte(&mut self, n: u8) {
        self.put(&[n]);
    }
    pub fn le16(&mut self, n: u16) {
        self.put(&n.to_le_bytes());
    }
    pub fn le32(&mut self, n: u32) {
        self.put(&n.to_le_bytes());
    }
    pub fn be16(&mut self, n: u16) {
        self.put(&n.to_be_bytes());
    }
    pub fn be32(&mut self, n: u32) {
        self.put(&n.to_be_bytes());
    }
    pub fn text(&mut self, args: std::fmt::Arguments<'_>) {
        use std::fmt::Write;
        if self.write_fmt(args).is_err() {
            self.failed = true;
        }
    }
    pub fn finish(self) -> Result<usize, Refusal> {
        if self.failed {
            Err(Refusal::new(
                Exception::MemoryError,
                "output exceeds address space",
            ))
        } else {
            Ok(self.len)
        }
    }
}
impl std::fmt::Write for Sink<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.put(s.as_bytes());
        Ok(())
    }
}
