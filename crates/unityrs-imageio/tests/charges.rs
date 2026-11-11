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
//! tally records how far the heap ever rose past what was held: at each
//! allocation, and as each charge is dropped, against the charges held
//! without it, so a charge given back before its buffer is freed shows. It
//! also counts the blocks of the output's planned size and every
//! `realloc`, so the output is seen made once, at its size, and never
//! grown. The allocator refuses a request only on the counted thread.

use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_imageio::{
    Buffer, Charger, DEFLATE_STATE_REQUESTS, EncodeError, Image, ImageError, Mode, Plan, encode,
    plan,
};
use unityrs_testkit::counted::{
    Allocator, Tally, count_this_thread, counted, others_are_not_counted,
};

/// Bytes live on the counted thread.
static LIVE: AtomicUsize = AtomicUsize::new(0);
/// [`LIVE`] when the measure began.
static BASE: AtomicUsize = AtomicUsize::new(0);
/// Bytes of the charges granted and not yet dropped.
static HELD: AtomicUsize = AtomicUsize::new(0);
/// How far the heap above [`BASE`] ever passed [`HELD`].
static OVER: AtomicUsize = AtomicUsize::new(0);
/// A request size the allocator refuses on the counted thread (0: none).
static REFUSE: AtomicUsize = AtomicUsize::new(0);
/// The size of the blocks [`OUTPUTS`] counts: the output's plan (0: none).
static OUTPUT: AtomicUsize = AtomicUsize::new(0);
/// Blocks of [`OUTPUT`] bytes made on the counted thread.
static OUTPUTS: AtomicUsize = AtomicUsize::new(0);
/// `realloc` calls on the counted thread.
static REALLOCS: AtomicUsize = AtomicUsize::new(0);

struct Watching;

impl Tally for Watching {
    fn refuses(&self, size: usize) -> bool {
        // The allocator asks on every thread; libtest's main thread, which
        // allocates beside the test, must never see a refusal.
        counted() && size != 0 && size == REFUSE.load(Ordering::Relaxed)
    }
    fn refuses_realloc(&self, new_size: usize) -> bool {
        if counted() {
            REALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        self.refuses(new_size)
    }
    fn grew(&self, size: usize, _zeroed: bool) {
        if size != 0 && size == OUTPUT.load(Ordering::Relaxed) {
            OUTPUTS.fetch_add(1, Ordering::Relaxed);
        }
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
        // What is live must fit the charges still held once this one is
        // given back: its buffer is freed first.
        let live = LIVE.load(Ordering::Relaxed);
        let above = live.saturating_sub(BASE.load(Ordering::Relaxed));
        let rest = HELD.load(Ordering::Relaxed).saturating_sub(self.0);
        OVER.fetch_max(above.saturating_sub(rest), Ordering::Relaxed);
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
    OUTPUTS.store(0, Ordering::Relaxed);
    REALLOCS.store(0, Ordering::Relaxed);
}

/// How far the heap passed the charges since [`start`] (at an allocation,
/// or as a charge was given back), the charges held, and what is live above
/// where it started.
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
        OUTPUT.store(plan.output, Ordering::Relaxed);
        start();
        let png = encode(&image, &mut charger).unwrap();
        let made = (
            OUTPUTS.load(Ordering::Relaxed),
            REALLOCS.load(Ordering::Relaxed),
        );
        OUTPUT.store(0, Ordering::Relaxed);
        let over = OVER.load(Ordering::Relaxed);
        // The output is one allocation of its planned size, never grown.
        assert_eq!(
            made,
            (1, 0),
            "{width}x{height} {mode}: (blocks of the output's size, reallocs)"
        );
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
        // The test's own blocks above (`seen`'s) may have passed the
        // charges; the drop is measured alone: the bytes are freed before
        // their charge is given back.
        OVER.store(0, Ordering::Relaxed);
        drop(png);
        assert_eq!(
            OVER.load(Ordering::Relaxed),
            0,
            "{width}x{height} {mode}: the output's charge given back before its bytes"
        );
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
    let zero = |width, height| Err(ImageError::ZeroSide { width, height });
    assert_eq!(plan(0, 1, Mode::L), zero(0, 1));
    assert_eq!(plan(1, 0, Mode::Rgba), zero(1, 0));
    assert_eq!(plan(0, u32::MAX, Mode::Rgba), zero(0, u32::MAX));
    // Pillow's encoder takes rows of at most `INT_MAX / bits - 7` pixels
    // (Pillow 9.5.0, `src/encode.c:261`), each mode's widest here, and
    // raises MemoryError one past it.
    for (mode, widest) in [
        (Mode::L, 268_435_448_u32),
        (Mode::La, 134_217_720),
        (Mode::Rgb, 89_478_478),
        (Mode::Rgba, 67_108_856),
    ] {
        let p = plan(widest, 1, mode).unwrap_or_else(|e| panic!("{mode} {widest}: {e}"));
        assert_eq!(p.rows, [widest as usize * mode.channels() + 1; 5], "{mode}");
        assert_eq!(p.idat_size, 4 * widest as usize, "{mode}");
        for width in [widest + 1, u32::MAX] {
            assert_eq!(
                plan(width, 1, mode),
                Err(ImageError::TooWide {
                    width,
                    mode,
                    widest
                }),
                "{mode} {width}"
            );
        }
        // Too wide with a zero height: `_setimage` checks the sides first
        // (`src/encode.c:253-257`), so Pillow's error is the zero side's.
        assert_eq!(plan(widest + 1, 0, mode), zero(widest + 1, 0), "{mode}");
    }
    // The crate's own bound: Pillow's sides are C ints.
    assert!(plan(1, 0x7FFF_FFFF, Mode::L).is_ok());
    assert_eq!(
        plan(1, 0x8000_0000, Mode::L),
        Err(ImageError::TooLarge {
            width: 1,
            height: 0x8000_0000
        })
    );
    // The deflate state is a constant: its slots' requests, about 398 KiB.
    let state: usize = DEFLATE_STATE_REQUESTS.iter().sum();
    assert_eq!(state, (6 << 10) + 2 * (64 << 10) + 2 * (128 << 10) + 5 * 8);
    let p = plan(100, 10, Mode::Rgb).unwrap();
    assert_eq!(p.rows, [301; 5]);
    assert_eq!(p.filtered, 3010);
    assert_eq!(p.idat_size, 65536);
    assert_eq!(plan(20000, 1, Mode::L).unwrap().idat_size, 80000);
}
