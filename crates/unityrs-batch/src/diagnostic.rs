//! Bounded diagnostic construction. The host prepares immutable borrowed
//! secrets before running; a set-once credential slot can extend that list.
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering::Relaxed},
};
pub(crate) mod format;
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
/// Count a startup line before a controller can finish. A process that exits
/// during its write reports it as unresolved, just like the early writer.
pub(crate) struct PendingLine<'a>(&'a Writer);
#[cfg(test)]
#[path = "diagnostic/retry_tests.rs"]
mod retry_tests;
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
    /// Includes a startup warning still being written when sampled.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Relaxed)
    }
    pub(crate) fn pending_line(&self) -> PendingLine<'_> {
        self.dropped.fetch_add(1, Relaxed);
        PendingLine(self)
    }
}
impl PendingLine<'_> {
    /// Mask once and retry within the early-output budget. Only a complete
    /// write clears the outstanding line; expiry leaves its one drop counted.
    pub(crate) fn within(
        self,
        text: &str,
        secrets: &Secrets<'_>,
        buffer: &mut [u8; LINE_BYTES],
        wait: std::time::Duration,
    ) {
        let n = secrets.mask(text, buffer);
        if crate::sys::stderr::within(self.0.route, buffer.get(..n).unwrap_or_default(), wait) {
            self.0.dropped.fetch_sub(1, Relaxed);
        }
        #[cfg(any(test, feature = "test-hooks"))]
        crate::sys::stderr::retry_finished();
    }
}

/// No panic hook writes stderr. Each service thread has a fixed const TLS
/// slot; the body's `catch_unwind` reads it after hand-over. Pool `JoinError`s
/// carry their own payload to the controller instead.
pub mod panic {
    use std::cell::RefCell;
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
    thread_local! { static SLOT: RefCell<Slot> = const { RefCell::new(Slot::new()) }; }
    pub fn install() {
        std::panic::set_hook(Box::new(|info| {
            SLOT.with(|slot| {
                let Ok(mut slot) = slot.try_borrow_mut() else {
                    return;
                };
                *slot = Slot::new();
                let payload = info
                    .payload()
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("non-string panic payload");
                let mut capture = |args| {
                    let (len, cut) = super::format::current().format(args, &mut slot.bytes);
                    slot.len = len;
                    slot.cut = cut;
                };
                if let Some(location) = info.location() {
                    capture(format_args!(
                        "{}:{}: {payload}",
                        location.file(),
                        location.line()
                    ));
                } else {
                    capture(format_args!("{payload}"));
                }
            });
        }));
    }
    #[must_use]
    pub fn read() -> Slot {
        SLOT.with(|s| *s.borrow())
    }
}

/// Stderr lines a run failure's catch site leaves for the process host
/// (2.4, 2.11's *Panics*): a worker's, a blocking-pool task's or the
/// controller drive's caught panic, its location and payload read from the
/// panicking thread's slot, or a host thread's. The host writes them after
/// its hand-over, through its writer, so no stop waits on stderr. Fixed
/// storage: a line is cut to [`pending::LINE`] bytes, its `\n` kept.
///
/// The queue is a ring of [`pending::LINES`]: a drain frees each slot it
/// copies out. Between drains it is not enough for every panic a run can
/// admit at once (each worker's and each pool task's, `threads + 2 x
/// (download_concurrency + 1)` and more): the first stop is the run's
/// cause and its line is queued first, so the lines past the ring's room
/// are counted, never allocated, and the drain writes one line for them,
/// `unityrs batch: <n> more internal lines dropped`; the head's
/// `stderr_dropped` counts them too ([`pending::unwritten`]). So does a
/// line a drain leaves unwritten because another drain held the ring past
/// its bound ([`pending::drain`]).
pub mod pending {
    use std::{
        sync::{
            Mutex, MutexGuard, PoisonError, TryLockError,
            atomic::{AtomicU64, Ordering::SeqCst},
        },
        time::{Duration, Instant},
    };
    pub const LINES: usize = 8;
    pub const LINE: usize = 1_280;
    /// The pause between two tries of a drain lock another drain holds.
    const RETRY: Duration = Duration::from_millis(1);
    pub(crate) struct Queue {
        lines: [[u8; LINE]; LINES],
        lens: [usize; LINES],
        /// The oldest line's slot, and how many are queued.
        head: usize,
        count: usize,
        /// Lines not written and not yet in a drain's count line: past the
        /// ring's room, or left by a drain past its bound. Counted and
        /// taken under the ring's lock alone ([`Ring::drain`]).
        dropped: u64,
    }
    impl Queue {
        pub(crate) const EMPTY: Self = Self {
            lines: [[0; LINE]; LINES],
            lens: [0; LINES],
            head: 0,
            count: 0,
            dropped: 0,
        };
        /// `args` in the next free slot ([`fit`]); false when none is free.
        pub(crate) fn push(&mut self, args: std::fmt::Arguments<'_>) -> bool {
            if self.count >= LINES {
                return false;
            }
            let at = (self.head + self.count) % LINES;
            let Some(storage) = self.lines.get_mut(at) else {
                return false;
            };
            let len = fit(args, storage);
            if let Some(slot) = self.lens.get_mut(at) {
                *slot = len;
            }
            self.count += 1;
            true
        }
        /// The oldest line copied into `line`, its slot freed: its length.
        pub(crate) fn pop(&mut self, line: &mut [u8; LINE]) -> Option<usize> {
            if self.count == 0 {
                return None;
            }
            let at = self.head;
            let len = self.lens.get(at).copied().unwrap_or(0).min(LINE);
            if let (Some(dst), Some(src)) = (
                line.get_mut(..len),
                self.lines.get(at).and_then(|l| l.get(..len)),
            ) {
                dst.copy_from_slice(src);
            }
            self.head = (at + 1) % LINES;
            self.count -= 1;
            Some(len)
        }
        /// Every queued line's slot freed, unread: how many there were.
        fn discard(&mut self) -> u64 {
            let count = self.count;
            self.head = (self.head + count) % LINES;
            self.count = 0;
            u64::try_from(count).unwrap_or(u64::MAX)
        }
    }
    /// The ring, its drain lock and its count of lines never written: the
    /// process's one ([`line`], [`drain`]), or a test's.
    pub(crate) struct Ring {
        queue: Mutex<Queue>,
        /// Held by a drain from each take through its write: two drains
        /// never interleave, so the lines keep their order, and a drain
        /// that takes it after another has waited for every line the
        /// other took to be written first. [`Self::queue`] itself is held
        /// only around a push, a take or a count, so no queueing thread
        /// waits on stderr.
        drain: Mutex<()>,
        /// Every queued line, or one past the ring's room, never written;
        /// counted under the ring's lock, read without it.
        unwritten: AtomicU64,
    }
    impl Ring {
        pub(crate) const fn new() -> Self {
            Self {
                queue: Mutex::new(Queue::EMPTY),
                drain: Mutex::new(()),
                unwritten: AtomicU64::new(0),
            }
        }
        fn queue(&self) -> MutexGuard<'_, Queue> {
            self.queue.lock().unwrap_or_else(PoisonError::into_inner)
        }
        /// [`line`] on this ring.
        pub(crate) fn line(&self, args: std::fmt::Arguments<'_>) {
            let mut queue = self.queue();
            if !queue.push(args) {
                self.count(&mut queue, 1);
            }
        }
        /// `n` lines not written, counted under the ring's lock `queue`:
        /// for the next count line, and in all.
        fn count(&self, queue: &mut Queue, n: u64) {
            queue.dropped = queue.dropped.saturating_add(n);
            self.unwritten.fetch_add(n, SeqCst);
        }
        /// [`drain`] on this ring.
        pub(crate) fn drain(&self, until: Option<Instant>, write: impl FnMut(&str)) {
            self.drain_at(until, write, || {});
        }
        /// [`Self::drain`], `discarded` called when a drain past its bound
        /// has discarded the lines left and not yet counted them, the
        /// ring's lock held: a test's seam, nothing in the process's.
        pub(super) fn drain_at(
            &self,
            until: Option<Instant>,
            mut write: impl FnMut(&str),
            discarded: impl FnOnce(),
        ) {
            let writing = match until {
                None => self.drain.lock().unwrap_or_else(PoisonError::into_inner),
                Some(until) => loop {
                    // Past the bound the last try is made under the ring's
                    // lock, which the holder's last look at the ring takes
                    // too: the holder has let go, or it will see the count.
                    let queue = (Instant::now() >= until).then(|| self.queue());
                    match self.drain.try_lock() {
                        Ok(writing) => break writing,
                        Err(TryLockError::Poisoned(writing)) => break writing.into_inner(),
                        Err(TryLockError::WouldBlock) => {
                            let Some(mut queue) = queue else {
                                std::thread::sleep(RETRY);
                                continue;
                            };
                            let left = queue.discard();
                            discarded();
                            self.count(&mut queue, left);
                            return;
                        }
                    }
                },
            };
            let mut line = [0_u8; LINE];
            loop {
                // The ring's lock released before each write; the drain's
                // kept until a look finds nothing left, under the ring's.
                let mut queue = self.queue();
                let len = if let Some(len) = queue.pop(&mut line) {
                    len
                } else {
                    let dropped = std::mem::take(&mut queue.dropped);
                    if dropped == 0 {
                        drop(writing);
                        return;
                    }
                    fit(
                        format_args!("unityrs batch: {dropped} more internal lines dropped\n"),
                        &mut line,
                    )
                };
                drop(queue);
                write(std::str::from_utf8(line.get(..len).unwrap_or_default()).unwrap_or_default());
            }
        }
        /// [`unwritten`] on this ring.
        pub(crate) fn unwritten(&self) -> u64 {
            self.unwritten.load(SeqCst)
        }
    }
    static RING: Ring = Ring::new();
    /// `args` into `line`: cut at `LINE - 1` bytes on a UTF-8 boundary,
    /// then ended by its `\n`. Gives its length.
    pub(crate) fn fit(args: std::fmt::Arguments<'_>, line: &mut [u8; LINE]) -> usize {
        let (text, end) = line.split_at_mut(LINE - 1);
        let (len, _) = super::format::current().format(args, text);
        if len > 0 && text.get(len - 1) == Some(&b'\n') {
            return len;
        }
        // The cut (or a text without one) ends with its `\n` all the same.
        match text.get_mut(len) {
            Some(b) => *b = b'\n',
            None => {
                if let Some(b) = end.first_mut() {
                    *b = b'\n';
                }
            }
        }
        len + 1
    }
    /// Queue `unityrs batch: <who>internal: <location: payload>`, `who`
    /// being empty or `<kind> <id>: `, from the panicking thread's slot.
    pub fn panic(who: std::fmt::Arguments<'_>) {
        let slot = super::panic::read();
        let text =
            std::str::from_utf8(slot.bytes.get(..slot.len).unwrap_or_default()).unwrap_or_default();
        line(format_args!("unityrs batch: {who}internal: {text}\n"));
    }
    /// Queue one line ([`fit`]); one past the ring's room is counted.
    pub fn line(args: std::fmt::Arguments<'_>) {
        RING.line(args);
    }
    /// Hand each queued line to `write`, in order, then one line for the
    /// lines not written since the last such line, and again, until a look
    /// at the ring finds neither. Each line is copied out under the ring's
    /// lock and written with it released, so no queueing thread waits on
    /// stderr; the drain lock is held throughout, and let go under the
    /// ring's lock at that last look. While another drain holds it, this
    /// one waits: with `until` `None`, as long as the other writes (the
    /// controller's, which only parks after); else no later than `until`,
    /// after which, under the ring's lock, it takes the lines left and
    /// counts them, unwritten, for the holder's count line and the head
    /// ([`unwritten`]). A line counted past the ring's room, or a drain's
    /// count, lands before the holder's last look, which writes it, or
    /// after it, when no drain holds the lock: then it waits, counted, for
    /// the next drain. So a drain held in a write nothing bounds (a
    /// regular file's, under writeback) never holds the finisher's exit or
    /// an exit at once past its bound, and no line it takes is lost
    /// uncounted or written out of order.
    pub fn drain(until: Option<Instant>, write: impl FnMut(&str)) {
        RING.drain(until, write);
    }
    /// Every queued line never written so far: past the ring's room, or
    /// left by a drain past its bound (the head's `stderr_dropped`).
    #[must_use]
    pub fn unwritten() -> u64 {
        RING.unwritten()
    }
}

#[cfg(test)]
mod tests;
