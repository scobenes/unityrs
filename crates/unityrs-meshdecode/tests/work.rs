//! The handler's traversal is counted as decode work (`docs/plans/core.md`
//! section 3.15, `docs/plans/extract.md` E59): what `process` and
//! `get_triangles` count grows with the channels they visit (each one of
//! dimension 0 skipped included), the submeshes (each of no indices
//! included) and the fields of the classes they read, by at least one unit
//! each, and in proportion. The cases are the oracle fixture's `scale_*`
//! ones (`tools/fixtures/meshdecode_cases.py`'s `scale_cases`): N of 4, 32
//! and 256 each, the rest of each case the same; and N elements of M fields
//! more each (`element_field_cases`), N and M of 4 and 32, which part each
//! count plays: an element's unit, the fields its reads scan, and its
//! class's build; and arrays of N (`array_scale_cases`), N of 4, 32 and 256,
//! whose loops' counts grow by exactly their documented units, as do a
//! compressed mesh's (`tools/fixtures/meshdecode_compressed.py`'s
//! `scale_cases`).

use std::sync::Arc;

use serde_json::Value as J;
use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::walk::{self, ReadCtx, ReadOptions, RefTypes};
use unityrs_core::{Bytes, Parsed};
use unityrs_meshdecode::{Gather, MeshHandler, Source, Version, unpack_vertexdata};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn parse(rows: &J, data: Vec<u8>) -> Parsed {
    let rows = rows
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let level = i32::try_from(r[0].as_i64().unwrap()).unwrap();
            NodeRow::new(level, r[1].as_str().unwrap(), r[2].as_str().unwrap())
                .with_meta_flag(r[3].as_i64().unwrap())
        })
        .collect();
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

/// The work `process` and `get_triangles` count on case `name`, both of
/// which succeed.
fn work(body: &J, name: &str) -> u64 {
    let c = body["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no case {name}"));
    assert_eq!(c["source"], "mesh");
    let parsed = parse(
        &body["trees"][c["tree"].as_str().unwrap()],
        unhex(c["hex"].as_str().unwrap()),
    );
    let v = &c["version"];
    let part = |i: usize| v[i].as_u64().unwrap();
    let version = Version::new(
        u16::try_from(part(0)).unwrap(),
        u16::try_from(part(1)).unwrap(),
        u16::try_from(part(2)).unwrap(),
        u8::try_from(part(3)).unwrap(),
        u8::try_from(part(4)).unwrap(),
    );
    let meter = Meter::unlimited();
    let p = MeshHandler::new(Source::mesh(&parsed), version)
        .process(None, &meter)
        .unwrap();
    p.get_triangles(&meter).unwrap();
    let done = meter.decoded();
    // Exactly that much: a bound one unit short refuses it.
    let short =
        Meter::with_decode_work(u64::MAX, done - 1, Arc::new(Unbounded), Reservation::Input);
    let refused = MeshHandler::new(Source::mesh(&parsed), version)
        .process(None, &short)
        .and_then(|p| p.get_triangles(&short).map(drop));
    assert!(refused.is_err(), "{name}: {done} - 1 of work sufficed");
    done
}

#[test]
fn the_work_grows_with_the_channels_submeshes_and_fields_read() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/process.json");
    let body: J = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    for kind in ["channels", "submeshes", "fields"] {
        let [w4, w32, w256] = [4, 32, 256].map(|n| work(&body, &format!("scale_{kind}_{n}")));
        // At least a unit each,
        assert!(
            w32 - w4 >= 28 && w256 - w32 >= 224,
            "{kind}: {w4} {w32} {w256}"
        );
        // and the same for each: the work is affine in N.
        assert_eq!(
            (w256 - w32) * 28,
            (w32 - w4) * 224,
            "{kind}: {w4} {w32} {w256}"
        );
    }
}

/// How many children the node at `path` has in case `name`'s tree.
fn children(body: &J, name: &str, path: &[&str]) -> u64 {
    let c = body["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap();
    let rows = body["trees"][c["tree"].as_str().unwrap()]
        .as_array()
        .unwrap();
    let level = |r: &J| r[0].as_i64().unwrap();
    let (mut at, mut depth) = (0, 0);
    for step in path {
        at = (at + 1..rows.len())
            .take_while(|&i| level(&rows[i]) > depth)
            .find(|&i| level(&rows[i]) == depth + 1 && rows[i][2] == *step)
            .unwrap_or_else(|| panic!("{name}: no {step}"));
        depth += 1;
    }
    let n = (at + 1..rows.len())
        .take_while(|&i| level(&rows[i]) > depth)
        .filter(|&i| level(&rows[i]) == depth + 1)
        .count();
    u64::try_from(n).unwrap()
}

/// N elements of `b` + M fields each: the work is `C + s·M + N·(t + a·(b +
/// M))`, and each part is one count's.
/// - `t`, an element's own unit, is 1 (`vertex.rs`'s a unit a channel,
///   `triangles.rs`'s a unit a submesh), whatever its fields.
/// - `a` is two units a field for each field the element reads, its
///   children and its dict scanned (`view.rs`'s `get`): a skipped channel
///   reads `dimension`, a submesh of no indices `topology`, `firstByte` and
///   `indexCount`.
/// - `s`, a field's part apart from N, is its class's build (`view.rs`'s
///   `builds`): each child against each required field and each field of
///   the slotted class, once a list, its elements one node.
///
/// M = 0 is the `scale_*` case of N elements.
#[test]
fn each_element_counts_a_unit_besides_its_fields_and_its_class_s_build() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/process.json");
    let body: J = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    // (kind, the element's path, the fields it reads, its class's required
    // fields and all its fields).
    let kinds: [(&str, &[&str], u64, u64, u64); 2] = [
        (
            "channels",
            &["m_VertexData", "m_Channels", "Array", "data"],
            1,
            4,
            4,
        ),
        ("submeshes", &["m_SubMeshes", "Array", "data"], 3, 5, 9),
    ];
    for (kind, at, reads, required, fields) in kinds {
        let work_at = |elements: u64, more: u64| match more {
            0 => work(&body, &format!("scale_{kind}_{elements}")),
            _ => work(&body, &format!("scale_{kind}_{elements}_fields_{more}")),
        };
        let own = children(&body, &format!("scale_{kind}_4"), at);
        assert_eq!(
            children(&body, &format!("scale_{kind}_4_fields_32"), at),
            own + 32
        );
        let more = [0, 4, 32];
        let [few, many] = [4, 32].map(|elements| more.map(|m| work_at(elements, m)));
        let all = format!("{kind}: N 4 {few:?}, N 32 {many:?} for M {more:?}");
        // At least a unit an element and field,
        for (elements, row) in [(4, few), (32, many)] {
            for (i, m) in more.iter().enumerate() {
                assert!(row[i] - row[0] >= elements * m, "{all}");
            }
        }
        // the part of each element, D(M), the same for each,
        let element = [0, 1, 2].map(|i| {
            assert_eq!((many[i] - few[i]) % 28, 0, "{all}");
            (many[i] - few[i]) / 28
        });
        let per_field = (element[2] - element[1]) / 28;
        assert_eq!(
            (element[2] - element[1], element[1] - element[0]),
            (28 * per_field, 4 * per_field),
            "{all}: D {element:?}"
        );
        assert_eq!(per_field, 2 * reads, "{all}: two units a field, a read");
        assert_eq!(
            element[0] - per_field * own,
            1,
            "{all}: an element's own unit"
        );
        // and the rest, a field's part apart from the elements.
        let rest = [0, 1, 2].map(|i| few[i] - 4 * element[i]);
        let build = (rest[2] - rest[1]) / 28;
        assert_eq!(
            (rest[2] - rest[1], rest[1] - rest[0]),
            (28 * build, 4 * build),
            "{all}: C {rest:?}"
        );
        assert_eq!(build, required + fields, "{all}: the class's build");
    }
}

/// An array of N, for N of 4, 32 and 256 (`array_scale_cases`): the work
/// grows by exactly the units its loops count, each loop's count before it
/// runs (`docs/plans/extract.md` E59):
/// - an index buffer of N 16-bit indices: 2 an index byte
///   (`unpack_indices`), under a list (12 a triangle), a strip (12 a step,
///   the dropped degenerates included) and quads (12 a triangle);
/// - an index buffer of N under one strip whose every step is degenerate: 2
///   an index byte and 12 a step, though the strip keeps none;
/// - N bones below 2018.2: 1 a value, 8 a bone (its four indices and four
///   weights, `copy_skin`), besides its eight reads' scans of its eight
///   fields, two units a field a read;
/// - one channel of N vertices: 1 a byte of its components, of floats and
///   of ints (`unpack_channel`'s two loops);
/// - N channels of dimension 0 from 5 on: 1 a channel in `get_streams` and 1
///   in `read_vertex_data`, besides its four reads' scans of its four fields;
/// - `unpack_vertexdata` of N vertices: 1 an output byte, and nothing else.
#[test]
fn each_array_counts_its_units_exactly() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/process.json");
    let body: J = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let grows = |kind: &str, units: &dyn Fn(u64) -> u64| {
        let [few, more, most] = [4, 32, 256].map(|n| work(&body, &format!("scale_{kind}_{n}")));
        assert_eq!(
            (more - few, most - more),
            (units(32) - units(4), units(256) - units(32)),
            "{kind}: {few} {more} {most}"
        );
    };
    grows("indices", &|n| {
        2 * (2 * n) + 12 * (n / 3) + 12 * (n - 2) + 12 * (2 * (n / 4))
    });
    grows("strip_degenerate", &|n| 2 * (2 * n) + 12 * (n - 2));
    grows("bones", &|n| n * (2 * 4 + 8 * 2 * 8));
    grows("vertices_float", &|n| n * 3 * 4);
    grows("vertices_int", &|n| n * 4);
    grows("channels_2019", &|n| n * (1 + 1 + CHANNEL_READS * 2 * 4));
    // A compressed mesh of N vertices' positions, N normals and N signs: a
    // unit an item unpacked and a unit a float made of one (3N positions,
    // 2N normal components), a unit a sign, a unit a zero of the 3N, and a
    // unit a normal.
    let [few, more, most] = [4, 32, 256].map(|n| work(&body, &format!("compressed_scale_{n}")));
    assert_eq!(
        (more - few, most - more),
        (15 * 28, 15 * 224),
        "compressed: {few} {more} {most}"
    );
    for n in [4u32, 32, 256] {
        let data = vec![0u8; 12 * n as usize];
        let args = Gather {
            component_byte_size: 4,
            vertex_count: n,
            stream_offset: 0,
            stream_stride: 12,
            channel_offset: 0,
            channel_dimension: 3,
            swap: false,
        };
        let meter = Meter::unlimited();
        let out = unpack_vertexdata(&data, &args, &meter).unwrap();
        assert_eq!(out.as_slice().len(), 12 * n as usize);
        assert_eq!(meter.decoded(), 12 * u64::from(n), "the gather of {n}");
    }
}

/// The reads of a channel of dimension 0 from 5 on: `get_streams` reads
/// `stream` in its first loop, and `stream` and `dimension` in its second
/// (`format` only past dimension 0); `read_vertex_data` reads `dimension`.
const CHANNEL_READS: u64 = 4;
