//! Errors of the borrowed object-mode view.
use crate::limits::ChargeError;

/// A failure reading an object-mode attribute.
#[derive(Debug, thiserror::Error)]
pub enum ViewError {
    /// A required attribute missing on an `UnknownObject`.
    #[error("'UnknownObject' object has no attribute '{0}'")]
    NoAttribute(&'static str),
    /// A type or range no characterised typetree writes.
    #[error("malformed object: {0}")]
    Malformed(String),
    /// An integer that Python's bytes constructor refuses.
    #[error("bytes must be in range(0, 256)")]
    ByteRange,
    /// Work refused by the caller's meter.
    #[error(transparent)]
    Charge(#[from] ChargeError),
}
const _: () = crate::send_sync::<ViewError>();
impl ViewError {
    /// Formats a malformed-input diagnostic into a fixed 512-byte buffer.
    /// An allocator refusal returns `Charge` instead, tagged with `meter`'s
    /// reservation. A longer diagnostic stops at the last complete write.
    pub fn malformed(detail: std::fmt::Arguments<'_>, meter: &crate::limits::Meter) -> Self {
        // Diagnostics are a fixed-size constant, as in meshdecode's existing
        // memory contract. Even this allocation can be refused by the host.
        let mut bytes = Vec::new();
        if let Err(error) =
            crate::limits::reserve_exact(&mut bytes, CAPACITY, CAPACITY as u64, meter)
        {
            return Self::Charge(ChargeError::Budget(error));
        }
        let mut text = String::from_utf8(bytes).unwrap_or_default();
        let _ = std::fmt::write(&mut Bounded(&mut text), detail);
        Self::Malformed(text)
    }
}

const CAPACITY: usize = 512;
struct Bounded<'a>(&'a mut String);
impl std::fmt::Write for Bounded<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        if self.0.len().saturating_add(s.len()) > CAPACITY {
            return Err(std::fmt::Error);
        }
        self.0.push_str(s);
        Ok(())
    }
}
