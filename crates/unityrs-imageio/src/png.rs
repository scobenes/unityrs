// Ported from Pillow 9.5.0 (MIT-CMU): src/PIL/PngImagePlugin.py's _save, putchunk and _idat, src/PIL/ImageFile.py's _save and _encode_tile, and src/libImaging/ZipEncode.c's ImagingZipEncode. See THIRD_PARTY_LICENSES.
//! A PNG file byte-identical to Pillow 9.5.0's `Image.save(fp, "PNG")` at
//! its defaults, for an image with no `info` (`docs/plans/extract.md` E19).
//!
//! What Pillow writes (`PngImagePlugin.py`'s `_save`): the signature; IHDR
//! (the size, bit depth 8, the mode's colour type, no interlace); no other
//! chunk before the image data, since the image carries no ICC profile,
//! transparency, `pnginfo`, `dpi` or `exif`; the IDAT chunks; IEND.
//!
//! The IDAT chunks are what `ImageFile._save` makes of the zip encoder's
//! output. The PNG plugin hands it an `_idat` object, which has no
//! `fileno`, so the output goes through `_encode_tile`'s Python loop: each
//! `encoder.encode(bufsize)` call returns up to `bufsize` bytes, and each
//! becomes one IDAT chunk through `_idat.write`, where `bufsize` is
//! `max(MAXBLOCK, 4 x width)` and `MAXBLOCK` 65,536. A call returns early
//! only at the stream's end, so every chunk but the last holds `bufsize`
//! bytes.
//!
//! Each call is `ZipEncode.c`'s `ImagingZipEncode`, emulated step for step,
//! since where a call stops decides the chunks: deflate first what input
//! the last call left; then, while the window has room, filter the next row
//! (`crate::filter`) and deflate it with `Z_NO_FLUSH`; after the last row,
//! deflate with `Z_FINISH` until `Z_STREAM_END`. The deflate stream is zlib
//! 1.2.13 at Pillow's settings (`crate::zlib`).
//!
//! The output is one allocation of [`crate::Plan::output`] bytes. Each
//! call's window is `bufsize` bytes, or, near the end of that allocation,
//! one byte more than the stream can still write (its `deflateBound` less
//! what it wrote), which no call can fill, so a call stops exactly where
//! Pillow's would.

use std::fmt;
use std::pin::{Pin, pin};
use std::slice::ChunksExact;

use crate::charge::{self, Buffer, Charger};
use crate::filter::Rows;
use crate::image::{Image, ImageError};
use crate::zlib::{self, Deflater, Z_FINISH, Z_MEM_ERROR, Z_NO_FLUSH, Z_OK, Z_STREAM_END};

/// The PNG signature.
const SIGNATURE: [u8; 8] = *b"\x89PNG\r\n\x1a\n";

/// A PNG file, and the charge of its allocation, held until the bytes are
/// dropped ([`crate::charge`]).
pub struct Png<C> {
    // Before `charge`: fields drop in order, so the bytes are freed before
    // their charge is given back.
    bytes: Vec<u8>,
    charge: C,
}

impl<C> Png<C> {
    /// The file's bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The charge of the file's allocation.
    #[must_use]
    pub fn charge(&self) -> &C {
        &self.charge
    }

    /// The bytes, in their one allocation of [`crate::Plan::output`]
    /// bytes, and their charge, which the caller holds until it frees them.
    #[must_use]
    pub fn into_parts(self) -> (Vec<u8>, C) {
        (self.bytes, self.charge)
    }
}

impl<C> AsRef<[u8]> for Png<C> {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

impl<C> fmt::Debug for Png<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Png")
            .field("len", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

/// Why no PNG was written. `R` is the [`Charger`]'s refusal.
#[derive(Debug, thiserror::Error)]
pub enum EncodeError<R> {
    /// Pillow 9.5.0 writes no PNG of the image, or this crate does not
    /// ([`ImageError::TooLarge`]); nothing was written or charged.
    #[error(transparent)]
    Image(#[from] ImageError),
    /// The charger refused the charge for `buffer`; nothing was allocated
    /// for it.
    #[error("the charge for the {buffer:?} buffer was refused")]
    Refused {
        /// The buffer.
        buffer: Buffer,
        /// The charger's refusal.
        refusal: R,
    },
    /// Rust's global allocator refused `request` bytes for `buffer`, after
    /// its charge was granted.
    #[error("the allocator refused {request} bytes for the {buffer:?} buffer")]
    Alloc {
        /// The buffer.
        buffer: Buffer,
        /// The bytes asked for.
        request: usize,
    },
    /// zlib's `Z_MEM_ERROR`: Rust's global allocator refused a block of the
    /// deflate state (`docs/plans/batch.md` 2.7: `host_memory`).
    #[error("zlib's allocator refused a block of the deflate state")]
    ZlibMemory,
    /// zlib returned another error, or asked for a deflate state larger than
    /// [`crate::DEFLATE_STATE_REQUESTS`], or the stream passed its
    /// `deflateBound`: none can happen with this crate's zlib, so each is a
    /// bug (`internal`).
    #[error("zlib failed: {what}")]
    Zlib {
        /// What failed: zlib's return code, or the bound it passed.
        what: ZlibFailure,
    },
}

/// What [`EncodeError::Zlib`] reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ZlibFailure {
    /// A call returned this code.
    Code(i32),
    /// `zalloc` refused a block past its slot of the charged state.
    StateOverAllowance,
    /// zlib's `deflateBound` is above the planned one, or the stream wrote
    /// past it.
    Bound,
}

impl fmt::Display for ZlibFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Code(code) => write!(f, "return code {code}"),
            Self::StateOverAllowance => f.write_str("a deflate state larger than charged"),
            Self::Bound => f.write_str("output past deflateBound"),
        }
    }
}

/// Where the encoder is in Pillow's encode loop (`state->state`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Rows are left to deflate (1).
    Rows,
    /// Every row is in; deflate with `Z_FINISH` (2).
    Finish,
}

/// Encodes `image` as Pillow 9.5.0's `Image.save(fp, "PNG")` does at its
/// defaults, charging each buffer to `charger` before it is allocated.
pub fn encode<C: Charger>(
    image: &Image<'_>,
    charger: &mut C,
) -> Result<Png<C::Charge>, EncodeError<C::Refusal>> {
    let plan = charge::plan(image.width(), image.height(), image.mode())?;
    let bound = charge::deflate_bound(plan.filtered).ok_or(zlib_failure(ZlibFailure::Bound))?;

    // Each charge is declared before what it pays for, so it drops after it.
    let _state_charge = charge(charger, Buffer::DeflateState, &plan.deflate_state)?;
    let mut z = pin!(Deflater::new());
    match z.as_mut().init() {
        Z_OK => {}
        Z_MEM_ERROR if z.over_allowance() => {
            return Err(zlib_failure(ZlibFailure::StateOverAllowance));
        }
        Z_MEM_ERROR => return Err(EncodeError::ZlibMemory),
        code => return Err(zlib_failure(ZlibFailure::Code(code))),
    }
    if z.as_mut()
        .bound(plan.filtered)
        .is_some_and(|b| b as u64 > bound)
    {
        return Err(zlib_failure(ZlibFailure::Bound));
    }

    let output_charge = charge(charger, Buffer::Output, &[plan.output])?;
    let mut out = zeroed(Buffer::Output, plan.output)?;

    let _rows_charge = charge(charger, Buffer::Rows, &plan.rows)?;
    let mut buffers = [const { Vec::new() }; 5];
    for (buffer, &len) in buffers.iter_mut().zip(&plan.rows) {
        *buffer = zeroed(Buffer::Rows, len)?;
    }
    let mut rows = Rows::new(buffers, image.mode().channels());

    let mut w = Writer {
        out: &mut out,
        len: 0,
    };
    w.put(&SIGNATURE)?;
    // The size, bit depth 8, the colour type, then compression, filter
    // method and interlace 0.
    let [w0, w1, w2, w3] = image.width().to_be_bytes();
    let [h0, h1, h2, h3] = image.height().to_be_bytes();
    let color = image.mode().color_type();
    w.chunk(
        *b"IHDR",
        &[w0, w1, w2, w3, h0, h1, h2, h3, 8, color, 0, 0, 0],
    )?;

    let mut pixels = image.pixels().chunks_exact(image.row_bytes());
    let mut stage = Stage::Rows;
    let mut written = 0_u64;
    loop {
        // One `encoder.encode(bufsize)` call, and the IDAT chunk it makes.
        let head = w.len;
        let data = head
            .checked_add(8)
            .ok_or(zlib_failure(ZlibFailure::Bound))?;
        let room = bound
            .checked_sub(written)
            .and_then(|r| r.checked_add(1))
            .ok_or(zlib_failure(ZlibFailure::Bound))?;
        let window = plan
            .idat_size
            .min(usize::try_from(room).unwrap_or(usize::MAX));
        let end = data
            .checked_add(window)
            .ok_or(zlib_failure(ZlibFailure::Bound))?;
        let Some(output) = w.out.get_mut(data..end) else {
            return Err(zlib_failure(ZlibFailure::Bound));
        };
        let (n, done) = encode_call(z.as_mut(), &mut rows, &mut pixels, &mut stage, output)?;
        w.len = data;
        w.skip(n)?;
        w.seal(head, *b"IDAT")?;
        written = written.saturating_add(n as u64);
        if done {
            break;
        }
    }
    w.chunk(*b"IEND", &[])?;

    let len = w.len;
    out.truncate(len);
    // The rows, then the stream (deflateEnd), drop before their charges,
    // each declared after its charge.
    Ok(Png {
        bytes: out,
        charge: output_charge,
    })
}

/// One call of `ImagingZipEncode` (`ZipEncode.c:23-344`) into `output`:
/// the bytes it wrote there, and whether the stream ended. The two calls
/// that hand zlib its input and output are unsafe: zlib keeps both pointers
/// across calls (their SAFETY comments).
#[allow(unsafe_code)]
fn encode_call<R>(
    mut z: Pin<&mut Deflater>,
    rows: &mut Rows,
    pixels: &mut ChunksExact<'_, u8>,
    stage: &mut Stage,
    output: &mut [u8],
) -> Result<(usize, bool), EncodeError<R>> {
    let window = output.len();
    // SAFETY: the output is neither read nor written, nor moved, until this
    // call's deflates are done (`encode` writes the chunk around it after
    // this returns); the next call sets a new window before it deflates.
    unsafe { z.as_mut().set_output(output) };

    // Input the last call left (ZipEncode.c:125-146).
    if z.avail_in() > 0 {
        deflated(z.as_mut().deflate(Z_NO_FLUSH))?;
    }
    let mut done = false;
    if *stage == Stage::Rows {
        // ZipEncode.c:151-308.
        while z.avail_out() > 0 {
            let Some(row) = pixels.next() else {
                *stage = Stage::Finish;
                break;
            };
            rows.current_pixels().copy_from_slice(row);
            let chosen = rows.choose();
            // SAFETY: zlib reads the chosen row during this deflate and, if
            // the window fills first, during the next call's first deflate
            // (above), before any row is written again: rows are filtered
            // only while the window has room, so only after zlib has read
            // all its input. `advance` swaps the two rows' buffers, moving
            // no bytes, and `rows` outlives the stream (`encode`).
            unsafe { z.as_mut().set_input(rows.chosen(chosen)) };
            deflated(z.as_mut().deflate(Z_NO_FLUSH))?;
            rows.advance();
        }
    }
    if *stage == Stage::Finish {
        // ZipEncode.c:310-334, which this enters with room in the window:
        // the rows ran out while it had room, or in an earlier call.
        while z.avail_out() > 0 {
            let code = z.as_mut().deflate(Z_FINISH);
            if code == Z_STREAM_END {
                done = true;
                break;
            }
            deflated(code)?;
        }
    }
    let left = usize::try_from(z.avail_out()).unwrap_or(usize::MAX);
    Ok((window.saturating_sub(left), done))
}

fn zlib_failure<R>(what: ZlibFailure) -> EncodeError<R> {
    EncodeError::Zlib { what }
}

/// A deflate call's code as Pillow reads it: below 0 is an error
/// (`ZipEncode.c:129`, `:281`).
fn deflated<R>(code: i32) -> Result<(), EncodeError<R>> {
    match code {
        c if c >= 0 => Ok(()),
        Z_MEM_ERROR => Err(EncodeError::ZlibMemory),
        c => Err(zlib_failure(ZlibFailure::Code(c))),
    }
}

fn charge<C: Charger>(
    charger: &mut C,
    buffer: Buffer,
    requests: &[usize],
) -> Result<C::Charge, EncodeError<C::Refusal>> {
    charger
        .charge(buffer, requests)
        .map_err(|refusal| EncodeError::Refused { buffer, refusal })
}

/// `len` zero bytes in one allocation of exactly `len`, or the allocator's
/// refusal.
fn zeroed<R>(buffer: Buffer, len: usize) -> Result<Vec<u8>, EncodeError<R>> {
    let mut v = Vec::new();
    v.try_reserve_exact(len).map_err(|_| EncodeError::Alloc {
        buffer,
        request: len,
    })?;
    v.resize(len, 0);
    Ok(v)
}

/// Writes a PNG into its planned allocation.
struct Writer<'a> {
    out: &'a mut Vec<u8>,
    len: usize,
}

impl Writer<'_> {
    fn put<R>(&mut self, bytes: &[u8]) -> Result<(), EncodeError<R>> {
        let end = self.len.checked_add(bytes.len());
        let slot = end.and_then(|end| self.out.get_mut(self.len..end));
        let Some(slot) = slot else {
            return Err(zlib_failure(ZlibFailure::Bound));
        };
        slot.copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }

    /// Takes `n` bytes already in place.
    fn skip<R>(&mut self, n: usize) -> Result<(), EncodeError<R>> {
        match self.len.checked_add(n) {
            Some(end) if end <= self.out.len() => {
                self.len = end;
                Ok(())
            }
            _ => Err(zlib_failure(ZlibFailure::Bound)),
        }
    }

    /// `putchunk`: the length, the type, the data, the CRC of the type and
    /// the data.
    fn chunk<R>(&mut self, kind: [u8; 4], data: &[u8]) -> Result<(), EncodeError<R>> {
        let head = self.len;
        self.put(&[0; 8])?;
        self.put(data)?;
        self.seal(head, kind)
    }

    /// Closes the chunk whose 8-byte head is at `head` and whose data runs
    /// to the end of what is written: fills in its length and type, then
    /// writes its CRC.
    fn seal<R>(&mut self, head: usize, kind: [u8; 4]) -> Result<(), EncodeError<R>> {
        let data = head.saturating_add(8);
        let len = self.len.saturating_sub(data);
        let len32 = u32::try_from(len).map_err(|_| zlib_failure(ZlibFailure::Bound))?;
        let Some(prefix) = self.out.get_mut(head..data) else {
            return Err(zlib_failure(ZlibFailure::Bound));
        };
        let [l0, l1, l2, l3] = len32.to_be_bytes();
        let [k0, k1, k2, k3] = kind;
        prefix.copy_from_slice(&[l0, l1, l2, l3, k0, k1, k2, k3]);
        let body = self.out.get(data..self.len).unwrap_or_default();
        let crc = zlib::crc32(zlib::crc32(0, &kind), body);
        self.put(&crc.to_be_bytes())
    }
}
