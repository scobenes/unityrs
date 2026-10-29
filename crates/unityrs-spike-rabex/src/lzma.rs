//! `rabex-dump lzma`: rabex's LZMA codec against liblzma, head to head on
//! every LZMA block of the selected bundles (docs/spikes/rabex.md section 5).
//!
//! Blocks come from rabex's own `BundleFileReader` (its block list, and the
//! data offset its reader stops at). Each block is decoded twice, single
//! threaded, from memory:
//! - with rabex 0.2.1's exact call (`bundlefile/mod.rs`, `decompress_block`):
//!   `lzma_rs::lzma_decompress_with_options` with
//!   `UnpackedSize::UseProvided(Some(uncompressed_size))`;
//! - with liblzma's raw LZMA1 decoder, the 5 property bytes first, as
//!   UnityPy's `decompress_lzma` does through Python's `lzma`.
//!
//! The two outputs must be equal; the run counts any that differ.

use std::io::Cursor;
use std::time::{Duration, Instant};

use rabex::files::bundlefile::{BundleFileReader, ExtractionConfig};
use serde_json::json;

use crate::dump::input_bytes;
use crate::{Args, collect_inputs};

/// rabex 0.2.1's call, verbatim but for the error type.
fn lzma_rs_decode(block: &[u8], unc: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    lzma_rs::lzma_decompress_with_options(
        &mut &block[..],
        &mut out,
        &lzma_rs::decompress::Options {
            unpacked_size: lzma_rs::decompress::UnpackedSize::UseProvided(Some(u64::from(unc))),
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())?;
    Ok(out)
}

fn liblzma_decode(block: &[u8], unc: u32) -> Result<Vec<u8>, String> {
    use liblzma::stream::{Action, Filters, Stream};
    let props = block.get(..5).ok_or("block shorter than its properties")?;
    let mut out = vec![0u8; unc as usize];
    let mut filters = Filters::new();
    filters.lzma1_properties(props).map_err(|e| e.to_string())?;
    let mut s = Stream::new_raw_decoder(&filters).map_err(|e| e.to_string())?;
    let input = &block[5..];
    loop {
        let (r, w) = (to_usize(s.total_in()), to_usize(s.total_out()));
        if w == out.len() {
            return Ok(out);
        }
        s.process(&input[r..], &mut out[w..], Action::Run)
            .map_err(|e| e.to_string())?;
        if to_usize(s.total_in()) == r && to_usize(s.total_out()) == w {
            return Err("liblzma made no progress".into());
        }
    }
}

fn to_usize(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// The version string of the liblzma linked in.
fn liblzma_version() -> String {
    // SAFETY: lzma_version_string takes no arguments and returns a pointer
    // to a static, NUL-terminated string.
    unsafe { std::ffi::CStr::from_ptr(liblzma_sys::lzma_version_string()) }
        .to_string_lossy()
        .into_owned()
}

#[allow(clippy::cast_precision_loss)] // seconds and megabytes for a report
pub fn lzma(a: &Args) -> Result<(), String> {
    let (mut t_rs, mut t_lib) = (Duration::ZERO, Duration::ZERO);
    let (mut blocks, mut bundles, mut unreadable, mut mismatches) = (0u64, 0u64, 0u64, 0u64);
    let (mut bytes_in, mut bytes_out) = (0u64, 0u64);
    for (abs, _, _) in collect_inputs(a)? {
        let data = std::fs::read(&abs).map_err(|e| e.to_string())?;
        let input = input_bytes(&data, a.shim);
        let Ok(r) =
            BundleFileReader::from_reader(Cursor::new(&*input), &ExtractionConfig::default())
        else {
            unreadable += 1;
            continue;
        };
        bundles += 1;
        let mut pos = to_usize(r.reader.position());
        for b in r.blocks() {
            let end = pos + b.compressed_size as usize;
            let block = input
                .get(pos..end)
                .ok_or("block past the end of the file")?;
            pos = end;
            if b.flags & 0x3f != 1 {
                continue;
            }
            let t0 = Instant::now();
            let x = lzma_rs_decode(block, b.uncompressed_size)?;
            t_rs += t0.elapsed();
            let t1 = Instant::now();
            let y = liblzma_decode(block, b.uncompressed_size)?;
            t_lib += t1.elapsed();
            mismatches += u64::from(x != y);
            blocks += 1;
            bytes_in += block.len() as u64;
            bytes_out += y.len() as u64;
        }
    }
    let mb = bytes_out as f64 / 1e6;
    let (s_rs, s_lib) = (t_rs.as_secs_f64(), t_lib.as_secs_f64());
    let out = json!({
        "bundles": bundles,
        "unreadable_bundles": unreadable,
        "shim_fs6": a.shim,
        "lzma_blocks": blocks,
        "compressed_bytes": bytes_in,
        "uncompressed_bytes": bytes_out,
        "output_mismatches": mismatches,
        "lzma_rs": {"version": "0.3.0", "seconds": s_rs, "mb_per_s": mb / s_rs},
        "liblzma": {"version": liblzma_version(), "crate": "liblzma 0.4.9, static", "seconds": s_lib, "mb_per_s": mb / s_lib},
        "ratio_lzma_rs_over_liblzma": s_rs / s_lib,
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    Ok(())
}
