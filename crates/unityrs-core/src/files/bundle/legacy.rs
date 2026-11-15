// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/BundleFile.py. See THIRD_PARTY_LICENSES.
//! Legacy `read_web_raw`, eagerly decoded once; entry cells share the FS reader.
use super::counts::DecodeCounts;
use super::decode::BlockContext;
use super::index::{Ends, RunIndex};
use super::open::{files_dict, keep, twins};
use super::{BundleFile, BundleHeader, DirEntry, LayoutKind};
use crate::budget::DecodeBuf;
use crate::codec::{BlockCodec, Lzma};
use crate::counters::Origin;
use crate::error::{Error, LimitKind};
use crate::files::Parent;
use crate::limits::{Meter, MeteredVec, heap_cost};
use crate::options::FileContext;
use crate::raw::RawStr;
use crate::stream::{Endian, Reader, ReaderKind};
use crate::typetree::parse::string_to_null;
use bytes::Bytes;
use std::sync::Arc;

#[allow(clippy::too_many_arguments)]
#[expect(
    clippy::too_many_lines,
    reason = "format-gated legacy header and charged container construction"
)]
pub(super) fn open(
    name: RawStr,
    source: Bytes,
    parent: Option<Parent>,
    meter: &Arc<Meter>,
    ctx: &Arc<FileContext>,
    kind: ReaderKind,
    source_depth: u32,
) -> Result<Arc<BundleFile>, Error> {
    let mut charge = meter.charge(heap_cost((size_of::<BundleFile>() + 16) as u64))?;
    let mut r = Reader::with_kind(&source, Endian::Big, kind);
    // files/BundleFile.py:36-47; formats other than 6 take this reader.
    let signature = string_to_null(&mut r, &source, meter)?;
    let format = r.read_u32()?;
    let version_player = string_to_null(&mut r, &source, meter)?;
    let version_engine = string_to_null(&mut r, &source, meter)?;
    // files/BundleFile.py:55-94: skipped level rows and declarations are
    // not allocation hints. A negative level count moves the signed cursor.
    let (hash, crc) = if format >= 4 {
        let bytes = r.read_bytes_short(16);
        (
            Some(<[u8; 16]>::try_from(bytes).map_err(|_| Error::Malformed {
                what: "web/raw hash",
                detail: "short hash".into(),
            })?),
            Some(r.read_u32()?),
        )
    } else {
        (None, None)
    };
    let _minimum = r.read_u32()?;
    let header_size = r.read_u32()?;
    let _download = r.read_u32()?;
    let levels = r.read_i32()?;
    r.set_position(r.position() + 8 * (i128::from(levels) - 1))?;
    let compressed = r.read_u32()?;
    let _uncompressed = r.read_u32()?;
    if format >= 2 {
        r.read_u32()?;
    }
    if format >= 3 {
        r.read_u32()?;
    }
    r.set_position(i128::from(header_size))?;
    let range = r.read_range(i64::from(compressed))?;
    let packed = source.slice(range);
    let data = if signature.as_bytes() == b"UnityWeb" {
        inflate(&packed, meter, ctx)?
    } else {
        packed
    };
    let mut r = Reader::new(&data, Endian::Big);
    let mut directory = MeteredVec::new(meter);
    for _ in 0..r.read_i32()?.max(0) {
        let path = string_to_null(&mut r, &data, meter)?;
        let offset = i64::from(r.read_u32()?);
        let size = i64::from(r.read_u32()?);
        directory.push(DirEntry {
            path,
            offset,
            size,
            flags: None,
        })?;
    }
    let files = keep(files_dict(&directory, meter)?, &mut charge)?;
    let directory = keep(directory, &mut charge)?;
    let twins = keep(twins(&directory, meter)?, &mut charge)?;
    let mut entries = MeteredVec::new(meter);
    for _ in &directory {
        entries.push(super::super::teardown::FileCell::new())?;
    }
    let entries = keep(entries, &mut charge)?;
    let (ends, c) = Ends::new(0, meter)?;
    charge.absorb(c);
    let runs = RunIndex::mutex(&mut charge)?;
    let header = BundleHeader {
        signature,
        format,
        version_player,
        version_engine,
        layout: LayoutKind::WebRaw,
        engine_version: None,
        data_flags: None,
        flags_class: None,
        block_alignment: false,
        encrypted: false,
        hash,
        crc,
        blocks_info: None,
    };
    Ok(Arc::new_cyclic(|this| BundleFile {
        changed: std::sync::atomic::AtomicBool::new(false),
        name,
        source,
        legacy_data: Some(data),
        header,
        blocks: Box::default(),
        directory,
        files,
        parent,
        source_depth,
        lengths: Box::default(),
        ends,
        runs,
        counts: DecodeCounts::default(),
        entries,
        twins,
        charge,
        ctx: BlockContext::new(ctx, false, None),
        stored: Box::default(),
        this: this.clone(),
        origin: Origin::opened(),
    }))
}

/// helpers/CompressionHelper.py:17-44: five properties, skip eight size bytes.
/// The stored size is never a bound or allocation hint. Start with one byte.
fn inflate(packed: &Bytes, meter: &Arc<Meter>, ctx: &FileContext) -> Result<Bytes, Error> {
    let mut out = DecodeBuf::inflated(meter, ctx.limits.max_inflate)?;
    // As in bundle/decode.rs: pay for every packed byte before copying or
    // decoding, including trailing bytes LZMA may leave unread. Output uses
    // that credit, so the decode counts the larger of packed and written.
    out.prepay(packed.len() as u64)?;
    let mut copied = 0;
    let got = (|| {
        let mut input = DecodeBuf::new(meter, 0, packed.len() as u64, LimitKind::InputBytes)?;
        input.extend_from_slice(packed.get(..5).unwrap_or(packed))?;
        copied = input.len() as u64;
        input.extend_from_slice(packed.get(13..).unwrap_or_default())?;
        copied = input.len() as u64;
        Lzma.decode(&input.into_bytes()?, 1, &mut out)
    })();
    let unused = out.end_prepaid();
    if matches!(got, Err(Error::Budget(_) | Error::Allocator(_))) {
        // Unused output credit still pays for input already copied. Refund
        // only prepayment for bytes neither copied nor written: skipped size
        // bytes and any input a refused allocation prevented us from copying.
        meter.uncount_decode(unused.min((packed.len() as u64).saturating_sub(copied)));
    }
    got?;
    Ok(out.into_bytes()?)
}
