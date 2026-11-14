//! Bounded diagnostic construction. The host prepares immutable borrowed
//! secrets before running; a set-once credential slot can extend that list.
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering::Relaxed},
};
pub const LINE_BYTES: usize = libc::PIPE_BUF;
pub struct Secrets<'a> {
    pub values: &'a [&'a str],
    pub credentials: &'a OnceLock<[&'a str; 3]>,
}
impl Secrets<'_> {
    /// Mask before cutting, including overlapping secrets and those crossing
    /// the output boundary. Empty values are skipped; short values still mask.
    pub fn mask(&self, input: &str, output: &mut [u8]) -> usize {
        let mut at = 0;
        let mut used = 0;
        while at < input.len() {
            let mut end = at;
            for secret in self
                .values
                .iter()
                .copied()
                .chain(self.credentials.get().into_iter().flatten().copied())
            {
                if !secret.is_empty() && input.get(at..).is_some_and(|s| s.starts_with(secret)) {
                    end = end.max(at + secret.len());
                }
            }
            if end > at {
                // Include any secret overlapping the matched span.
                let mut cursor = at;
                while cursor < end {
                    for secret in self
                        .values
                        .iter()
                        .copied()
                        .chain(self.credentials.get().into_iter().flatten().copied())
                    {
                        if !secret.is_empty()
                            && input.get(cursor..).is_some_and(|s| s.starts_with(secret))
                        {
                            end = end.max(cursor + secret.len());
                        }
                    }
                    cursor += input
                        .get(cursor..)
                        .and_then(|s| s.chars().next())
                        .map_or(1, char::len_utf8);
                }
                let Some(dst) = output.get_mut(used..used + 3) else {
                    break;
                };
                dst.copy_from_slice(b"***");
                used += 3;
                at = end;
            } else {
                let Some(c) = input.get(at..).and_then(|s| s.chars().next()) else {
                    break;
                };
                let n = c.len_utf8();
                let Some(dst) = output.get_mut(used..used + n) else {
                    break;
                };
                c.encode_utf8(dst);
                used += n;
                at += n;
            }
        }
        used
    }
}
pub struct Writer {
    route: crate::sys::stderr::Route,
    dropped: AtomicU64,
}
impl Writer {
    /// Early, single-threaded setup may replace fd 2 but never changes flags
    /// of the caller's shared description.
    pub fn prepare() -> std::io::Result<Self> {
        Ok(Self {
            route: crate::sys::stderr::prepare()?,
            dropped: AtomicU64::new(0),
        })
    }
    pub fn line(
        &self,
        text: &str,
        secrets: &Secrets<'_>,
        buffer: &mut [u8; LINE_BYTES],
        lifecycle: bool,
    ) {
        let n = secrets.mask(text, buffer);
        if !crate::sys::stderr::write(self.route, buffer.get(..n).unwrap_or_default(), lifecycle) {
            self.dropped.fetch_add(1, Relaxed);
        }
    }
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Relaxed)
    }
}

/// No panic hook writes stderr. Each service thread has a fixed const TLS
/// slot; the body's `catch_unwind` reads it after hand-over. Pool `JoinError`s
/// carry their own payload to the controller instead.
pub mod panic {
    use std::{cell::RefCell, fmt::Write};
    #[derive(Clone, Copy)]
    pub struct Slot {
        pub bytes: [u8; 1024],
        pub len: usize,
        pub cut: bool,
    }
    impl Slot {
        const fn new() -> Self {
            Self {
                bytes: [0; 1024],
                len: 0,
                cut: false,
            }
        }
    }
    impl Write for Slot {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            if self.cut {
                return Ok(());
            }
            let mut n = s.len().min(self.bytes.len() - self.len);
            while !s.is_char_boundary(n) {
                n -= 1;
            }
            if let Some(dst) = self.bytes.get_mut(self.len..self.len + n) {
                dst.copy_from_slice(s.get(..n).unwrap_or_default().as_bytes());
            }
            self.len += n;
            self.cut |= n < s.len();
            Ok(())
        }
    }
    thread_local! { static SLOT: RefCell<Slot> = const { RefCell::new(Slot::new()) }; }
    pub fn install() {
        std::panic::set_hook(Box::new(|info| {
            SLOT.with(|slot| {
                let Ok(mut slot) = slot.try_borrow_mut() else {
                    return;
                };
                *slot = Slot::new();
                if let Some(location) = info.location() {
                    let _ = write!(&mut *slot, "{}:{}: ", location.file(), location.line());
                }
                let payload = info
                    .payload()
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("non-string panic payload");
                let _ = slot.write_str(payload);
            });
        }));
    }
    #[must_use]
    pub fn read() -> Slot {
        SLOT.with(|s| *s.borrow())
    }
}

#[cfg(test)]
mod tests;
