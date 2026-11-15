//! The already-linked liblzma, with stack filters and a fallible allocator.
//! Encoder settings are passed from the bundle writer's fixed Unity format:
//! raw LZMA1, dictionary 8 MiB, lc/lp/pb 3/0/2, normal, BT4, nice length 123.

use std::alloc::{self, Layout};
use std::ffi::c_void;
use std::ptr;

use liblzma_sys as sys;

use crate::Error;

// Preserve malloc's fundamental alignment on the supported 64-bit targets,
// including after the private size header, rather than just pointer alignment.
const ALIGN: usize = align_of::<u128>();
const HEADER: usize = ALIGN;

/// A native encoder failure or an error returned by the caller's output sink.
#[derive(Debug)]
pub enum LzmaError<E> {
    /// The native encoder refused this operation.
    Native(Error),
    /// The caller refused output, with its original error preserved.
    Output(E),
}

struct Allowance {
    limit: u64,
    used: u64,
    refused: Option<Error>,
}

fn layout(bytes: usize) -> Option<Layout> {
    Layout::from_size_align(bytes.checked_add(HEADER)?, ALIGN).ok()
}

// allocate's Layout aligns its header for usize; the C pointer hides that.
#[allow(clippy::cast_ptr_alignment)]
extern "C" fn allocate(opaque: *mut c_void, count: usize, size: usize) -> *mut c_void {
    // SAFETY: opaque is this encoder's live Allowance; the single-threaded
    // raw encoder invokes its allocator synchronously.
    let state = unsafe { &mut *opaque.cast::<Allowance>() };
    let Some(request) = count.checked_mul(size).and_then(layout) else {
        state.refused = Some(Error::Allowance);
        return ptr::null_mut();
    };
    let cost = heap_cost(request.size() as u64);
    let Some(next) = state.used.checked_add(cost).filter(|n| *n <= state.limit) else {
        state.refused = Some(Error::Allowance);
        return ptr::null_mut();
    };
    // SAFETY: request is a valid nonzero layout including the size header.
    // A null is returned to liblzma, never passed to handle_alloc_error.
    let block = unsafe { alloc::alloc(request) };
    if block.is_null() {
        state.refused = Some(Error::Allocator(request.size() as u64));
        return ptr::null_mut();
    }
    state.used = next;
    // SAFETY: block is aligned for usize with room for the header and data.
    unsafe {
        block.cast::<usize>().write(request.size());
        block.add(HEADER).cast()
    }
}

// Every pointer here came from allocate's usize-aligned Layout.
#[allow(clippy::cast_ptr_alignment)]
extern "C" fn free(opaque: *mut c_void, block: *mut c_void) {
    if block.is_null() {
        return;
    }
    // SAFETY: liblzma returns only live blocks from allocate, exactly once;
    // the opaque state and size header remain live through lzma_end.
    unsafe {
        let state = &mut *opaque.cast::<Allowance>();
        let base = block.cast::<u8>().sub(HEADER);
        let size = base.cast::<usize>().read();
        let request = Layout::from_size_align_unchecked(size, ALIGN);
        state.used -= heap_cost(size as u64);
        alloc::dealloc(base, request);
    }
}

// Match core's conservative glibc chunk charge, including header/rounding.
const fn heap_cost(bytes: u64) -> u64 {
    let chunk = bytes.saturating_add(8 + 15) & !15;
    if chunk < 32 {
        32
    } else if chunk < 128 << 10 {
        chunk
    } else {
        chunk.saturating_add(8 + 4095) & !4095
    }
}

struct Encoder(sys::lzma_stream);

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: zero-initialization is LZMA_STREAM_INIT, and liblzma permits
        // lzma_end after failed initialization. The allocator remains live.
        unsafe { sys::lzma_end(&raw mut self.0) };
    }
}

fn options() -> Result<sys::lzma_options_lzma, Error> {
    // SAFETY: all-zero integer/enum-alias/pointer fields are valid; preset
    // initialization writes the complete options before they are used.
    let mut options: sys::lzma_options_lzma = unsafe { std::mem::zeroed() };
    // SAFETY: writable options of the ABI linked by this same sys crate.
    if unsafe { sys::lzma_lzma_preset(&raw mut options, 6) } != 0 {
        return Err(Error::Compress);
    }
    options.dict_size = 0x0080_0000;
    options.lc = 3;
    options.lp = 0;
    options.pb = 2;
    options.mode = sys::LZMA_MODE_NORMAL;
    options.mf = sys::LZMA_MF_BT4;
    options.nice_len = 123;
    Ok(options)
}

fn filters(options: &mut sys::lzma_options_lzma) -> [sys::lzma_filter; 2] {
    [
        sys::lzma_filter {
            id: sys::LZMA_FILTER_LZMA1,
            options: ptr::from_mut(options).cast(),
        },
        sys::lzma_filter {
            id: sys::LZMA_VLI_UNKNOWN,
            options: ptr::null_mut(),
        },
    ]
}

/// Complete encoder allowance, including allocator size headers and chunk
/// rounding. The raw encoder reports its payload requirements; 64 KiB more
/// covers allocation overhead, enforced on each live allocation by allocate.
pub fn lzma_encoder_memory() -> Result<u64, Error> {
    let mut options = options()?;
    let filters = filters(&mut options);
    // SAFETY: the terminated filter array and its options are live.
    let memory = unsafe { sys::lzma_raw_encoder_memusage(filters.as_ptr()) };
    memory.checked_add(64 << 10).ok_or(Error::Range)
}

/// Encode raw LZMA1 through a fallible caller sink. The caller must first
/// charge `allowance`, normally `lzma_encoder_memory()`. All native memory
/// goes through Rust's fallible allocator and is held to that allowance.
/// The fixed output window lives on the stack and is borrowed only during
/// each synchronous sink call; no caller pointer survives this function.
pub fn lzma_compress<E>(
    data: &[u8],
    allowance: u64,
    mut emit: impl FnMut(&[u8]) -> Result<(), E>,
) -> Result<(), LzmaError<E>> {
    let mut options = options().map_err(LzmaError::Native)?;
    let filters = filters(&mut options);
    let mut state = Allowance {
        limit: allowance,
        used: 0,
        refused: None,
    };
    let allocator = sys::lzma_allocator {
        alloc: Some(allocate),
        free: Some(free),
        opaque: ptr::from_mut(&mut state).cast(),
    };
    // SAFETY: all-zero is liblzma's LZMA_STREAM_INIT value.
    let mut encoder = Encoder(unsafe { std::mem::zeroed() });
    encoder.0.allocator = &raw const allocator;
    // SAFETY: stream, filters, options, allocator and state remain live until
    // lzma_end, which runs before any borrowed stack storage is dropped.
    let status = unsafe { sys::lzma_raw_encoder(&raw mut encoder.0, filters.as_ptr()) };
    if status != sys::LZMA_OK {
        return Err(LzmaError::Native(state.refused.unwrap_or(Error::Compress)));
    }
    encoder.0.next_in = data.as_ptr();
    encoder.0.avail_in = data.len();
    let mut output = [0u8; 16 << 10];
    loop {
        encoder.0.next_out = output.as_mut_ptr();
        encoder.0.avail_out = output.len();
        let before = encoder.0.total_in;
        // SAFETY: input and output fields name live nonoverlapping slices.
        // FINISH consumes all remaining input then writes the end marker.
        let status = unsafe { sys::lzma_code(&raw mut encoder.0, sys::LZMA_FINISH) };
        let made = output.len() - encoder.0.avail_out;
        emit(&output[..made]).map_err(LzmaError::Output)?;
        if status == sys::LZMA_STREAM_END {
            break;
        }
        if status != sys::LZMA_OK || (made == 0 && encoder.0.total_in == before) {
            return Err(LzmaError::Native(state.refused.unwrap_or(Error::Compress)));
        }
    }
    drop(encoder);
    debug_assert_eq!(state.used, 0);
    Ok(())
}
