//! The only unsafe WebP module: C boundary and fallible global allocator.
//! No libwebp worker threads are requested. Each encode has a thread-local
//! allowance; every allocation includes a size header and 48 bytes of heap
//! overhead in its charge. Out-of-allowance returns NULL, never an abort.
#![allow(unsafe_code)]
// Allocation layouts guarantee 16-byte alignment of both size headers.
#![allow(clippy::cast_ptr_alignment)]
use crate::{WebpError, WebpOptions};
use std::alloc::{self, Layout};
use std::cell::Cell;
use std::ffi::{c_int, c_void};
use std::ptr;

#[derive(Clone, Copy, Default)]
struct State {
    limit: usize,
    live: usize,
    over: bool,
    memory: bool,
}
thread_local! { static STATE: Cell<State> = const { Cell::new(State { limit: 0, live: 0, over: false, memory: false }) }; }
const ALIGN: usize = 16;
const HEADER: usize = 16;
unsafe extern "C" {
    fn unityrs_webp_encode(
        rgb: *const u8,
        width: c_int,
        height: c_int,
        alpha: c_int,
        quality: f32,
        method: c_int,
        exact: c_int,
        lossless: c_int,
        output: *mut u8,
        capacity: usize,
        size: *mut usize,
    ) -> c_int;
}

#[allow(clippy::too_many_arguments)] // Mirrors the synchronous C encoder boundary.
pub(crate) fn encode<R>(
    pixels: &[u8],
    width: u32,
    height: u32,
    alpha: bool,
    options: WebpOptions,
    lossless: bool,
    output: &mut [u8],
    limit: usize,
) -> Result<usize, WebpError<R>> {
    // The safe wrapper has checked dimensions and exact input length. This
    // function is crate-private and called only from that wrapper. The C shim
    // reads packed input as bytes; it imposes no alignment on a Rust slice.
    STATE.with(|s| {
        s.set(State {
            limit,
            ..State::default()
        });
    });
    let mut size = 0;
    // SAFETY: buffers are live for the synchronous call, disjoint, and have
    // the exact lengths required by the checked dimensions and channel mode.
    let code = unsafe {
        unityrs_webp_encode(
            pixels.as_ptr(),
            width.cast_signed(),
            height.cast_signed(),
            i32::from(alpha),
            options.quality,
            options.method.into(),
            i32::from(options.exact),
            i32::from(lossless),
            output.as_mut_ptr(),
            output.len(),
            &raw mut size,
        )
    };
    let state = STATE.with(|s| s.replace(State::default()));
    if state.over {
        return Err(WebpError::Allowance);
    }
    if state.memory {
        return Err(WebpError::Memory);
    }
    match code {
        0 if size > 0 && size <= output.len() && state.live == 0 => Ok(size),
        1 | 2 => Err(WebpError::Memory),
        _ => Err(WebpError::Encoder(code)),
    }
}

/// SAFETY: C calls only with a size, never unwinds. The returned block is
/// 16-byte aligned (libwebp aligns SIMD buffers itself, even in generic C).
#[unsafe(no_mangle)]
unsafe extern "C" fn unityrs_webp_malloc(size: usize) -> *mut c_void {
    allocate(size, false)
}
#[unsafe(no_mangle)]
unsafe extern "C" fn unityrs_webp_calloc(count: usize, size: usize) -> *mut c_void {
    match count.checked_mul(size) {
        Some(n) => allocate(n, true),
        None => refused(),
    }
}
// Some libwebp OOM paths leave error_code at OK. Preserve the refusal
// independently so even an optional fallback can never publish altered bytes.
fn refused() -> *mut c_void {
    let _ = STATE.try_with(|s| {
        let mut state = s.get();
        state.memory = true;
        s.set(state);
    });
    ptr::null_mut()
}
fn allocate(size: usize, zero: bool) -> *mut c_void {
    let Some(total) = size.max(1).checked_add(HEADER) else {
        return refused();
    };
    let Ok(layout) = Layout::from_size_align(total, ALIGN) else {
        return refused();
    };
    let Some(cost) = total.checked_add(48) else {
        return refused();
    };
    let allowed = STATE
        .try_with(|s| {
            let mut state = s.get();
            if cost > state.limit.saturating_sub(state.live) {
                state.over = true;
                s.set(state);
                false
            } else {
                state.live += cost;
                s.set(state);
                true
            }
        })
        .unwrap_or(false);
    if !allowed {
        return ptr::null_mut();
    }
    // SAFETY: layout has nonzero size and valid alignment. Null is handled.
    let p = unsafe {
        if zero {
            alloc::alloc_zeroed(layout)
        } else {
            alloc::alloc(layout)
        }
    };
    if p.is_null() {
        let _ = STATE.try_with(|s| {
            let mut state = s.get();
            state.live -= cost;
            state.memory = true;
            s.set(state);
        });
        return ptr::null_mut();
    }
    // SAFETY: allocation has room for the aligned size header and payload.
    unsafe {
        p.cast::<usize>().write(total);
        p.add(HEADER).cast()
    }
}
#[unsafe(no_mangle)]
unsafe extern "C" fn unityrs_webp_free(p: *mut c_void) {
    if p.is_null() {
        return;
    }
    // SAFETY: C frees only pointers returned by our allocator, exactly once;
    // the size header and allocation layout are those installed above.
    unsafe {
        let start = p.cast::<u8>().sub(HEADER);
        let total = start.cast::<usize>().read();
        if let Ok(layout) = Layout::from_size_align(total, ALIGN) {
            alloc::dealloc(start, layout);
            let _ = STATE.try_with(|s| {
                let mut state = s.get();
                state.live = state.live.saturating_sub(total + 48);
                s.set(state);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_exhausted_workspace_is_a_typed_refusal() {
        let result = encode::<()>(
            &[17, 33, 91],
            1,
            1,
            false,
            WebpOptions::default(),
            false,
            &mut [0; 256],
            0,
        );
        assert!(matches!(result, Err(WebpError::Allowance)));
        // A failed call leaves no allocator state behind on the thread.
        assert_eq!(STATE.with(|s| s.get().live), 0);
    }
}
