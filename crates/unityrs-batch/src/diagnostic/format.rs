//! Allocation-free masking across formatter writes, before bounded capture.
use super::Secrets;
use std::{
    fmt::{self, Write},
    sync::OnceLock,
};

static SOURCE: OnceLock<fn() -> Secrets<'static>> = OnceLock::new();
pub(crate) fn source(read: fn() -> Secrets<'static>) {
    let _ = SOURCE.set(read);
}
pub(super) fn current() -> Secrets<'static> {
    static EMPTY: OnceLock<[&str; 3]> = OnceLock::new();
    SOURCE.get().map_or(
        Secrets {
            values: &[],
            credentials: &EMPTY,
        },
        |read| read(),
    )
}

struct Masked<'a, 'b> {
    secrets: &'b Secrets<'a>,
    output: &'b mut [u8],
    used: usize,
    cut: bool,
    // Always borrowed from a secret; arbitrarily long prefixes need no buffer.
    pending: &'a str,
    // Leading bytes already covered by a complete match, including overlaps.
    covered: usize,
}
impl<'a> Masked<'a, '_> {
    fn values(&self) -> impl Iterator<Item = &'a str> + use<'a, '_> {
        self.secrets
            .values
            .iter()
            .copied()
            .chain(
                self.secrets
                    .credentials
                    .get()
                    .into_iter()
                    .flatten()
                    .copied(),
            )
            .filter(|s| !s.is_empty())
    }
    fn emit(&mut self, text: &str) {
        if self.cut {
            return;
        }
        let mut n = text.len().min(self.output.len().saturating_sub(self.used));
        while !text.is_char_boundary(n) {
            n -= 1;
        }
        if let Some(dst) = self.output.get_mut(self.used..self.used + n) {
            dst.copy_from_slice(text.get(..n).unwrap_or_default().as_bytes());
        }
        self.used += n;
        self.cut |= n < text.len();
    }
    fn matches(&mut self) {
        let matched = self
            .values()
            .filter(|s| self.pending.starts_with(s))
            .map(str::len)
            .max()
            .unwrap_or(0);
        if matched > 0 {
            if self.covered == 0 {
                self.emit("***");
            }
            self.covered = self.covered.max(matched);
        }
    }
    fn shift(&mut self) {
        // A shorter secret may begin inside a failed longer candidate.
        self.matches();
        let Some(c) = self.pending.chars().next() else {
            return;
        };
        let n = c.len_utf8();
        if self.covered == 0 {
            self.emit(self.pending.get(..n).unwrap_or_default());
        }
        self.covered = self.covered.saturating_sub(n);
        self.pending = self.pending.get(n..).unwrap_or_default();
    }
}
impl Write for Masked<'_, '_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for c in text.chars() {
            let mut bytes = [0; 4];
            let next = c.encode_utf8(&mut bytes);
            loop {
                if self.cut {
                    return Ok(());
                }
                let candidate = self.values().find(|s| {
                    s.starts_with(self.pending)
                        && s.get(self.pending.len()..)
                            .is_some_and(|tail| tail.starts_with(&*next))
                });
                if let Some(candidate) = candidate {
                    self.pending = candidate
                        .get(..self.pending.len() + next.len())
                        .unwrap_or_default();
                    self.matches();
                    break;
                }
                if self.pending.is_empty() {
                    self.emit(next);
                    break;
                }
                self.shift();
            }
        }
        Ok(())
    }
}
impl Secrets<'_> {
    pub(super) fn format(&self, args: fmt::Arguments<'_>, output: &mut [u8]) -> (usize, bool) {
        let mut writer = Masked {
            secrets: self,
            output,
            used: 0,
            cut: false,
            pending: "",
            covered: 0,
        };
        let _ = writer.write_fmt(args);
        while !writer.pending.is_empty() && !writer.cut {
            writer.shift();
        }
        (writer.used, writer.cut)
    }
}
