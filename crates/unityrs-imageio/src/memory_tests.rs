//! The crate's own measures under a counting allocator
//! (`docs/plans/extract.md` E59). They run among the unit tests, for what
//! only a unit test reaches:
//!
//! - the visible hash allocates nothing (`crate::hash`);
//! - a block of zlib's past its slot ends the encode with nothing live and
//!   every charge given back: `zalloc`'s fail-safe, which only a narrowed
//!   slot reaches, and `tests/charges.rs`, built without `cfg(test)`,
//!   cannot narrow one (`crate::zlib`'s `TEST_SLOTS`).
//!
//! This module installs the counting allocator for the crate's unit tests.
//! It counts only the thread under test (`unityrs_testkit::counted`), so
//! the other unit tests allocate unseen; the measures share its counts, so
//! one test runs them in turn.

use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_testkit::counted::{Allocator, Tally, count_this_thread, others_are_not_counted};

use crate::zlib::{STATE_SLOTS, TEST_SLOTS};
use crate::{Buffer, Charger, EncodeError, Image, Mode, ZlibFailure, encode, visible_hash};

/// Bytes live on the counted thread.
static LIVE: AtomicUsize = AtomicUsize::new(0);
/// Blocks made on the counted thread, a `realloc`'s new block among them.
static MADE: AtomicUsize = AtomicUsize::new(0);
/// Bytes of the charges granted and not yet given back.
static HELD: AtomicUsize = AtomicUsize::new(0);

struct Counting;

impl Tally for Counting {
    fn grew(&self, size: usize, _zeroed: bool) {
        LIVE.fetch_add(size, Ordering::Relaxed);
        MADE.fetch_add(1, Ordering::Relaxed);
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(size, Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOC: Allocator<Counting> = Allocator(Counting);

/// What `f` returns, and the blocks it made.
fn made_by<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = MADE.load(Ordering::Relaxed);
    let out = f();
    (out, MADE.load(Ordering::Relaxed) - before)
}

#[test]
fn the_measures_under_a_counting_allocator() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    the_hash_allocates_nothing();
    a_block_past_its_slot_leaves_nothing_live();
}

fn the_hash_allocates_nothing() {
    let pixels: Vec<u8> = (0..4 * 3000_u32).map(|i| i.to_le_bytes()[0]).collect();
    // Every mode; one pixel, and images of several chunks of the hash's
    // stack buffer.
    for mode in Mode::ALL {
        for (width, height) in [(1, 1), (3000, 1), (1000, 3)] {
            let n = (width * height) as usize * mode.channels();
            let image = Image::new(width, height, mode, &pixels[..n]).unwrap();
            let (_, made) = made_by(|| visible_hash(&image));
            assert_eq!(made, 0, "{width}x{height} {mode}: blocks the hash made");
        }
    }
    // A side of `u32::MAX`, the widest the prefix writes.
    let widest = Image::new(u32::MAX, 0, Mode::Rgba, &[]).unwrap();
    let (_, made) = made_by(|| visible_hash(&widest));
    assert_eq!(made, 0, "4294967295x0: blocks the hash made");
}

/// A granted charge: its bytes stay in [`HELD`] until it is dropped.
struct Held(usize);

impl Drop for Held {
    fn drop(&mut self) {
        HELD.fetch_sub(self.0, Ordering::Relaxed);
    }
}

/// Grants every charge, keeping the buffers it was asked for in room of
/// its own, so it allocates nothing.
struct Granting {
    asked: [Option<Buffer>; 3],
    n: usize,
}

impl Charger for Granting {
    type Charge = Held;
    type Refusal = ();

    fn charge(&mut self, buffer: Buffer, requests: &[usize]) -> Result<Held, ()> {
        if let Some(slot) = self.asked.get_mut(self.n) {
            *slot = Some(buffer);
        }
        self.n += 1;
        let n = requests.iter().sum();
        HELD.fetch_add(n, Ordering::Relaxed);
        Ok(Held(n))
    }
}

fn a_block_past_its_slot_leaves_nothing_live() {
    let pixels = [7_u8; 4 * 30];
    let image = Image::new(30, 1, Mode::Rgba, &pixels).unwrap();
    // The state's slot narrowed, so its first block is refused; and the
    // pending buffer's, the fifth, refused once deflateInit2 has the four
    // before it, which it frees.
    for (slot, made_before) in [(0, 0), (4, 4)] {
        let mut slots = STATE_SLOTS;
        slots[slot] = if slot == 0 { 64 } else { STATE_SLOTS[slot] - 1 };
        TEST_SLOTS.with(|s| s.set(slots));
        let mut charger = Granting {
            asked: [None; 3],
            n: 0,
        };
        let live = LIVE.load(Ordering::Relaxed);
        let (e, made) = made_by(|| encode(&image, &mut charger).unwrap_err());
        TEST_SLOTS.with(|s| s.set(STATE_SLOTS));
        assert!(
            matches!(
                e,
                EncodeError::Zlib {
                    what: ZlibFailure::StateOverAllowance
                }
            ),
            "slot {slot}: {e:?}"
        );
        assert_eq!(made, made_before, "slot {slot}: blocks zlib had");
        assert_eq!(
            LIVE.load(Ordering::Relaxed),
            live,
            "slot {slot}: bytes left live"
        );
        assert_eq!(HELD.load(Ordering::Relaxed), 0, "slot {slot}: charges held");
        assert_eq!(
            (charger.n, charger.asked[0]),
            (1, Some(Buffer::DeflateState)),
            "slot {slot}: the charges asked for"
        );
    }
}
