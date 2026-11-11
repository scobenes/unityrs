//! The encoder pays before it allocates (`docs/plans/extract.md` E59): every
//! block it takes, zlib's deflate state included, is charged before the
//! allocator sees it, the charges are exactly what [`plan`] says, a refusal
//! (the charger's or the allocator's) is an error and allocates nothing
//! more, and every block is freed before its charge is given back.
//!
//! A counting global allocator watches the heap of the test's own thread
//! (`unityrs_testkit::counted`): libtest's main thread allocates beside it.
//! The measures share the allocator's counts, so one test runs them in turn.
//! Each charge the test grants adds to `HELD` until it is dropped, and the
//! tally records how far the heap ever rose past what was held.

use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_imageio::{
    Buffer, Charger, DEFLATE_STATE_REQUESTS, EncodeError, Image, Mode, Plan, encode, plan,
};
use unityrs_testkit::counted::{Allocator, Tally, count_this_thread, others_are_not_counted};

/// Bytes live on the counted thread.
static LIVE: AtomicUsize = AtomicUsize::new(0);
/// [`LIVE`] when the measure began.
static BASE: AtomicUsize = AtomicUsize::new(0);
/// Bytes of the charges granted and not yet dropped.
static HELD: AtomicUsize = AtomicUsize::new(0);
/// How far the heap above [`BASE`] ever passed [`HELD`].
static OVER: AtomicUsize = AtomicUsize::new(0);
/// A request size the allocator refuses (0: none).
static REFUSE: AtomicUsize = AtomicUsize::new(0);

struct Watching;

impl Tally for Watching {
    fn refuses(&self, size: usize) -> bool {
        size != 0 && size == REFUSE.load(Ordering::Relaxed)
    }
    fn grew(&self, size: usize, _zeroed: bool) {
        let now = LIVE.fetch_add(size, Ordering::Relaxed) + size;
        let above = now.saturating_sub(BASE.load(Ordering::Relaxed));
        let over = above.saturating_sub(HELD.load(Ordering::Relaxed));
        OVER.fetch_max(over, Ordering::Relaxed);
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(size, Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOC: Allocator<Watching> = Allocator(Watching);

/// A granted charge: its bytes stay in [`HELD`] until it is dropped.
#[derive(Debug)]
struct Held(usize);

impl Drop for Held {
    fn drop(&mut self) {
        HELD.fetch_sub(self.0, Ordering::Relaxed);
    }
}

/// Grants every charge but `refuse`'s, recording each in room it has
/// before the encode, so it allocates nothing the encode is measured for.
struct Recording {
    room: Vec<(Buffer, [usize; 5], usize)>,
    refuse: Option<Buffer>,
}

impl Recording {
    fn new(refuse: Option<Buffer>) -> Self {
        Self {
            room: Vec::with_capacity(8),
            refuse,
        }
    }

    /// The charges asked for, in order.
    fn seen(&self) -> Vec<(Buffer, Vec<usize>)> {
        self.room
            .iter()
            .map(|(b, r, n)| (*b, r[..*n].to_vec()))
            .collect()
    }
}

impl Charger for Recording {
    type Charge = Held;
    type Refusal = &'static str;

    fn charge(&mut self, buffer: Buffer, requests: &[usize]) -> Result<Held, &'static str> {
        assert!(self.room.len() < self.room.capacity() && requests.len() <= 5);
        let mut r = [0; 5];
        r[..requests.len()].copy_from_slice(requests);
        self.room.push((buffer, r, requests.len()));
        if self.refuse == Some(buffer) {
            return Err("refused");
        }
        let n = requests.iter().sum();
        HELD.fetch_add(n, Ordering::Relaxed);
        Ok(Held(n))
    }
}

/// An image of noise from xorshift, so zlib's output is near its bound.
fn noise(width: u32, height: u32, mode: Mode) -> Vec<u8> {
    let mut x = 0x2545_F491_4F6C_DD1D_u64;
    let n = width as usize * height as usize * mode.channels();
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x.to_le_bytes()[3]
        })
        .collect()
}

fn start() {
    BASE.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    OVER.store(0, Ordering::Relaxed);
}

/// How far the heap passed the charges since [`start`], the charges held,
/// and what is live above where it started.
fn settled() -> (usize, usize, usize) {
    let live = LIVE.load(Ordering::Relaxed) - BASE.load(Ordering::Relaxed);
    (
        OVER.load(Ordering::Relaxed),
        HELD.load(Ordering::Relaxed),
        live,
    )
}

fn expected(plan: &Plan) -> Vec<(Buffer, Vec<usize>)> {
    vec![
        (Buffer::DeflateState, plan.deflate_state.to_vec()),
        (Buffer::Output, vec![plan.output]),
        (Buffer::Rows, plan.rows.to_vec()),
    ]
}

#[test]
fn the_encoder_pays_before_it_allocates() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    every_block_is_charged_before_it_is_allocated();
    a_refused_charge_allocates_nothing_more();
    a_refused_allocation_is_an_error();
}

fn every_block_is_charged_before_it_is_allocated() {
    for (width, height, mode) in [
        (1, 1, Mode::L),
        (300, 200, Mode::Rgba),
        (20000, 3, Mode::Rgb),
        (7, 9000, Mode::La),
    ] {
        let pixels = noise(width, height, mode);
        let image = Image::new(width, height, mode, &pixels).unwrap();
        let plan = plan(width, height, mode).unwrap();
        let mut charger = Recording::new(None);
        start();
        let png = encode(&image, &mut charger).unwrap();
        let over = OVER.load(Ordering::Relaxed);
        assert_eq!(
            over, 0,
            "{width}x{height} {mode}: {over} bytes past the charges"
        );
        assert_eq!(charger.seen(), expected(&plan), "{width}x{height} {mode}");
        // Only the output's charge is still held, with the output.
        assert_eq!(HELD.load(Ordering::Relaxed), plan.output);
        let live = LIVE.load(Ordering::Relaxed) - BASE.load(Ordering::Relaxed);
        assert_eq!(
            live, plan.output,
            "{width}x{height} {mode}: what the encode kept"
        );
        assert!(png.bytes().len() < plan.output);
        drop(png);
        assert_eq!(HELD.load(Ordering::Relaxed), 0);
        assert_eq!(LIVE.load(Ordering::Relaxed), BASE.load(Ordering::Relaxed));
    }
}

fn a_refused_charge_allocates_nothing_more() {
    let (width, height, mode) = (300, 200, Mode::Rgba);
    let pixels = noise(width, height, mode);
    let image = Image::new(width, height, mode, &pixels).unwrap();
    let plan = plan(width, height, mode).unwrap();
    let all = expected(&plan);
    for (at, (buffer, _)) in all.iter().enumerate() {
        let mut charger = Recording::new(Some(*buffer));
        start();
        let e = encode(&image, &mut charger).unwrap_err();
        let after = settled();
        assert!(
            matches!(e, EncodeError::Refused { buffer: b, refusal: "refused" } if b == *buffer),
            "{e:?}"
        );
        assert_eq!(charger.seen(), all[..=at], "{buffer:?}");
        assert_eq!(after, (0, 0, 0), "{buffer:?}: (over, held, live)");
    }
}

fn a_refused_allocation_is_an_error() {
    let (width, height, mode) = (300, 200, Mode::Rgba);
    let pixels = noise(width, height, mode);
    let image = Image::new(width, height, mode, &pixels).unwrap();
    let plan = plan(width, height, mode).unwrap();
    // zlib's window (`deflateInit2`'s second block), the output, a row.
    for refused in [DEFLATE_STATE_REQUESTS[1], plan.output, plan.rows[0]] {
        let mut charger = Recording::new(None);
        start();
        REFUSE.store(refused, Ordering::Relaxed);
        let e = encode(&image, &mut charger).unwrap_err();
        REFUSE.store(0, Ordering::Relaxed);
        let after = settled();
        if refused == DEFLATE_STATE_REQUESTS[1] {
            assert!(matches!(e, EncodeError::ZlibMemory), "{e:?}");
        } else {
            let buffer = if refused == plan.output {
                Buffer::Output
            } else {
                Buffer::Rows
            };
            assert!(
                matches!(e, EncodeError::Alloc { buffer: b, request } if b == buffer && request == refused),
                "{e:?}"
            );
        }
        assert_eq!(after, (0, 0, 0), "{refused}: (over, held, live)");
    }
}

#[test]
fn the_plan_refuses_what_pillow_refuses() {
    assert!(plan(0, 1, Mode::L).is_err());
    assert!(plan(1, 0, Mode::Rgba).is_err());
    // Pillow's encoder buffer, `4 x width` bytes, is a C int.
    assert!(plan(0x1FFF_FFFF, 1, Mode::Rgba).is_ok());
    assert!(plan(0x2000_0000, 1, Mode::L).is_err());
    assert!(plan(1, 0x7FFF_FFFF, Mode::L).is_ok());
    assert!(plan(1, 0x8000_0000, Mode::L).is_err());
    // The deflate state is a constant: its slots' requests, about 398 KiB.
    let state: usize = DEFLATE_STATE_REQUESTS.iter().sum();
    assert_eq!(state, (6 << 10) + 2 * (64 << 10) + 2 * (128 << 10) + 5 * 8);
    let p = plan(100, 10, Mode::Rgb).unwrap();
    assert_eq!(p.rows, [301; 5]);
    assert_eq!(p.filtered, 3010);
    assert_eq!(p.idat_size, 65536);
    assert_eq!(plan(20000, 1, Mode::L).unwrap().idat_size, 80000);
}
