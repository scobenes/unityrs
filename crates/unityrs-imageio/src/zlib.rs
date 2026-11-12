//! The vendored zlib 1.2.13 (`build.rs`): its deflate, called as Pillow
//! 9.5.0 calls it for a PNG, with allocators of this crate's own.
//!
//! zlib allocates through [`zalloc`] and [`zfree`], which call Rust's
//! global allocator, as flate2 1.1.10's C backend does (its `src/ffi/c.rs`):
//! each block is the request rounded up to 8 bytes behind an 8-byte header
//! that holds its size for `zfree` (`docs/plans/batch.md` 2.6 step 10). So a
//! counting global allocator sees zlib's blocks and its refusals.
//!
//! The deflate state is charged before `deflateInit2` as a constant: the
//! five blocks `deflateInit2` asks for at Pillow's settings, each at most
//! its slot of [`STATE_SLOTS`]. [`zalloc`] holds zlib to those slots, in
//! order: a block past its slot, or a sixth, is refused, so zlib can never
//! hold memory that was not charged ([`Deflater::over_allowance`]).
//! zlib 1.2.13 allocates nowhere else: `deflate` and `deflateBound` take no
//! memory, and `deflateEnd` frees the five.

#![allow(unsafe_code)]

use std::alloc::{self, Layout};
use std::ffi::{CStr, c_char, c_int, c_uint, c_ulong, c_void};
use std::marker::PhantomPinned;
use std::pin::Pin;
use std::ptr;

#[cfg(feature = "test-reference")]
pub mod reference;

/// What zlib's `deflate` returns when it made progress.
pub(crate) const Z_OK: c_int = 0;
/// What `deflate(Z_FINISH)` returns once the stream is complete.
pub(crate) const Z_STREAM_END: c_int = 1;
/// A stream zlib does not know, or a call it cannot make on it: a bug.
const Z_STREAM_ERROR: c_int = -2;
/// zlib's allocator refused (here: Rust's global allocator, or the slots).
pub(crate) const Z_MEM_ERROR: c_int = -4;
pub(crate) const Z_NO_FLUSH: c_int = 0;
pub(crate) const Z_FINISH: c_int = 4;
const Z_DEFLATED: c_int = 8;
const Z_FILTERED: c_int = 1;
const Z_DEFAULT_COMPRESSION: c_int = -1;

/// Pillow 9.5.0's PNG settings (`src/libImaging/ZipEncode.c:91-101`):
/// `compress_level` -1 (zlib's level 6), windowBits 15, memLevel 9 and,
/// for a PNG, `Z_FILTERED`.
const LEVEL: c_int = Z_DEFAULT_COMPRESSION;
const WINDOW_BITS: c_int = 15;
const MEM_LEVEL: c_int = 9;
const STRATEGY: c_int = Z_FILTERED;

/// The header [`zalloc`] puts before each block: its size, for [`zfree`].
pub(crate) const HEADER: usize = size_of::<usize>();
/// The alignment of zlib's blocks, and the multiple their sizes round up to.
const ALIGN: usize = align_of::<usize>();

/// The most each of `deflateInit2`'s five blocks may be, in order, as zlib
/// asks for it (`items * size`, before [`HEADER`]), at Pillow's settings
/// (zlib 1.2.13's `deflate.c`, `deflateInit2_`): the state, then the window
/// (`w_size` x 2 bytes), `prev` (`w_size` x 2), `head` (`hash_size` x 2) and
/// the pending buffer (`lit_bufsize` x 4), with `w_size` 32 Ki, `hash_size`
/// `1 << (memLevel + 7)` and `lit_bufsize` `1 << (memLevel + 6)`. The
/// state's `sizeof(deflate_state)` is 5,952 bytes on 64-bit Linux and macOS
/// and less on 32-bit targets; its slot is a bound above it.
pub(crate) const STATE_SLOTS: [usize; 5] = [6 << 10, 64 << 10, 64 << 10, 128 << 10, 128 << 10];

#[cfg(test)]
thread_local! {
    /// The slots [`zalloc`] holds this thread's streams to in the unit
    /// tests: [`STATE_SLOTS`], unless a test narrows one
    /// (`src/memory_tests.rs`).
    pub(crate) static TEST_SLOTS: std::cell::Cell<[usize; 5]> =
        const { std::cell::Cell::new(STATE_SLOTS) };
}

/// The slots [`zalloc`] holds zlib to.
#[cfg(not(test))]
const fn slots() -> [usize; 5] {
    STATE_SLOTS
}

/// The slots [`zalloc`] holds zlib to: the thread's `TEST_SLOTS`.
#[cfg(test)]
fn slots() -> [usize; 5] {
    TEST_SLOTS
        .try_with(std::cell::Cell::get)
        .unwrap_or(STATE_SLOTS)
}

/// What the global allocator is asked for a block of `bytes`, as
/// [`zalloc`] asks: rounded up to [`ALIGN`], plus [`HEADER`].
pub(crate) const fn request(bytes: usize) -> usize {
    bytes
        .div_ceil(ALIGN)
        .saturating_mul(ALIGN)
        .saturating_add(HEADER)
}

/// zlib's `z_stream` (zlib.h), as `Z_PREFIX` leaves it.
#[repr(C)]
struct ZStream {
    next_in: *const u8,
    avail_in: c_uint,
    total_in: c_ulong,
    next_out: *mut u8,
    avail_out: c_uint,
    total_out: c_ulong,
    msg: *const c_char,
    state: *mut c_void,
    zalloc: AllocFunc,
    zfree: FreeFunc,
    opaque: *mut c_void,
    data_type: c_int,
    adler: c_ulong,
    reserved: c_ulong,
}

type AllocFunc = unsafe extern "C" fn(*mut c_void, c_uint, c_uint) -> *mut c_void;
type FreeFunc = unsafe extern "C" fn(*mut c_void, *mut c_void);

// The vendored zlib's functions, under `Z_PREFIX`'s names (build.rs).
unsafe extern "C" {
    fn z_deflateInit2_(
        strm: *mut ZStream,
        level: c_int,
        method: c_int,
        window_bits: c_int,
        mem_level: c_int,
        strategy: c_int,
        version: *const c_char,
        stream_size: c_int,
    ) -> c_int;
    fn z_deflate(strm: *mut ZStream, flush: c_int) -> c_int;
    fn z_deflateEnd(strm: *mut ZStream) -> c_int;
    fn z_deflateBound(strm: *mut ZStream, source_len: c_ulong) -> c_ulong;
    fn z_crc32(crc: c_ulong, buf: *const u8, len: c_uint) -> c_ulong;
    safe fn z_zlibVersion() -> *const c_char;
}

/// The version string of the zlib this crate compiled in: `"1.2.13"`.
#[must_use]
pub fn zlib_version() -> &'static str {
    let p = z_zlibVersion();
    if p.is_null() {
        return "";
    }
    // SAFETY: zlibVersion returns ZLIB_VERSION, a static NUL-terminated
    // string literal (zutil.c).
    let version = unsafe { CStr::from_ptr(p) };
    version.to_str().unwrap_or("")
}

/// zlib's CRC-32 of `data`, continuing `crc` (0 to start): a PNG chunk's.
#[must_use]
pub(crate) fn crc32(crc: u32, data: &[u8]) -> u32 {
    let mut crc = c_ulong::from(crc);
    // At most 1 GiB a call: crc32's length is a uInt.
    for part in data.chunks(1 << 30) {
        let len = c_uint::try_from(part.len()).unwrap_or(0);
        // SAFETY: `part` is a live slice of at least `len` bytes; crc32 only
        // reads them.
        crc = unsafe { z_crc32(crc, part.as_ptr(), len) };
    }
    // A CRC-32 is 32 bits wide whatever the width of uLong.
    u32::try_from(crc & 0xFFFF_FFFF).unwrap_or(0)
}

/// What [`zalloc`] holds zlib to: the slots of [`STATE_SLOTS`] still to
/// use, in order.
struct Allowance {
    used: usize,
    /// What zlib asked for, `items * size`, by slot.
    asked: [usize; STATE_SLOTS.len()],
    /// A block was refused for passing its slot (or a sixth asked for).
    over: bool,
}

/// One deflate stream at Pillow's settings. It lives pinned (zlib keeps a
/// pointer back to the stream and refuses a moved one: `deflateStateCheck`),
/// and ends with `deflateEnd` when dropped.
pub(crate) struct Deflater {
    stream: ZStream,
    allowance: Allowance,
    live: bool,
    _pinned: PhantomPinned,
}

impl Deflater {
    /// A stream not yet initialised; [`Self::init`] starts it once pinned.
    pub(crate) fn new() -> Self {
        Self {
            stream: ZStream {
                next_in: ptr::null(),
                avail_in: 0,
                total_in: 0,
                next_out: ptr::null_mut(),
                avail_out: 0,
                total_out: 0,
                msg: ptr::null(),
                state: ptr::null_mut(),
                zalloc,
                zfree,
                opaque: ptr::null_mut(),
                data_type: 0,
                adler: 0,
                reserved: 0,
            },
            allowance: Allowance {
                used: 0,
                asked: [0; STATE_SLOTS.len()],
                over: false,
            },
            live: false,
            _pinned: PhantomPinned,
        }
    }

    /// `deflateInit2` at Pillow's settings; zlib's return code.
    pub(crate) fn init(self: Pin<&mut Self>) -> c_int {
        // SAFETY: nothing is moved out of the pinned value; its fields are
        // only written in place.
        let this = unsafe { self.get_unchecked_mut() };
        if this.live {
            return Z_OK;
        }
        this.stream.opaque = (&raw mut this.allowance).cast();
        let size = c_int::try_from(size_of::<ZStream>()).unwrap_or(c_int::MAX);
        // SAFETY: the stream is pinned, so the pointer zlib keeps to it and
        // the `opaque` pointer to the allowance stay valid until drop, which
        // ends the stream; `zalloc` and `zfree` are set; the version is the
        // vendored zlib's own, NUL-terminated.
        let code = unsafe {
            z_deflateInit2_(
                &raw mut this.stream,
                LEVEL,
                Z_DEFLATED,
                WINDOW_BITS,
                MEM_LEVEL,
                STRATEGY,
                c"1.2.13".as_ptr(),
                size,
            )
        };
        this.live = code == Z_OK;
        code
    }

    /// Whether `zalloc` refused a block for passing its slot of
    /// [`STATE_SLOTS`]: the charged constant is wrong for this target.
    pub(crate) fn over_allowance(&self) -> bool {
        self.allowance.over
    }

    /// `deflateBound` of `len` bytes for this stream, or `None` when it does
    /// not fit a `usize`.
    pub(crate) fn bound(self: Pin<&mut Self>, len: u64) -> Option<usize> {
        // SAFETY: as in `init`.
        let this = unsafe { self.get_unchecked_mut() };
        let len = c_ulong::try_from(len).ok()?;
        // SAFETY: the stream is initialised (or zlib returns its
        // conservative bound for a stream it does not know); deflateBound
        // only reads it.
        let bound = unsafe { z_deflateBound(&raw mut this.stream, len) };
        // deflateBound's arithmetic wraps in a uLong; a bound below the
        // input is a wrapped one.
        (bound >= len)
            .then(|| usize::try_from(bound).ok())
            .flatten()
    }

    /// The bytes of input zlib has not read yet.
    pub(crate) fn avail_in(&self) -> c_uint {
        self.stream.avail_in
    }

    /// The room left in the output window of the last [`Self::deflate`].
    pub(crate) fn avail_out(&self) -> c_uint {
        self.stream.avail_out
    }

    /// Points zlib's input at `input`, which zlib reads during later calls
    /// to [`Self::deflate`] until [`Self::avail_in`] is 0.
    ///
    /// # Safety
    /// `input` must stay alive and unwritten until zlib has read it all
    /// ([`Self::avail_in`] 0) or another input replaces it.
    pub(crate) unsafe fn set_input(self: Pin<&mut Self>, input: &[u8]) {
        // SAFETY: as in `init`.
        let this = unsafe { self.get_unchecked_mut() };
        this.stream.next_in = input.as_ptr();
        this.stream.avail_in = c_uint::try_from(input.len()).unwrap_or(c_uint::MAX);
    }

    /// Points zlib's output at `output`, which zlib writes during later
    /// calls to [`Self::deflate`].
    ///
    /// # Safety
    /// `output` must stay alive and unread until the next call to this.
    pub(crate) unsafe fn set_output(self: Pin<&mut Self>, output: &mut [u8]) {
        // SAFETY: as in `init`.
        let this = unsafe { self.get_unchecked_mut() };
        this.stream.next_out = output.as_mut_ptr();
        this.stream.avail_out = c_uint::try_from(output.len()).unwrap_or(c_uint::MAX);
    }

    /// One call of `deflate(flush)`; zlib's return code, or
    /// `Z_STREAM_ERROR`, as zlib's for a stream it does not know, when the
    /// stream is not live: a bug, never the allocator's refusal.
    pub(crate) fn deflate(self: Pin<&mut Self>, flush: c_int) -> c_int {
        // SAFETY: as in `init`.
        let this = unsafe { self.get_unchecked_mut() };
        if !this.live {
            return Z_STREAM_ERROR;
        }
        // SAFETY: the stream is initialised and pinned; its input and output
        // windows were set by `set_input` and `set_output`, whose callers
        // keep them valid (their safety contracts).
        unsafe { z_deflate(&raw mut this.stream, flush) }
    }
}

impl Drop for Deflater {
    fn drop(&mut self) {
        if self.live {
            // SAFETY: the stream was initialised and has not been ended;
            // deflateEnd frees its blocks through `zfree`. Its return code
            // (`Z_DATA_ERROR` for a stream ended early) frees all the same.
            unsafe { z_deflateEnd(&raw mut self.stream) };
            self.live = false;
        }
    }
}

/// zlib's `zalloc`: a block of `items * size` bytes from Rust's global
/// allocator, behind a [`HEADER`] that holds its size, if it fits the next
/// slot of [`STATE_SLOTS`]; null otherwise, which zlib reports as
/// `Z_MEM_ERROR`.
unsafe extern "C" fn zalloc(opaque: *mut c_void, items: c_uint, size: c_uint) -> *mut c_void {
    // SAFETY: `opaque` is the allowance of the pinned `Deflater` whose
    // stream zlib is serving (`Deflater::init`), alive until its drop.
    let Some(allowance) = (unsafe { opaque.cast::<Allowance>().as_mut() }) else {
        return ptr::null_mut();
    };
    let bytes = usize::try_from(items)
        .ok()
        .zip(usize::try_from(size).ok())
        .and_then(|(items, size)| items.checked_mul(size));
    let index = allowance.used;
    allowance.used = allowance.used.saturating_add(1);
    let (Some(bytes), Some(slot)) = (bytes, slots().get(index).copied()) else {
        allowance.over = true;
        return ptr::null_mut();
    };
    if let Some(asked) = allowance.asked.get_mut(index) {
        *asked = bytes;
    }
    if bytes > slot {
        allowance.over = true;
        return ptr::null_mut();
    }
    let Ok(layout) = Layout::from_size_align(request(bytes), ALIGN) else {
        return ptr::null_mut();
    };
    // SAFETY: the layout's size is at least HEADER, never zero.
    let block = unsafe { alloc::alloc(layout) };
    if block.is_null() {
        return ptr::null_mut();
    }
    // SAFETY: the block holds at least HEADER bytes (aligned for a usize,
    // being ALIGN-aligned, though the write does not need it); the bytes
    // after the header are the caller's.
    unsafe {
        block.cast::<usize>().write_unaligned(layout.size());
        block.add(HEADER).cast()
    }
}

/// zlib's `zfree`: gives back a block [`zalloc`] made.
unsafe extern "C" fn zfree(_opaque: *mut c_void, address: *mut c_void) {
    if address.is_null() {
        return;
    }
    // SAFETY: `address` is a block `zalloc` returned, HEADER bytes past the
    // start of an allocation whose first usize holds its size, made with
    // ALIGN.
    unsafe {
        let block = address.cast::<u8>().sub(HEADER);
        let size = block.cast::<usize>().read_unaligned();
        alloc::dealloc(block, Layout::from_size_align_unchecked(size, ALIGN));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slots_are_what_deflate_init2_asks_for() {
        let mut d = std::pin::pin!(Deflater::new());
        assert_eq!(d.as_mut().init(), Z_OK);
        assert!(!d.over_allowance());
        assert_eq!(d.allowance.used, STATE_SLOTS.len());
        // The four buffers are their slots exactly; the state is below its
        // bound, and on 64-bit Unix it is zlib 1.2.13's 5,952 bytes.
        assert_eq!(d.allowance.asked[1..], STATE_SLOTS[1..]);
        assert!(d.allowance.asked[0] <= STATE_SLOTS[0]);
        #[cfg(all(unix, target_pointer_width = "64"))]
        assert_eq!(d.allowance.asked[0], 5952);
    }

    #[test]
    fn a_block_past_its_slot_is_refused() {
        let mut allowance = Allowance {
            used: 0,
            asked: [0; STATE_SLOTS.len()],
            over: false,
        };
        let opaque = (&raw mut allowance).cast();
        let past = c_uint::try_from(STATE_SLOTS[0] + 1).unwrap();
        // SAFETY: test-only: `opaque` is a live allowance nothing else reads.
        let block = unsafe { zalloc(opaque, 1, past) };
        assert!(block.is_null());
        assert!(allowance.over);
        assert_eq!(allowance.asked[0], STATE_SLOTS[0] + 1);
        assert_eq!(allowance.used, 1);
        // A block of its slot exactly is made.
        let mut allowance = Allowance {
            used: 0,
            asked: [0; STATE_SLOTS.len()],
            over: false,
        };
        let opaque = (&raw mut allowance).cast();
        let at = c_uint::try_from(STATE_SLOTS[0]).unwrap();
        // SAFETY: as above; the block is zalloc's, freed once.
        unsafe {
            let block = zalloc(opaque, 1, at);
            assert!(!block.is_null());
            zfree(opaque, block);
        }
        assert!(!allowance.over);
    }

    #[test]
    fn a_block_with_no_slot_left_is_refused() {
        // One slot used up before deflateInit2 starts: its fifth block finds
        // no slot left, so it fails with Z_MEM_ERROR, the allowance's
        // refusal (src/memory_tests.rs counts that deflateInit2 frees the
        // blocks it had).
        let mut d = std::pin::pin!(Deflater::new());
        // SAFETY: test-only: the stream is not initialised yet, so nothing
        // has read the allowance.
        unsafe { d.as_mut().get_unchecked_mut() }.allowance.used = 1;
        let code = d.as_mut().init();
        assert_eq!(code, Z_MEM_ERROR);
        assert!(d.over_allowance());
        // The stream is not live, so a deflate is a bug, not a refusal.
        assert_eq!(d.as_mut().deflate(Z_FINISH), Z_STREAM_ERROR);
    }

    #[test]
    fn a_stream_not_live_is_a_stream_error() {
        let mut d = std::pin::pin!(Deflater::new());
        assert_eq!(d.as_mut().deflate(Z_NO_FLUSH), Z_STREAM_ERROR);
    }

    #[test]
    fn crc32_is_the_png_crc() {
        // The IEND chunk's CRC, which every PNG ends with.
        assert_eq!(crc32(0, b"IEND"), 0xAE42_6082);
    }
}
