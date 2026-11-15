// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/WebFile.py. See THIRD_PARTY_LICENSES.
//! Metered WebFile stream inflation (`files/WebFile.py:20-37`).
use crate::budget::DecodeBuf;
use crate::error::{Error, LimitError, LimitKind};
use crate::limits::{ChargeError, Meter, MeteredVec, heap_cost};
use crate::options::FileContext;
use brotli_decompressor::{
    Allocator, BrotliDecompressStream, BrotliResult, BrotliState, HuffmanCode, SliceWrapper,
    SliceWrapperMut,
};
use bytes::Bytes;
use std::cell::RefCell;
use std::sync::Arc;

pub(super) fn bad(detail: &'static str) -> Error {
    Error::Malformed {
        what: "WebFile",
        detail: detail.into(),
    }
}

/// The allocator's empty return signals failure to Brotli. Fixed allocations
/// that upstream does not check are made before entering the decoder. Once
/// any allocation fails, all later allocators return empty too: in particular
/// a failed u32 tree index makes its checked Huffman code allocation fail.
#[derive(Default)]
struct Memory<T>(Option<MeteredVec<T>>);
impl<T> SliceWrapper<T> for Memory<T> {
    fn slice(&self) -> &[T] {
        self.0.as_ref().map_or(&[], MeteredVec::as_slice)
    }
}
impl<T> SliceWrapperMut<T> for Memory<T> {
    fn slice_mut(&mut self) -> &mut [T] {
        self.0.as_mut().map_or(&mut [], MeteredVec::as_mut_slice)
    }
}
struct Alloc<'a, T> {
    meter: &'a Arc<Meter>,
    error: &'a RefCell<Option<Error>>,
    fixed: [Option<Memory<T>>; 3],
    work: &'a RefCell<Work>,
}
impl<'a, T> Alloc<'a, T> {
    fn new(
        meter: &'a Arc<Meter>,
        error: &'a RefCell<Option<Error>>,
        work: &'a RefCell<Work>,
    ) -> Self {
        Self {
            meter,
            error,
            fixed: [None, None, None],
            work,
        }
    }
}
fn memory<T: Default>(meter: &Arc<Meter>, n: usize) -> Result<Memory<T>, ChargeError> {
    let mut v = MeteredVec::with_capacity(meter, n)?;
    for _ in 0..n {
        v.push(T::default())?;
    }
    Ok(Memory(Some(v)))
}
impl<T: Default> Allocator<T> for Alloc<'_, T> {
    type AllocatedMemory = Memory<T>;
    fn alloc_cell(&mut self, n: usize) -> Memory<T> {
        if self.error.borrow().is_some() {
            return Memory(None);
        }
        for slot in &mut self.fixed {
            if slot.as_ref().is_some_and(|v| v.slice().len() == n) {
                return slot.take().unwrap_or_default();
            }
        }
        // All input-sized u8 allocations, including the ring, are capped
        // before allocation. The other allocators also charge every table
        // through MeteredVec before allocating it.
        let bytes = (n as u64).saturating_mul(size_of::<T>() as u64);
        if let Err(e) = self
            .work
            .borrow_mut()
            .allocation(bytes, size_of::<T>() == 1)
        {
            *self.error.borrow_mut() = Some(e);
            return Memory(None);
        }
        match memory(self.meter, n) {
            Ok(v) => v,
            Err(e) => {
                // Nothing was allocated or initialized. As with a refused
                // DecodeBuf growth, a pool refusal uses no work allowance.
                self.work.borrow_mut().refund_allocation(bytes);
                *self.error.borrow_mut() = Some(e.into());
                Memory(None)
            }
        }
    }
    fn free_cell(&mut self, _data: Memory<T>) {}
}

// brotli-decompressor 5.0.0, decode.rs:1815-1857: a ring allocation is its
// power-of-two window plus 42 write-ahead bytes and a 24-byte dictionary
// word. These are constant slack, never a stream-controlled allowance.
const RING_SLACK: u64 = 42 + 24;

/// A decoder call can fill its ring before returning even one output byte.
/// Reserve work for everything it can produce *before* calling it, then
/// retain the change in the ring's absolute position, including bytes not
/// yet flushed. Initializing input-sized allocations prepays their bytes
/// as credit for output (as `DecodeBuf::prepay` does). The initial
/// ring's size is learned through the allocator, which extends the same
/// reservation before the ring can exist. Unused production reservations
/// return at each call boundary; copying counted output adds no more work.
struct Work {
    meter: Arc<Meter>,
    cap: u64,
    reserved: u64,
    credit: u64,
}
impl Work {
    fn reserve(&mut self, n: u64) -> Result<(), Error> {
        let n = n.saturating_sub(self.credit);
        self.meter.count_decode(n)?;
        self.reserved += n;
        Ok(())
    }

    fn allocation(&mut self, n: u64, byte_cells: bool) -> Result<(), Error> {
        if byte_cells && n > self.cap.saturating_add(RING_SLACK) {
            return Err(Error::Limit(LimitError::refused(
                LimitKind::Inflate,
                n.saturating_sub(RING_SLACK),
                self.cap,
            )));
        }
        // Pay before zero-initializing any input-sized table or window.
        // The ring's credit also covers its first fill in this very call.
        self.meter.count_decode(n)?;
        self.credit += n;
        Ok(())
    }

    fn refund_allocation(&mut self, n: u64) {
        self.credit -= n;
        self.meter.uncount_decode(n);
    }

    fn finish(&mut self, produced: u64) -> Result<(), Error> {
        let credit = produced.min(self.credit);
        self.credit -= credit;
        let produced = produced - credit;
        if produced > self.reserved {
            // An upstream change must never silently leave output unpaid.
            self.meter.add_work(produced - self.reserved);
            self.reserved = 0;
            return Err(bad("brotli exceeded reserved ring work"));
        }
        self.reserved -= produced;
        self.meter
            .uncount_decode(std::mem::take(&mut self.reserved));
        Ok(())
    }

    fn failed(&mut self) {
        // decode.rs:2479 can return a fatal distance error without saving
        // its local position. Keep the whole prepaid call in that case:
        // every byte it could have produced stays counted, still bounded.
        self.reserved = 0;
        self.credit = 0;
    }
}
impl Drop for Work {
    fn drop(&mut self) {
        self.meter.uncount_decode(self.reserved);
    }
}

pub(super) fn brotli(data: &Bytes, meter: &Arc<Meter>, ctx: &FileContext) -> Result<Bytes, Error> {
    let error = RefCell::new(None);
    let work = RefCell::new(Work {
        meter: meter.clone(),
        cap: ctx.limits.max_inflate,
        reserved: 0,
        credit: 0,
    });
    let mut hc = Alloc::<HuffmanCode>::new(meter, &error, &work);
    // brotli-decompressor 5.0.0: state.rs:388-396 and decode.rs:2840-2850.
    // BROTLI_HUFFMAN_MAX_TABLE_SIZE=1080. These constant tables are not
    // sized from the input. In particular block_len_trees is unchecked.
    hc.fixed = [
        Some(memory(meter, 1080)?),
        Some(memory(meter, 3240)?),
        Some(memory(meter, 3240)?),
    ];
    let mut state = BrotliState::new_strict(
        Alloc::<u8>::new(meter, &error, &work),
        Alloc::<u32>::new(meter, &error, &work),
        hc,
    );
    let mut out = DecodeBuf::new(meter, 0, ctx.limits.max_inflate, LimitKind::Inflate)?;
    let mut input_at = 0;
    let mut available = 0;
    let mut total = 0;
    let mut scratch = [0; 8192];
    let mut window = 1;
    let mut produced = 0u64;
    loop {
        if available == 0 {
            available = data.len().saturating_sub(input_at).min(8192);
            // Pay for bytes offered to the decoder before it can read or
            // buffer them, as ZIP's input windows do.
            meter.count_decode(available as u64)?;
        }
        // WriteRingBuffer stops a full ring when output is exhausted. A
        // call can therefore produce at most the room it flushes plus one
        // ring (including write-ahead slack), less its already-buffered
        // bytes. This also covers multiple wraps of a small window. A
        // finished stream only drains bytes whose production already paid.
        if !state.BrotliStateIsStreamEnd() {
            let buffered = produced.saturating_sub(state.partial_pos_out as u64);
            let ahead =
                (state.ringbuffer.slice().len() as u64 + window as u64).saturating_sub(buffered);
            work.borrow_mut().reserve(ahead)?;
        }
        let mut room = window;
        let mut wrote = 0;
        let result = BrotliDecompressStream(
            &mut available,
            &mut input_at,
            data,
            &mut room,
            &mut wrote,
            &mut scratch,
            &mut total,
            &mut state,
        );
        let position = (state.rb_roundtrips as u64)
            .saturating_mul(u64::try_from(state.ringbuffer_size).unwrap_or(0))
            .saturating_add(u64::try_from(state.pos).unwrap_or(0));
        if matches!(result, BrotliResult::ResultFailure) && error.borrow().is_none() {
            work.borrow_mut().failed();
        } else {
            work.borrow_mut()
                .finish(position.saturating_sub(produced))?;
        }
        produced = position;
        if let Some(e) = error.borrow_mut().take() {
            return Err(e);
        }
        if produced > ctx.limits.max_inflate {
            return Err(Error::Limit(LimitError::refused(
                LimitKind::Inflate,
                produced,
                ctx.limits.max_inflate,
            )));
        }
        out.extend_from_slice(scratch.get(..wrote).ok_or_else(|| bad("brotli output"))?)?;
        window = (window * 2).min(scratch.len());
        match result {
            BrotliResult::ResultSuccess if available == 0 && input_at == data.len() => {
                return Ok(out.into_bytes()?);
            }
            BrotliResult::NeedsMoreOutput => {}
            BrotliResult::NeedsMoreInput if input_at < data.len() => {}
            _ => return Err(bad("invalid or incomplete brotli stream")),
        }
    }
}

pub(super) fn gzip(data: &Bytes, meter: &Arc<Meter>, ctx: &FileContext) -> Result<Bytes, Error> {
    // Charge the detector's existing inflater state and scratch before it
    // allocates. Output is retained only by DecodeBuf, from an empty buffer.
    let _decoder = meter.charge(heap_cost(128 * 1024))?;
    let mut out = DecodeBuf::new(meter, 0, ctx.limits.max_inflate, LimitKind::Inflate)?;
    crate::detect::gzip_inflate_to::<Error>(
        data,
        ctx.limits.max_inflate,
        Some(meter),
        &mut [0; 8192],
        |chunk| out.extend_from_slice(chunk).map_err(Error::from),
    )
    .map_err(|e| match e {
        Error::Detect(
            crate::detect::DetectError::InflateLimit(l) | crate::detect::DetectError::Work(l),
        ) => Error::Limit(l),
        e => e,
    })?;
    Ok(out.into_bytes()?)
}
