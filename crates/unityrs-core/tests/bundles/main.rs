//! `BundleFile`'s lazy reads against its eager load (plan sections 3.6 and
//! 7.4): one layout in every read order, runs and kept block outcomes,
//! refusals that are not kept, decode work, no lock across caller code, the
//! entries' weak parent handle, entry slicing as Python slices, and the
//! input's meter over several files and hostile directories, the work an
//! entry's parse counts, what a read's search of its blocks and runs
//! costs, rows over a nested bundle whose scan fails, and the counts of
//! what reads decode, discard, decode again and cache (`counts`).

use std::sync::Arc;

use unityrs_core::budget::Reservation;
use unityrs_core::files::BundleFile;
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, Error, FileContext, RawStr};
use unityrs_testkit::bundle::{EntrySpec, matrix};

mod caller_code;
mod counts;
mod entries;
mod format;
mod layout;
mod memory;
mod nested;
mod outcomes;
mod runs;
mod work;

fn ctx() -> Arc<FileContext> {
    Arc::new(FileContext::unitypy())
}

/// The UnityPy context, changed by `f`.
fn ctx_with(f: impl FnOnce(&mut FileContext)) -> Arc<FileContext> {
    let mut c = FileContext::unitypy();
    f(&mut c);
    Arc::new(c)
}

fn meter_for(ctx: &FileContext) -> Arc<Meter> {
    Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input)
}

fn open_with(
    data: &[u8],
    ctx: &Arc<FileContext>,
    meter: &Arc<Meter>,
) -> Result<Arc<BundleFile>, Error> {
    BundleFile::open(
        RawStr::from("test.bundle"),
        Bytes::copy_from_slice(data),
        None,
        meter,
        ctx,
    )
}

fn open(data: &[u8]) -> Arc<BundleFile> {
    let c = ctx();
    open_with(data, &c, &meter_for(&c)).unwrap()
}

fn by_name(name: &str) -> Vec<u8> {
    matrix()
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("{name}"))
        .1
}

fn all_entries(b: &BundleFile, order: &[usize]) -> Vec<Result<Bytes, String>> {
    let mut out = vec![Err(String::new()); b.directory().len()];
    for &i in order {
        out[i] = b.entry_bytes(i).map_err(|e| e.to_string());
    }
    out
}

fn small_entries() -> Vec<EntrySpec> {
    vec![
        EntrySpec {
            name: b"a".to_vec(),
            data: (0..100u8).collect(),
            flags: 0,
        },
        EntrySpec {
            name: b"b".to_vec(),
            data: (0..300u32).map(|i| (i % 7) as u8).collect(),
            flags: 0,
        },
    ]
}
