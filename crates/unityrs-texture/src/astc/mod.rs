// Ported from Arm astcenc commit 631fb04 (Apache-2.0): its 2D LDR decompressor, from Source/astcenc_block_sizes.cpp, astcenc_integer_sequence.cpp, astcenc_partition_tables.cpp, astcenc_symbolic_physical.cpp, astcenc_color_unquantize.cpp, astcenc_decompress_symbolic.cpp, astcenc_image.cpp and astcenc_entry.cpp. See THIRD_PARTY_LICENSES.
// Copyright 2011-2024 Arm Limited.
//! ASTC 2D LDR decoding. Reference source: Arm astcenc at commit
//! `631fb04e1c4bca002a8b653f34e044dbb35b32b4` (between 4.8.0 and 5.0.0),
//! as bundled by astc-encoder-py 0.1.12.
//! <https://github.com/ARM-software/astc-encoder/tree/631fb04e1c4bca002a8b653f34e044dbb35b32b4/Source>.
//! All source citations in this module and its children refer to that commit.
//!
//! UnityPy decodes the twelve ASTC formats (`ASTC_RGB_NxN` and
//! `ASTC_RGBA_NxN`, N of 4, 5, 6, 8, 10 and 12) with an astcenc context of
//! the LDR profile into a `U8` image, which makes astcenc round as
//! `decode_unorm8` (astcenc_entry.cpp:1173-1174), on the texture's size
//! padded to whole blocks (`decode`'s module docs). Its pieces:
//!
//! - `block_mode`, `ise`, `partition`, `endpoint` and `tables`: the block
//!   header, integer sequences, partition patterns and colour endpoints;
//! - `context`: the tables one decode builds for its footprint;
//! - `decimation`: a weight grid spread over the texels;
//! - `block`: one block to its texels, void extent and error blocks too;
//! - [`decode_image`]: the blocks in raster order into the image, each edge
//!   block cut to the image, as astcenc's `store_image_block` does
//!   (astcenc_image.cpp:344-359, astcenc_entry.cpp:1151-1216).
//!
//! **Memory.** The decode's tables (`context::Tables`) are its only heap:
//! the block-mode table, the decimation entries of every grid a legal mode
//! names (8 bytes a texel a grid: 1,152 bytes at 4x4 to 100,224 at 12x12),
//! and the partition table (3 x 1,024 seeds x one byte a texel, 442,368
//! bytes at 12x12), each charged before it is allocated (556,960 bytes in
//! all at 12x12, 60,592 at 4x4) and dropped when the decode ends. Their
//! rows are computed the first time a block names them, so a small
//! texture's decode does not pay for the rows it never reads. A block is
//! decoded into an array on the stack.

pub(super) mod block;
pub(super) mod block_mode;
pub(crate) mod context;
pub(super) mod decimation;
pub(super) mod endpoint;
pub(super) mod ise;
pub(super) mod partition;
mod tables;

pub(crate) use block::ERROR;
pub(crate) use context::{Footprint, Tables};

/// A bounded bit read, little endian within each byte. Unlike the C++ reader
/// (astcenc_integer_sequence.cpp:478-490), it never overreads the input.
pub(super) fn read_bits(data: &[u8], offset: usize, count: usize) -> Option<u32> {
    if count > 16 || offset.checked_add(count)? > data.len().checked_mul(8)? {
        return None;
    }
    let mut value = 0;
    for bit in 0..count {
        let pos = offset + bit;
        value |= u32::from((data.get(pos / 8)? >> (pos % 8)) & 1) << bit;
    }
    Some(value)
}

/// Decodes `data`, blocks of `tables`' footprint in raster order, into
/// `out`, a `width` x `height` RGBA image: every block that covers part of
/// the image, each cut at the image's right and bottom edges, as
/// astcenc_image.cpp:344-359 cuts it. Rows go top first, or bottom first
/// when `flip` is set (UnityPy's flip, `decode`'s module docs). A block the
/// data does not hold is left as `out` was; `plan()` refuses such data. An
/// error block is `error`: [`block::ERROR`], but in tests.
pub(crate) fn decode_image(
    tables: &mut Tables,
    data: &[u8],
    (width, height): (usize, usize),
    flip: bool,
    error: [u8; 4],
    out: &mut [u8],
) {
    let fp = tables.footprint();
    let (bw, bh) = (usize::from(fp.width()), usize::from(fp.height()));
    let across = width.div_ceil(bw);
    let blocks = across * height.div_ceil(bh);
    let mut texels = [0; 4 * block::MAX_TEXELS];
    for (i, bytes) in data.as_chunks::<16>().0.iter().take(blocks).enumerate() {
        block::decode(tables, bytes, &mut texels, error);
        let (x0, y0) = ((i % across) * bw, (i / across) * bh);
        let n = bw.min(width - x0);
        for (y, src) in texels.chunks_exact(4 * bw).take(bh).enumerate() {
            let at = y0 + y;
            if at >= height {
                break;
            }
            let row = if flip { height - 1 - at } else { at };
            let start = (row * width + x0) * 4;
            if let (Some(dst), Some(src)) = (out.get_mut(start..start + 4 * n), src.get(..4 * n)) {
                dst.copy_from_slice(src);
            }
        }
    }
}

#[cfg(test)]
mod tests;
