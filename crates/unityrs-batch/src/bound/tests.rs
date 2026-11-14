//! 5.1's bound term by term, on specs small enough to sum by hand.

use unityrs_core::limits::{Limits, heap_cost};

use super::{
    C_FIN, CHECK_ENV_BLOCK, CONTAINER_LEVEL_STACK, PANIC_SLOT, PROVISIONAL, THREAD_STACK,
    WORKER_STATE, c_hdr, memory_bound,
};
use crate::alloc::Measured;
use crate::spec::model::Spec;

const MIB: u64 = 1 << 20;

fn spec(extra: &str) -> Spec {
    let text = format!(
        r#"{{"spec_version":1,"run_id":"r","attempt":1,"threads":3,"download_concurrency":2,
        "max_inflight_uncompressed_bytes":268435456,"max_decoded_bytes":16777216,
        "dep_cache_bytes":33554432,"max_view_bundles":10,
        "spool":{{"dir":"/spool","size_bytes":1073741824,"reserve_bytes":0}},
        "outputs":{{"textures":{{"key":"{{input}}/{{name}}.png"}}}}{extra}}}"#
    );
    crate::spec::parse(text.into_bytes())
        .unwrap_or_else(|e| panic!("{e}"))
        .spec
}

const DIRECTORY: &str = r#","sink":{"kind":"directory","root":"/out"}"#;

#[test]
fn every_term_is_5_3_s() {
    let s = spec(&format!(
        r#","inputs":[{{"id":"a","path":"/a"}},{{"id":"b","path":"/b"}}],
        "archives":[{{"key":"z.zip","members":[{{"kind":"texture","path":"{{name}}"}}]}}]{DIRECTORY}"#
    ));
    let measured = Measured {
        kept: 10_000,
        peak: 20_000,
    };
    let b = memory_bound(&s, measured, CHECK_ENV_BLOCK, &PROVISIONAL);
    let k = PROVISIONAL;
    let stack = Limits::default().stack_needed() as u64;
    // threads 3, download_concurrency 2: 6 blocking-pool threads, 5 sockets.
    let worker = 2 * stack + CONTAINER_LEVEL_STACK * 17 + WORKER_STATE + PANIC_SLOT;
    assert_eq!(b.i, 268_435_456);
    assert_eq!(
        b.c_thr,
        3 * worker + 5 * THREAD_STACK + 6 * MIB + 6 * THREAD_STACK
    );
    assert_eq!(b.c_env, 3 * (k.c_env + 10 * 1024));
    assert_eq!(b.c_net, 5 * k.c_net + 2 * 2 * MIB + 5 * 64 * 1024);
    assert_eq!(b.c_spec, 10_000, "the measure alone: no per-bundle term");
    assert_eq!(b.c_parse, 0, "p is far below I");
    assert_eq!(b.c_hdr, 0, "no header, no object store");
    assert_eq!(b.c_fin, C_FIN);
    assert_eq!(
        b.c_base,
        k.c_base
            + 3 * k.m_thread
            + 6 * k.m_pool
            + 2 * 2080
            + 3 * k.m_dispatch
            + 3 * k.m_end
            + 16 * 1024
            + 160
    );
    assert_eq!(
        b.total(),
        b.i + b.c_thr + b.c_env + b.c_net + b.c_spec + b.c_parse + b.c_hdr + b.c_fin + b.c_base
    );
}

/// `C_parse` is what phase 1's peak passes `C_spec + I + C_fin` by.
#[test]
fn c_parse_is_the_peak_past_what_the_rest_holds() {
    let s = spec(&format!(
        r#","inputs":[{{"id":"a","path":"/a"}}]{DIRECTORY}"#
    ));
    let kept = 1000;
    let c_spec = kept;
    let at = c_spec + 268_435_456 + C_FIN;
    for (peak, want) in [(at - 1, 0), (at, 0), (at + 5, 5)] {
        let b = memory_bound(&s, Measured { kept, peak }, 0, &PROVISIONAL);
        assert_eq!(b.c_parse, want, "{peak}");
    }
}

/// `C_hdr`: each distinct value once, every `{"env": NAME}` at 16 KiB,
/// the table and the masker, the largest block `4 x download_concurrency`
/// times, and the credentials only where an object store reads them.
#[test]
fn c_hdr_counts_distinct_values_and_the_largest_block() {
    let s = spec(&format!(
        r#","inputs":[
            {{"id":"a","url":"https://h/a","headers":{{"X-A":"vv","X-B":{{"env":"TOKEN"}}}}}},
            {{"id":"b","url":"https://h/b","headers":{{"X-A":"vv","Au":{{"env":"TOKEN"}}}}}}]{DIRECTORY}"#
    ));
    let values = heap_cost(2) + heap_cost(16 << 10);
    let table = heap_cost(48) + heap_cost(5);
    let masker = heap_cost(32);
    // a's block: "X-A: vv\r\n" (3 + 4 + 2) and "X-B: <16 KiB>\r\n".
    let block = (3 + 4 + 2) + (3 + 4 + (16 << 10));
    let want = values + table + masker + 4 * 2 * heap_cost(block);
    assert_eq!(c_hdr(&s), want);
    let store = spec(
        r#","inputs":[{"id":"a","path":"/a"}],"deadline":"2026-01-01T00:00:00Z",
        "sink":{"kind":"object_store","bucket":"b-1","region":"r"}"#,
    );
    assert_eq!(
        c_hdr(&store),
        6 * (heap_cost(128) * 2 + heap_cost(16 << 10))
    );
}

/// A version 2 spec's UnityCN key variable is one more `{"env": NAME}`
/// value at its 16 KiB cap; a spec naming none adds nothing.
#[test]
fn c_hdr_counts_the_unitycn_key_at_its_cap() {
    let at = |version: u32, key: &str| {
        let text = format!(
            r#"{{"spec_version":{version},"run_id":"r","attempt":1,"threads":3,
            "download_concurrency":2,"max_inflight_uncompressed_bytes":268435456,
            "max_decoded_bytes":16777216,"dep_cache_bytes":33554432,
            "spool":{{"dir":"/spool","size_bytes":1073741824,"reserve_bytes":0}},
            "outputs":{{"textures":{{"key":"{{name}}.png"}}}},
            "inputs":[{{"id":"a","path":"/a"}}]{key}{DIRECTORY}}}"#
        );
        c_hdr(&crate::spec::parse(text.into_bytes()).unwrap().spec)
    };
    assert_eq!(at(1, ""), 0);
    assert_eq!(at(2, ""), 0);
    assert_eq!(at(2, r#","unitycn_key":null"#), 0);
    assert_eq!(at(2, r#","unitycn_key":{"env":"K"}"#), heap_cost(16 << 10));
}

/// Calibration is certified for 4 KiB pages only: any other size, or an
/// unknown one, is uncertified, with its stderr line; 4 KiB keeps the
/// provisional label and adds no line.
#[test]
fn only_4_kib_pages_are_certified() {
    use super::Certification;
    assert_eq!(Certification::of(Some(4096)), Certification::Provisional);
    assert_eq!(Certification::Provisional.note("unityrs batch"), None);
    for page in [Some(16_384), Some(65_536), Some(8192), Some(0), None] {
        let c = Certification::of(page);
        assert_eq!(c, Certification::Uncertified { page_size: page });
        let note = c.note("unityrs batch").unwrap();
        assert!(
            note.starts_with("unityrs batch: memory_bound uncertified: "),
            "{note}"
        );
        assert!(
            note.ends_with(", calibrated for 4096-byte pages only\n"),
            "{note}"
        );
    }
    assert!(
        Certification::of(Some(16_384))
            .note("p")
            .unwrap()
            .contains("16384-byte pages")
    );
    let page = crate::sys::page_size().unwrap();
    assert!(page.is_power_of_two(), "{page}");
}
