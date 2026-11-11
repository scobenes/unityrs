//! The handler pays before it allocates (`docs/plans/extract.md` E59):
//! every allocation its input sizes is charged to the caller's meter
//! first, so at no allocation during `process` and `get_triangles` does
//! the heap stand higher above where it started than the charges then held:
//! not by a byte, but for a `Malformed` refusal's message (a fixed slack).
//! Each handler case is the oracle fixture's (`tests/fixtures/process.json`),
//! among them the derived streams of a channel naming stream 255 (256
//! streams) and its `big_*` cases, an index buffer, a list's, a strip's and
//! quads' triangles, a skin and its vertices' positions of 4 KiB or more
//! each; and each gather of `tests/fixtures/unpack.json` on its own. Besides
//! them, a node type of 64 KiB, which a refusal's message names by at most
//! 64 bytes.
//!
//! A counting global allocator watches the heap of the test's own thread
//! (`unityrs_testkit::counted`): libtest's main thread allocates beside it.
//! The meter books its charges with a budget that keeps them in `HELD`, so
//! the allocator compares the two at each allocation, as imageio's
//! `tests/charges.rs` does. One test runs every measure in turn, as they
//! share the counts.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use serde_json::Value as J;
use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation};
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::walk::{self, ReadCtx, ReadOptions, RefTypes};
use unityrs_core::{Bytes, Parsed, RawStr};
use unityrs_meshdecode::{
    Gather, MeshError, MeshHandler, Resources, Source, Version, unpack_vertexdata,
};
use unityrs_testkit::counted::{Allocator, Tally, count_this_thread, others_are_not_counted};

struct Watching;

/// Bytes live on the counted thread.
static LIVE: AtomicUsize = AtomicUsize::new(0);
/// [`LIVE`] when the measure began.
static BASE: AtomicUsize = AtomicUsize::new(0);
/// How far the heap above [`BASE`] rose during the measure.
static PEAK: AtomicUsize = AtomicUsize::new(0);
/// Bytes the meter's charges hold: booked by [`Booked`], given back by
/// [`Home`].
static HELD: AtomicU64 = AtomicU64::new(0);
/// How far the heap above [`BASE`] ever stood past [`HELD`].
static OVER: AtomicU64 = AtomicU64::new(0);

impl Tally for Watching {
    fn grew(&self, size: usize, _zeroed: bool) {
        let now = LIVE.fetch_add(size, Ordering::Relaxed) + size;
        let above = now.saturating_sub(BASE.load(Ordering::Relaxed));
        PEAK.fetch_max(above, Ordering::Relaxed);
        let above = u64::try_from(above).unwrap_or(u64::MAX);
        let over = above.saturating_sub(HELD.load(Ordering::Relaxed));
        OVER.fetch_max(over, Ordering::Relaxed);
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(size, Ordering::Relaxed);
    }
}

#[global_allocator]
static ALLOC: Allocator<Watching> = Allocator(Watching);

#[derive(Debug)]
struct Home;

impl PermitHome for Home {
    fn release(&self, bytes: u64) {
        HELD.fetch_sub(bytes, Ordering::Relaxed);
    }
}

/// A budget that keeps what it grants in [`HELD`]; its home is made before
/// a measure, so a reservation allocates nothing.
#[derive(Debug)]
struct Booked(Arc<Home>);

impl MemoryBudget for Booked {
    fn try_reserve(&self, bytes: u64, _what: Reservation) -> Result<Permit, BudgetExceeded> {
        HELD.fetch_add(bytes, Ordering::Relaxed);
        Ok(Permit::new(bytes, self.0.clone()))
    }
}

/// What the heap may stand past the charges in a run refused as
/// `Malformed`: its message (at most 64 bytes of a type name, and the
/// field's). Any other run gets none.
const SLACK: u64 = 1024;

/// The slack a run that ended in `failed` gets ([`SLACK`]).
fn slack(failed: Option<&str>) -> u64 {
    if failed.is_some_and(|f| f.starts_with("Malformed(")) {
        SLACK
    } else {
        0
    }
}

/// A case's resource bytes. `Bytes` from a `Vec` boxes a shared header on
/// its first clone, the caller's own allocation, not the handler's: made
/// here, before any measure.
struct Resource(Bytes);

impl Resource {
    fn new(data: Vec<u8>) -> Self {
        let bytes = Bytes::from(data);
        drop(bytes.clone());
        Self(bytes)
    }
}

impl Resources for Resource {
    fn resource(&self, _: &RawStr, _: u64, _: u64) -> Result<Bytes, unityrs_core::Error> {
        Ok(self.0.clone())
    }
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn rows_of(rows: &J) -> Vec<NodeRow> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let level = i32::try_from(r[0].as_i64().unwrap()).unwrap();
            NodeRow::new(level, r[1].as_str().unwrap(), r[2].as_str().unwrap())
                .with_meta_flag(r[3].as_i64().unwrap())
        })
        .collect()
}

fn parse(rows: Vec<NodeRow>, data: Vec<u8>) -> Parsed {
    let tree = Tree::compile(
        Arc::new(NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap();
    let options = ReadOptions::default();
    let meter = Meter::unlimited();
    let ctx = ReadCtx {
        endian: Endian::Little,
        ref_types: RefTypes::List(&[]),
        options: &options,
        meter: &meter,
    };
    walk::boost::read(&Bytes::from(data), &tree, &ctx).unwrap()
}

/// A fixture case, parsed, with how the handler is called.
struct Case {
    name: String,
    parsed: Parsed,
    sprite: bool,
    little: bool,
    version: Version,
    resource: Option<Resource>,
}

fn version(v: &J) -> Version {
    let part = |i: usize| v[i].as_u64().unwrap();
    Version::new(
        u16::try_from(part(0)).unwrap(),
        u16::try_from(part(1)).unwrap(),
        u16::try_from(part(2)).unwrap(),
        u8::try_from(part(3)).unwrap(),
        u8::try_from(part(4)).unwrap(),
    )
}

/// The fixture's cases, each tree passed through `edit` first.
fn cases(edit: impl Fn(&str, Vec<NodeRow>) -> Vec<NodeRow>) -> Vec<Case> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/process.json");
    let body: J = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    body["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let name = c["name"].as_str().unwrap().to_owned();
            let rows = edit(&name, rows_of(&body["trees"][c["tree"].as_str().unwrap()]));
            let reader = c["reader"].as_bool().unwrap_or(false);
            Case {
                parsed: parse(rows, unhex(c["hex"].as_str().unwrap())),
                name,
                sprite: c["source"] == "sprite",
                little: c["endian"] == "<",
                version: version(&c["version"]),
                resource: c["resource"]
                    .as_str()
                    .filter(|_| reader)
                    .map(|h| Resource::new(unhex(h))),
            }
        })
        .collect()
}

/// `run` on a fresh meter of no bound, booked in [`HELD`]: how far the heap
/// ever stood past the charges, how far it rose, what the meter granted,
/// and the error `run` returns, as its `Debug` form (made after the
/// measure).
fn watch(
    name: &str,
    run: impl FnOnce(&Arc<Meter>) -> Option<MeshError>,
) -> (u64, u64, u64, Option<String>) {
    let meter = Meter::new(
        u64::MAX,
        Arc::new(Booked(Arc::new(Home))),
        Reservation::Input,
    );
    assert_eq!(HELD.load(Ordering::Relaxed), 0);
    BASE.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    PEAK.store(0, Ordering::Relaxed);
    OVER.store(0, Ordering::Relaxed);
    let failed = run(&meter);
    let over = OVER.load(Ordering::Relaxed);
    let grew = u64::try_from(PEAK.load(Ordering::Relaxed)).unwrap();
    assert_eq!(meter.used(), 0, "{name}: a charge held after its value");
    assert_eq!(HELD.load(Ordering::Relaxed), 0, "{name}: a permit held");
    (
        over,
        grew,
        meter.granted(),
        failed.map(|e| format!("{e:?}")),
    )
}

/// `process`, `get_triangles` and `process` again ([`watch`]); the error is
/// the first `process`'s.
fn measure(c: &Case) -> (u64, u64, u64, Option<String>) {
    let src = if c.sprite {
        Source::sprite(&c.parsed)
    } else {
        Source::mesh(&c.parsed)
    };
    let handler = MeshHandler::new(src, c.version).little_endian(c.little);
    let resources = c.resource.as_ref().map(|r| r as &dyn Resources);
    watch(&c.name, |meter| match handler.process(resources, meter) {
        Ok(p) => {
            drop(p.get_triangles(meter));
            drop(p);
            drop(handler.process(resources, meter));
            None
        }
        Err(e) => Some(e),
    })
}

fn uint<T: TryFrom<u64>>(v: &J) -> T
where
    T::Error: std::fmt::Debug,
{
    T::try_from(v.as_u64().unwrap()).unwrap()
}

/// The first node of type `ty` given a type of 64 KiB of bytes that escape
/// to four each.
fn long_type(mut rows: Vec<NodeRow>, ty: &str) -> Vec<NodeRow> {
    let i = rows
        .iter()
        .position(|r| r.ty.as_bytes() == ty.as_bytes())
        .unwrap();
    rows[i].ty = RawStr::from("\u{7f}".repeat(64 << 10).as_str());
    rows
}

#[test]
fn every_allocation_the_input_sizes_is_charged_first() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));

    // Every case: the heap never stands past the charges held.
    let all = cases(|_, rows| rows);
    let mut charged = 0;
    for c in &all {
        let (over, grew, granted, failed) = measure(c);
        assert!(
            over <= slack(failed.as_deref()),
            "{}: the heap stood {over} bytes past the charges (rose {grew}, {granted} granted)",
            c.name
        );
        charged += u64::from(granted > 0);
    }
    assert!(charged > 200, "{charged} cases charged anything");
    // Each of the big cases' arrays is past the slack, so an uncharged one
    // would show.
    for name in ["big_list", "big_strip", "big_quads", "big_skin"] {
        let c = all.iter().find(|c| c.name == name).unwrap();
        let (over, grew, _, failed) = measure(c);
        assert_eq!((over, failed), (0, None), "{name}");
        assert!(grew > 4 * SLACK, "{name}: rose {grew}");
    }

    // Each gather of the oracle's on its own.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/unpack.json");
    let body: J = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for (i, c) in body["cases"].as_array().unwrap().iter().enumerate() {
        let data = unhex(c["data"].as_str().unwrap());
        let args = Gather {
            component_byte_size: uint(&c["cbs"]),
            vertex_count: uint(&c["vc"]),
            stream_offset: uint(&c["soff"]),
            stream_stride: uint(&c["stride"]),
            channel_offset: uint(&c["choff"]),
            channel_dimension: uint(&c["dim"]),
            swap: c["swap"].as_bool().unwrap(),
        };
        let (over, grew, granted, _) = watch(&format!("gather {i}"), |meter| {
            unpack_vertexdata(&data, &args, meter).err()
        });
        assert_eq!(over, 0, "gather {i}: rose {grew}, {granted} granted");
    }

    // 256 derived streams of 32 bytes each, from one channel of stream 255
    // and one vertex: charged before they are allocated.
    let c = all.iter().find(|c| c.name == "streams_255").unwrap();
    let (over, grew, granted, failed) = measure(c);
    assert_eq!(failed, None);
    assert!(
        grew >= 256 * 32 && granted >= 2 * 256 * 32,
        "rose {grew}, {granted} granted"
    );
    assert!(over <= SLACK, "{over} past the charges");

    // A Mesh node, and a ChannelInfo node, of a type 64 KiB long: refused,
    // with a message of at most 64 bytes of it.
    let edited = cases(|name, rows| match name {
        "format_2019.4.0f1_0_d3" => long_type(rows, "Mesh"),
        "streams_three" => long_type(rows, "ChannelInfo"),
        _ => rows,
    });
    for name in ["format_2019.4.0f1_0_d3", "streams_three"] {
        let c = edited.iter().find(|c| c.name == name).unwrap();
        let (over, grew, _, failed) = measure(c);
        let failed = failed.unwrap();
        assert!(failed.starts_with("Malformed("), "{name}: {failed}");
        assert!(
            failed.contains("…") && failed.len() < 200,
            "{name}: {failed}"
        );
        assert!(
            grew <= SLACK && over <= SLACK,
            "{name}: the refusal allocated {grew} bytes"
        );
    }
}
