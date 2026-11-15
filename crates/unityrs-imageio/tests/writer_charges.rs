//! Image writers charge every allocation first and release each charge
//! after its bytes. One thread performs all measurements so the allocator's
//! atomics cannot mix concurrent measured windows.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use unityrs_imageio::writers::{Format, SaveError, save};
use unityrs_imageio::{Buffer, Charger, EncodeError, Image, Mode};
use unityrs_testkit::counted::{
    Allocator, Tally, count_this_thread, counted, others_are_not_counted,
};

static LIVE: AtomicUsize = AtomicUsize::new(0);
static BASE: AtomicUsize = AtomicUsize::new(0);
static HELD: AtomicUsize = AtomicUsize::new(0);
static OVER: AtomicUsize = AtomicUsize::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
static REFUSE_AT: AtomicUsize = AtomicUsize::new(0);
static REALLOCS: AtomicUsize = AtomicUsize::new(0);
static BLOCKS: AtomicUsize = AtomicUsize::new(0);

struct Watching;

impl Tally for Watching {
    fn refuses(&self, size: usize) -> bool {
        if !counted() || !ACTIVE.load(Ordering::Relaxed) || size == 0 {
            return false;
        }
        let ordinal = ATTEMPTS.fetch_add(1, Ordering::Relaxed) + 1;
        ordinal == REFUSE_AT.load(Ordering::Relaxed)
    }

    fn refuses_realloc(&self, size: usize) -> bool {
        if counted() && ACTIVE.load(Ordering::Relaxed) {
            REALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        self.refuses(size)
    }

    fn grew(&self, size: usize, _: bool) {
        let now = LIVE.fetch_add(size, Ordering::Relaxed) + size;
        if ACTIVE.load(Ordering::Relaxed) {
            BLOCKS.fetch_add(1, Ordering::Relaxed);
            let above = now.saturating_sub(BASE.load(Ordering::Relaxed));
            OVER.fetch_max(
                above.saturating_sub(HELD.load(Ordering::Relaxed)),
                Ordering::Relaxed,
            );
        }
    }

    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(size, Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOC: Allocator<Watching> = Allocator(Watching);

#[derive(Debug)]
struct Held(usize);

impl Drop for Held {
    fn drop(&mut self) {
        let rest = HELD.load(Ordering::Relaxed).saturating_sub(self.0);
        if ACTIVE.load(Ordering::Relaxed) {
            let above = LIVE
                .load(Ordering::Relaxed)
                .saturating_sub(BASE.load(Ordering::Relaxed));
            OVER.fetch_max(above.saturating_sub(rest), Ordering::Relaxed);
        }
        HELD.fetch_sub(self.0, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Request {
    buffer: Buffer,
    sizes: [usize; 5],
    len: usize,
}

const EMPTY: Request = Request {
    buffer: Buffer::Output,
    sizes: [0; 5],
    len: 0,
};

struct Recording {
    requests: [Request; 256],
    len: usize,
    refuse: usize,
}

impl Recording {
    fn new(refuse: usize) -> Self {
        Self {
            requests: [EMPTY; 256],
            len: 0,
            refuse,
        }
    }
}

impl Charger for Recording {
    type Charge = Held;
    type Refusal = usize;

    fn charge(&mut self, buffer: Buffer, requests: &[usize]) -> Result<Held, usize> {
        assert!(self.len < self.requests.len() && requests.len() <= 5);
        let mut request = Request {
            buffer,
            sizes: [0; 5],
            len: requests.len(),
        };
        request.sizes[..requests.len()].copy_from_slice(requests);
        self.requests[self.len] = request;
        self.len += 1;
        if self.len == self.refuse {
            return Err(self.len);
        }
        let bytes = requests.iter().sum();
        HELD.fetch_add(bytes, Ordering::Relaxed);
        Ok(Held(bytes))
    }
}

fn start(refuse_allocation: usize) {
    assert_eq!(HELD.load(Ordering::Relaxed), 0);
    BASE.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    OVER.store(0, Ordering::Relaxed);
    ATTEMPTS.store(0, Ordering::Relaxed);
    REFUSE_AT.store(refuse_allocation, Ordering::Relaxed);
    REALLOCS.store(0, Ordering::Relaxed);
    BLOCKS.store(0, Ordering::Relaxed);
    ACTIVE.store(true, Ordering::Relaxed);
}

fn stop() -> (usize, usize, usize) {
    ACTIVE.store(false, Ordering::Relaxed);
    REFUSE_AT.store(0, Ordering::Relaxed);
    (
        OVER.load(Ordering::Relaxed),
        HELD.load(Ordering::Relaxed),
        LIVE.load(Ordering::Relaxed)
            .saturating_sub(BASE.load(Ordering::Relaxed)),
    )
}

fn exercise(image: &Image<'_>, format: Format, raw: bool) {
    let mut granted = Recording::new(0);
    start(0);
    let output = save(image, format, "sample.name", &mut granted).unwrap();
    let allocation_count = ATTEMPTS.load(Ordering::Relaxed);
    let output_size = output.bytes().len();
    let held = HELD.load(Ordering::Relaxed);
    let live = LIVE.load(Ordering::Relaxed) - BASE.load(Ordering::Relaxed);
    let reallocs = REALLOCS.load(Ordering::Relaxed);
    let blocks = BLOCKS.load(Ordering::Relaxed);
    drop(output);
    let settled = stop();
    assert_eq!(
        settled,
        (0, 0, 0),
        "{format:?} {}: (over, held, live)",
        image.mode()
    );
    assert_eq!(
        live, held,
        "{format:?}: only the returned output stays live"
    );
    assert_eq!(reallocs, 0, "{format:?}: buffers never grow");
    if raw {
        assert_eq!(blocks, 1, "{format:?}: exactly one allocation");
        assert_eq!(granted.len, 1, "{format:?}: exactly one charge");
        assert_eq!(
            granted.requests[0],
            Request {
                buffer: Buffer::Output,
                sizes: [output_size, 0, 0, 0, 0],
                len: 1
            }
        );
        assert_eq!(held, output_size, "{format:?}: final exact output capacity");
    }

    // Refuse each individual charge, including repeated Rows/Output
    // charges for the different icon sizes. Recording uses fixed stack
    // storage so a test helper cannot hide an uncharged writer allocation.
    for ordinal in 1..=granted.len {
        let mut charger = Recording::new(ordinal);
        start(0);
        let error = save(image, format, "sample.name", &mut charger).unwrap_err();
        let settled = stop();
        assert!(
            matches!(error, SaveError::Encode(EncodeError::Refused { refusal, buffer })
            if refusal == ordinal && buffer == granted.requests[ordinal - 1].buffer),
            "{format:?} charge {ordinal}: {error:?}"
        );
        assert_eq!(
            charger.len, ordinal,
            "{format:?}: stopped at the refused charge"
        );
        assert_eq!(&charger.requests[..ordinal], &granted.requests[..ordinal]);
        assert_eq!(
            settled,
            (0, 0, 0),
            "{format:?} charge {ordinal}: (over, held, live)"
        );
    }

    // Return null at each actual allocation ordinal. Fallible vector
    // allocations and zlib allocations must both become typed errors,
    // release every earlier allocation/charge, and request nothing more.
    for ordinal in 1..=allocation_count {
        let mut charger = Recording::new(0);
        start(ordinal);
        let error = save(image, format, "sample.name", &mut charger).unwrap_err();
        let attempted = ATTEMPTS.load(Ordering::Relaxed);
        let settled = stop();
        match error {
            SaveError::Encode(EncodeError::Alloc { .. }) => {
                assert_eq!(attempted, ordinal, "{format:?}: allocation after refusal");
            }
            SaveError::Encode(EncodeError::ZlibMemory) => {
                // deflateInit2 allocates four state blocks together before
                // checking their pointers, then frees all of them on error.
                assert!(attempted >= ordinal && attempted <= ordinal + 3);
                assert_eq!(
                    charger.requests[charger.len - 1].buffer,
                    Buffer::DeflateState
                );
            }
            _ => panic!("{format:?} allocation {ordinal}: {error:?}"),
        }
        assert_eq!(
            settled,
            (0, 0, 0),
            "{format:?} allocation {ordinal}: (over, held, live)"
        );
    }
}

fn supports(format: Format, mode: Mode) -> bool {
    match format {
        Format::Bmp | Format::Dib => mode != Mode::La,
        Format::Ppm => matches!(mode, Mode::L | Mode::Rgb | Mode::Rgba),
        Format::Pcx => matches!(mode, Mode::L | Mode::P | Mode::Rgb),
        Format::Sgi => matches!(mode, Mode::L | Mode::Rgb | Mode::Rgba),
        Format::Dds => mode != Mode::P,
        Format::Eps => matches!(mode, Mode::L | Mode::Rgb),
        _ => true,
    }
}

#[test]
fn every_writer_allocation_is_charged_and_fallible() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    let palette: [u8; 768] = std::array::from_fn(|i| (i * 73 + i / 3).to_le_bytes()[0]);
    for mode in Mode::ALL {
        let pixels: Vec<u8> = (0..23 * 17 * mode.channels())
            .map(|i| (i * 73 + i / 7).to_le_bytes()[0])
            .collect();
        let image = if mode == Mode::P {
            Image::with_palette(23, 17, &pixels, &palette).unwrap()
        } else {
            Image::new(23, 17, mode, &pixels).unwrap()
        };
        for format in [
            Format::Bmp,
            Format::Dib,
            Format::Tga,
            Format::Ppm,
            Format::Pcx,
            Format::Sgi,
            Format::Im,
            Format::Tiff,
            Format::Dds,
            Format::Eps,
        ] {
            if supports(format, mode) {
                exercise(&image, format, true);
            }
        }
        if mode == Mode::P {
            exercise(&image, Format::Png, false);
            let implicit = Image::new(23, 17, Mode::P, &pixels).unwrap();
            exercise(&implicit, Format::Png, false);
        }
    }
    // Palette ICNS exercises all PNG sizes cheaply through nearest
    // resizing; RGBA ICO also exercises the separable filter's temporary
    // rows, coefficient tables, bounds, and premultiplied-alpha storage.
    let pixels: Vec<u8> = (0_usize..33 * 29 * 4)
        .map(|i| (i * 19 + i / 11).to_le_bytes()[0])
        .collect();
    let image = Image::new(33, 29, Mode::Rgba, &pixels).unwrap();
    exercise(&image, Format::Ico, false);
    let pixels: Vec<u8> = (0_usize..17 * 13)
        .map(|i| (i * 73).to_le_bytes()[0])
        .collect();
    let image = Image::with_palette(17, 13, &pixels, &palette).unwrap();
    exercise(&image, Format::Icns, false);
}
