// Ported from CPython 3.11 (PSF-2.0): Lib/zipfile.py. See THIRD_PARTY_LICENSES.
//! ZIP indexing and stored/deflate reads, following `CPython`'s central-directory,
//! ZIP64, local-name and CRC rules. All buffers and work are bounded.

use crate::budget::DecodeBuf;
use crate::error::{Error, LimitError, LimitKind};
use crate::limits::{Limits, Meter, MeteredVec, heap_cost};
use crate::raw::RawStr;
use bytes::Bytes;
use flate2::{Crc, Decompress, FlushDecompress, Status};
use std::sync::Arc;

mod index;
use index::{directory, extra, item, sort, span, u16le, u32le};

/// One member in central-directory order.
#[derive(Debug)]
pub struct Member {
    /// Decoded member name (UTF-8 or CP437), truncated at NUL.
    pub name: RawStr,
    original: RawStr,
    offset: i128,
    end: i128,
    compressed: u64,
    size: u64,
    crc: u32,
    method: u16,
    flags: u16,
}

/// Validated ZIP index. Its bytes, names and tables keep their charges.
#[derive(Debug)]
pub struct ZipArchive {
    data: Bytes,
    members: MeteredVec<Member>,
    meter: Arc<Meter>,
    names: MeteredVec<usize>,
}

const _: () = crate::send_sync::<Member>();
const _: () = crate::send_sync::<ZipArchive>();

pub(crate) fn bad(detail: &'static str) -> Error {
    Error::Malformed {
        what: "zip",
        detail: detail.into(),
    }
}

impl ZipArchive {
    /// Index the central directory. Local headers are checked when read,
    /// as ZipFile.open does (unused duplicate rows need not be readable).
    pub fn open(data: Bytes, meter: &Arc<Meter>) -> Result<Self, Error> {
        let (start, stop, concat) = directory(&data, meter)?;
        let mut at = start;
        let mut members = MeteredVec::new(meter);
        while at < stop {
            members.push(parse_member(&data, &mut at, stop, concat, meter)?)?;
        }
        let mut names = MeteredVec::new(meter);
        for i in 0..members.len() {
            names.push(i)?;
        }
        // CPython assigns each entry the next header's offset in reverse
        // stable order, including equal offsets (only the first can be read).
        sort(names.as_mut_slice(), |a, b| {
            meter.count_decode(1)?;
            Ok(item(members.as_slice(), *a)?
                .offset
                .cmp(&item(members.as_slice(), *b)?.offset)
                .then(b.cmp(a)))
        })?;
        let mut end = start as i128;
        for &i in names.as_slice().iter().rev() {
            let m = members
                .as_mut_slice()
                .get_mut(i)
                .ok_or_else(|| bad("member index"))?;
            m.end = end;
            end = m.offset;
        }
        sort(names.as_mut_slice(), |a, b| {
            let (a_name, b_name) = (
                &item(members.as_slice(), *a)?.name,
                &item(members.as_slice(), *b)?.name,
            );
            meter.count_decode(a_name.len().min(b_name.len()) as u64 + 1)?;
            Ok(a_name.cmp(b_name).then(a.cmp(b)))
        })?;
        Ok(Self {
            data,
            members,
            meter: meter.clone(),
            names,
        })
    }

    /// Members in central-directory order, duplicates included.
    #[must_use]
    pub fn members(&self) -> &[Member] {
        self.members.as_slice()
    }

    /// Last central-directory member of this name, as ZipFile.open(name).
    /// Comparisons consume work and may refuse the search.
    pub fn find(&self, name: &[u8]) -> Result<Option<usize>, Error> {
        let (mut lo, mut hi) = (0, self.names.len());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let m = item(self.members.as_slice(), *item(self.names.as_slice(), mid)?)?;
            self.meter
                .count_decode(m.name.len().min(name.len()) as u64 + 1)?;
            if m.name.as_bytes() <= name {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let Some(i) = lo
            .checked_sub(1)
            .and_then(|i| self.names.as_slice().get(i))
            .copied()
        else {
            return Ok(None);
        };
        let m = item(self.members.as_slice(), i)?;
        self.meter
            .count_decode(m.name.len().min(name.len()) as u64 + 1)?;
        Ok((m.name.as_bytes() == name).then_some(i))
    }

    fn input(&self, m: &Member) -> Result<&[u8], Error> {
        let offset = usize::try_from(m.offset).map_err(|_| bad("bad local offset"))?;
        let h = span(&self.data, offset, 30)?;
        if h.get(..4) != Some(b"PK\x03\x04") {
            return Err(bad("bad local signature"));
        }
        let n = usize::from(u16le(h, 26)?);
        let x = usize::from(u16le(h, 28)?);
        let raw = span(
            &self.data,
            offset + 30,
            n.min(self.data.len() - offset - 30),
        )?;
        self.meter.count_decode((30 + n + x) as u64)?;
        let local = member_name(raw, u16le(h, 6)?, &self.meter)?;
        if local != m.original {
            return Err(bad("local name mismatch"));
        }
        // Local extra bytes are skipped, never parsed. Local sizes, flags,
        // method, CRC and any data descriptor do not determine the payload.
        let begin = offset
            .checked_add(30 + n + x)
            .ok_or_else(|| bad("member overflow"))?
            .min(self.data.len());
        if begin as i128 + i128::from(m.compressed) > m.end {
            return Err(bad("overlapping members"));
        }
        if m.flags & 0x61 != 0 {
            return Err(bad("encrypted or patched ZIP is unsupported"));
        }
        if !matches!(m.method, 0 | 8) {
            return Err(bad("unsupported compression method"));
        }
        let compressed = usize::try_from(m.compressed).unwrap_or(usize::MAX);
        span(&self.data, begin, compressed.min(self.data.len() - begin))
    }

    /// Loading opens every member, but a zero-size stream's Length/seek does
    /// not read payload or check CRC. Keep all ZipFile.open metadata checks.
    pub(crate) fn read_for_load(&self, index: usize, limits: &Limits) -> Result<Bytes, Error> {
        let m = self
            .members
            .as_slice()
            .get(index)
            .ok_or_else(|| bad("member index"))?;
        if m.size == 0 {
            self.input(m)?;
            return Ok(Bytes::new());
        }
        self.read(index, limits)
    }

    /// Read the central compressed extent, truncating output to `file_size` as
    /// `ZipExtFile` does. CRC at EOF decides validity, including short streams.
    /// Trailing compressed bytes and a larger declared size are permitted.
    pub fn read(&self, index: usize, limits: &Limits) -> Result<Bytes, Error> {
        let m = self
            .members
            .as_slice()
            .get(index)
            .ok_or_else(|| bad("member index"))?;
        let input = self.input(m)?;
        // Never allocate from an untrusted declared count. DecodeBuf charges
        // every later growth before allocation; empty payloads allocate nothing.
        let mut out = DecodeBuf::new(
            &self.meter,
            0,
            m.size.min(limits.max_inflate),
            LimitKind::Inflate,
        )?;
        let short = (input.len() as u64) < m.compressed;
        if input.is_empty() && short {
            return Err(bad("unexpected EOF"));
        }
        if m.method == 0 {
            self.meter.count_decode(input.len() as u64)?;
            let bytes = input
                .get(
                    ..input
                        .len()
                        .min(usize::try_from(m.size).unwrap_or(usize::MAX)),
                )
                .unwrap_or_default();
            self.meter.count_decode(bytes.len() as u64)?;
            out.extend_from_slice(bytes)?;
            if short && (out.len() as u64) < m.size {
                return Err(bad("unexpected EOF"));
            }
        } else {
            inflate(input, &mut out, m.size, short, limits, &self.meter)?;
        }
        let mut crc = Crc::new();
        crc.update(out.as_slice());
        if crc.sum() != m.crc {
            return Err(bad("Bad CRC-32"));
        }
        Ok(out.into_bytes()?)
    }
}

/// flate2 1.1.10's zlib-rs backend allocates one block containing decoder
/// state, a 32 KiB window, SIMD padding and alignment. Reserve a conservative
/// 64 KiB (plus allocator overhead) before construction; `zip_alloc` measures
/// construction and decoding with the testkit's counting allocator.
const DEFLATE_HEAP: u64 = heap_cost(64 * 1024);

/// `UnityPy` asks its stream reader for Length first: `ZipExtFile.seek` reads
/// through the declared size in `MAX_SEEK_READ` chunks. Each read uses
/// `_read1`/`_read2`, with at least `MIN_READ_SIZE` compressed and output bytes.
/// Finish that logical output window before applying `_left`, even when it
/// discards output or encounters corruption beyond the retained prefix.
fn inflate(
    input: &[u8],
    out: &mut DecodeBuf,
    size: u64,
    short: bool,
    limits: &Limits,
    meter: &Arc<Meter>,
) -> Result<(), Error> {
    const MIN_READ_SIZE: usize = 4096;
    const MAX_SEEK_READ: u64 = 1 << 24;
    // Declaration order matters: the decoder drops before its reservation.
    let _backend = meter.charge(DEFLATE_HEAP)?;
    let mut dec = Decompress::new(false);
    let (mut pos, mut end) = (0_usize, 0_usize);
    let mut scratch = [0; 8192];
    let mut seek_end = size.min(MAX_SEEK_READ);
    let mut read_left = seek_end;
    loop {
        let n = usize::try_from(read_left).map_err(|_| bad("inflate read size"))?;
        let tail = end - pos;
        let next = if n > tail {
            input
                .len()
                .min(end.saturating_add((n - tail).max(MIN_READ_SIZE)))
        } else {
            end
        };
        // An initial zero-sized member still gets one minimum window to
        // validate its stream and CRC, without reserving output storage.
        let next = if end == 0 {
            input.len().min(n.max(MIN_READ_SIZE))
        } else {
            next
        };
        let max_length = n.max(MIN_READ_SIZE);
        meter.count_decode((next - end) as u64)?;
        end = next;
        let mut window = 0;
        loop {
            // When all compressed bytes have been consumed, CPython flushes
            // buffered output, even past max_length, before slicing to _left.
            let flushing = pos == end && end == input.len() && !short;
            let room = if flushing {
                scratch.len()
            } else {
                scratch.len().min(max_length - window)
            };
            let before = (dec.total_in(), dec.total_out());
            let status = dec.decompress(
                input.get(pos..end).unwrap_or_default(),
                scratch
                    .get_mut(..room)
                    .ok_or_else(|| bad("inflate window"))?,
                FlushDecompress::None,
            );
            let used = usize::try_from(dec.total_in() - before.0)
                .map_err(|_| bad("inflate input overflow"))?;
            let wrote = usize::try_from(dec.total_out() - before.1)
                .map_err(|_| bad("inflate output overflow"))?;
            pos += used;
            window += wrote;
            meter.count_decode(wrote as u64)?;
            if dec.total_out() > limits.max_inflate {
                return Err(Error::Limit(LimitError::refused(
                    LimitKind::Inflate,
                    dec.total_out(),
                    limits.max_inflate,
                )));
            }
            let status = status.map_err(|_| bad("invalid deflate stream"))?;
            let keep = wrote.min(usize::try_from(size - out.len() as u64).unwrap_or(usize::MAX));
            out.extend_from_slice(scratch.get(..keep).ok_or_else(|| bad("inflate overflow"))?)?;
            if status == Status::StreamEnd {
                return Ok(());
            }
            if used == 0 && wrote == 0 {
                if pos == end {
                    if end == input.len() {
                        if short && (out.len() as u64) < size {
                            return Err(bad("unexpected EOF"));
                        }
                        return Ok(());
                    }
                    break;
                }
                return Err(bad("invalid deflate stream"));
            }
            if window >= max_length && !(pos == end && end == input.len() && !short) {
                break;
            }
        }
        // _left is applied only after _read1 has completed its window.
        if (out.len() as u64) >= size {
            return Ok(());
        }
        if out.len() as u64 >= seek_end {
            seek_end = seek_end.saturating_add(MAX_SEEK_READ).min(size);
        }
        // read() buffers _read1's excess beyond its requested length. It
        // satisfies the start of the next seek chunk without decoding again.
        // All decoded bytes already live in out; keep the seek boundary
        // independent of that buffer's length so the excess carries forward.
        read_left = seek_end - out.len() as u64;
    }
}

fn parse_member(
    data: &[u8],
    at: &mut usize,
    stop: usize,
    concat: i128,
    meter: &Arc<Meter>,
) -> Result<Member, Error> {
    let h = span(data, *at, 46)?;
    if *at + 46 > stop || h.get(..4) != Some(b"PK\x01\x02") {
        return Err(bad("bad central signature"));
    }
    if h.get(6).is_some_and(|&version| version > 63) {
        return Err(bad("unsupported ZIP version"));
    }
    let flags = u16le(h, 8)?;
    let n = usize::from(u16le(h, 28)?);
    let x = usize::from(u16le(h, 30)?);
    let c = usize::from(u16le(h, 32)?);
    meter.count_decode((46 + n + x + c) as u64)?;
    let raw = data
        .get(*at + 46..(*at + 46 + n).min(stop))
        .ok_or_else(|| bad("short name"))?;
    let original = member_name(raw, flags, meter)?;
    let name = RawStr::charged(
        original
            .as_bytes()
            .split(|&b| b == 0)
            .next()
            .unwrap_or_default(),
        meter,
    )?;
    let mut values = [
        u64::from(u32le(h, 24)?),
        u64::from(u32le(h, 20)?),
        u64::from(u32le(h, 42)?),
    ];
    let extra_start = (*at + 46 + n).min(stop);
    extra(
        span(data, extra_start, x.min(stop - extra_start))?,
        &mut values,
    )?;
    *at += 46 + n + x + c;
    let offset = i128::from(values[2]) + concat;
    Ok(Member {
        name,
        original,
        offset,
        end: stop as i128,
        compressed: values[1],
        size: values[0],
        crc: u32le(h, 16)?,
        method: u16le(h, 10)?,
        flags,
    })
}

fn member_name(raw: &[u8], flags: u16, meter: &Arc<Meter>) -> Result<RawStr, Error> {
    if flags & 0x800 != 0 {
        std::str::from_utf8(raw).map_err(|_| bad("invalid UTF-8 member name"))?;
        return Ok(RawStr::charged(raw, meter)?);
    }
    let mut out = DecodeBuf::new(meter, 0, raw.len() as u64 * 3, LimitKind::InputMemory)?;
    for &b in raw {
        if b < 128 {
            out.extend_from_slice(&[b])?;
        } else {
            let c = CP437
                .chars()
                .nth(usize::from(b - 128))
                .ok_or_else(|| bad("CP437 name"))?;
            out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes())?;
        }
    }
    Ok(RawStr::from_shared(out.into_bytes()?))
}

// IBM PC code page 437's upper half, in byte order.
const CP437: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■ ";
