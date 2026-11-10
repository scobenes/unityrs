//! `bench --many-blocks [--reads N]`: runs of many blocks, which the
//! corpus lacks (its 1,100 bundles hold 1,105 blocks, at most 6 a bundle),
//! so its passes cannot show what decoding a long run costs (perf stage 2's
//! review: while each block's room ended at that block's own end, a run of
//! 4,096 blocks reallocated its buffer 4,096 times). Each case is one entry
//! of 64 MiB cut into LZ4 blocks (of 128 KiB, the chunk Unity's LZ4 bundles
//! use, and of 16 KiB), built by the testkit before any clock starts. Each
//! read opens the bundle on a meter of its own and reads the entry whole
//! (`entry_bytes`: one run, every block decoded into it), timed by the wall
//! clock. It prints one JSON object: each case's size, block, blocks and
//! reads, and its reads' best and median in ms, with the best's MB/s.

use std::sync::Arc;
use std::time::Instant;

use serde_json::{Value, json};
use unityrs_core::budget::Reservation;
use unityrs_core::files::BundleFile;
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build};

/// The cases: an entry's bytes and its blocks' size.
pub const CASES: [(usize, usize); 2] = [(64 << 20, 128 << 10), (64 << 20, 16 << 10)];

/// Each case's reads, unless `--reads` says otherwise.
pub const READS: usize = 5;

/// `n` bytes no two neighbours of which are equal, which LZ4 compresses
/// some way.
fn data(n: usize) -> Vec<u8> {
    (0..n)
        .map(|i| u8::try_from((i * 7 + i / 4093) % 251).unwrap_or(0))
        .collect()
}

#[allow(clippy::cast_precision_loss)]
fn f(n: usize) -> f64 {
    n as f64
}

/// Times `reads` whole reads of each of `cases` (module docs).
pub fn many_blocks(cases: &[(usize, usize)], reads: usize) -> Result<Value, String> {
    let start = Instant::now();
    let ctx = Arc::new(FileContext::unitypy());
    let mut out = Vec::new();
    for &(bytes, block) in cases {
        let bundle = Bytes::from(build(&BundleSpec {
            data_codec: TestCodec::Lz4,
            block_size: block,
            entries: vec![EntrySpec {
                name: b"runs/entry".to_vec(),
                data: data(bytes),
                flags: 0,
            }],
            ..BundleSpec::default()
        }));
        let mut ms = Vec::with_capacity(reads);
        for _ in 0..reads {
            let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
            let b = BundleFile::open(
                RawStr::from("runs.bundle"),
                bundle.clone(),
                None,
                &meter,
                &ctx,
            )
            .map_err(|e| e.to_string())?;
            let t = Instant::now();
            let got = b.entry_bytes(0).map_err(|e| e.to_string())?;
            ms.push(t.elapsed().as_secs_f64() * 1e3);
            if got.len() != bytes {
                return Err(format!("read {} of {bytes} bytes", got.len()));
            }
        }
        ms.sort_by(f64::total_cmp);
        let best = ms.first().copied().unwrap_or(0.0);
        let median = ms.get(ms.len() / 2).copied().unwrap_or(0.0);
        out.push(json!({
            "bytes": bytes,
            "block": block,
            "blocks": bytes.div_ceil(block),
            "reads": reads,
            "best_ms": best,
            "median_ms": median,
            "best_mb_per_s": f(bytes) / 1e6 / (best / 1e3),
        }));
    }
    Ok(json!({
        "bench": "unityrs-many-block-runs",
        "wall_s": start.elapsed().as_secs_f64(),
        "cases": out,
    }))
}

/// `--many-blocks`'s own arguments (after it): `--reads N`, or nothing.
pub fn reads(args: &[String]) -> Result<usize, String> {
    match args {
        [] => Ok(READS),
        [flag, n] if flag == "--reads" => n
            .parse()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| "--reads takes a count of at least 1".to_owned()),
        _ => Err(format!("--many-blocks takes --reads N, not {args:?}")),
    }
}
