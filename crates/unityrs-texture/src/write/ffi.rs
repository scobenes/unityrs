//! The only unsafe encoder module. Both bridges are synchronous and keep no
//! pointers after returning. astcenc uses one thread; its allocator callbacks
//! carry a per-call meter, so concurrent encodes never share an allowance.
#![allow(unsafe_code)]
// Layout::extend guarantees Header alignment at the allocation start.
#![allow(clippy::cast_ptr_alignment)]
use super::WriteError;
use std::alloc::{self, Layout};
use std::ffi::{c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::Arc;
use unityrs_core::budget::AllocError;
use unityrs_core::limits::{Charge, Meter, heap_cost};

unsafe extern "C" {
    fn unityrs_etcpak_encode(
        format: u32,
        rgba: *const u8,
        rgba_len: usize,
        width: u32,
        height: u32,
        output: *mut u8,
        output_len: usize,
    ) -> c_int;
    fn unityrs_astc_encode(
        rgba: *const u8,
        rgba_len: usize,
        width: u32,
        height: u32,
        block_x: u32,
        block_y: u32,
        output: *mut u8,
        output_len: usize,
        user: *mut c_void,
        allocate: unsafe extern "C" fn(*mut c_void, usize, usize) -> *mut c_void,
        free: unsafe extern "C" fn(*mut c_void, *mut c_void, usize, usize),
    ) -> c_int;
}

struct Allocator<'a> {
    meter: &'a Arc<Meter>,
    error: Option<WriteError>,
    live: usize,
}
struct Header {
    charge: Charge,
}

fn layout(size: usize, alignment: usize) -> Option<(Layout, usize)> {
    let payload = Layout::from_size_align(size.max(1), alignment).ok()?;
    let (layout, offset) = Layout::new::<Header>().extend(payload).ok()?;
    Some((layout.pad_to_align(), offset))
}

// Each callback catches panics before returning to C++. Allocation refusal
// stays in the context independently of astcenc's optional fallback paths.
unsafe extern "C" fn allocate(user: *mut c_void, size: usize, align: usize) -> *mut c_void {
    let result = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the bridge calls on this thread only while encode's context
        // is alive, with no overlapping callback or Rust reference to it.
        let state = unsafe { &mut *user.cast::<Allocator<'_>>() };
        if state.error.is_some() {
            return ptr::null_mut();
        }
        let Some((layout, offset)) = layout(size, align) else {
            state.error = Some(WriteError::Invalid("encoder allocation layout"));
            return ptr::null_mut();
        };
        let charge = match state.meter.charge(heap_cost(layout.size() as u64)) {
            Ok(charge) => charge,
            Err(error) => {
                state.error = Some(error.into());
                return ptr::null_mut();
            }
        };
        // SAFETY: layout has a valid alignment and nonzero size. This is the
        // fallible allocator, so null is returned rather than aborting.
        let memory = unsafe { alloc::alloc(layout) };
        if memory.is_null() {
            state.error = Some(WriteError::Charge(
                AllocError {
                    requested: layout.size() as u64,
                    what: state.meter.reservation(),
                }
                .into(),
            ));
            return ptr::null_mut();
        }
        state.live += 1;
        // SAFETY: Header is aligned at the allocation start and has room;
        // extend aligned the payload at offset for the requested alignment.
        unsafe {
            memory.cast::<Header>().write(Header { charge });
            memory.add(offset).cast()
        }
    }));
    result.unwrap_or(ptr::null_mut())
}

unsafe extern "C" fn free(user: *mut c_void, memory: *mut c_void, size: usize, align: usize) {
    if memory.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some((layout, offset)) = layout(size, align) else {
            return;
        };
        // SAFETY: the bridge frees only live callback allocations, once,
        // with the original size/alignment, including exception cleanup.
        let (header, start) = unsafe {
            let start = memory.cast::<u8>().sub(offset);
            (start.cast::<Header>().read(), start)
        };
        // SAFETY: start and layout are the ones passed to alloc above.
        unsafe { alloc::dealloc(start, layout) };
        // Return the charge only after the allocation has been freed.
        drop(header.charge);
        // SAFETY: same synchronous callback context as allocate.
        let state = unsafe { &mut *user.cast::<Allocator<'_>>() };
        state.live = state.live.saturating_sub(1);
    }));
}

pub(super) fn encode(
    rgba: &[u8],
    width: u32,
    height: u32,
    format: i32,
    block: (u32, u32),
    output: &mut [u8],
    meter: &Arc<Meter>,
) -> Result<(), WriteError> {
    let mut state = Allocator {
        meter,
        error: None,
        live: 0,
    };
    // SAFETY: encoders::encode and astc_context checked nonzero bounded
    // dimensions, legal blocks, exact RGBA length and exact output size.
    // astcenc handles partial edge blocks; etcpak receives whole blocks.
    // These disjoint borrowed buffers and the callback context stay alive
    // throughout this synchronous, single-threaded call. The C++ bridges
    // validate the lengths again and catch exceptions before returning.
    let code = unsafe {
        if (48..=59).contains(&format) {
            unityrs_astc_encode(
                rgba.as_ptr(),
                rgba.len(),
                width,
                height,
                block.0,
                block.1,
                output.as_mut_ptr(),
                output.len(),
                (&raw mut state).cast(),
                allocate,
                free,
            )
        } else {
            unityrs_etcpak_encode(
                u32::try_from(if format == 60 { 34 } else { format })
                    .map_err(|_| WriteError::Invalid("encoder format"))?,
                rgba.as_ptr(),
                rgba.len(),
                width,
                height,
                output.as_mut_ptr(),
                output.len(),
            )
        }
    };
    if let Some(error) = state.error {
        return Err(error);
    }
    if code == 0 && state.live == 0 {
        Ok(())
    } else {
        Err(WriteError::Encoder(code))
    }
}
