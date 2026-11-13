//! What the compressed mesh's divergences and refusals change, against the
//! oracle's fixture (`tests/fixtures/process.json`, the cases of
//! `tools/fixtures/meshdecode_compressed.py`): `PARITY.md` section 19.1's
//! two Tier C rows, each UnityPy's value pinned and the correct one
//! asserted; section 15.3's division first in `unpack_floats`; and its
//! refusal of an item past 32 bits. `tests/oracle.rs` replays every case
//! (each Tier C case against the corrected answer the script records, its
//! expected-divergence mask `DIVERGES`).

use std::sync::Arc;

use serde_json::{Value as J, json};
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::walk::{self, ReadCtx, ReadOptions, RefTypes};
use unityrs_core::{Bytes, Parsed};
use unityrs_meshdecode::{
    Component, MeshError, MeshHandler, Processed, Source, Values, Version, VertexArray,
};

fn body() -> J {
    serde_json::from_str(include_str!("fixtures/process.json")).unwrap()
}

fn case<'a>(body: &'a J, name: &str) -> &'a J {
    body["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no case {name}"))
}

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

/// The case `name` through the crate's handler, handed to `f`.
fn ours<R>(body: &J, name: &str, f: impl FnOnce(Result<Processed<'_>, MeshError>) -> R) -> R {
    let c = case(body, name);
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
    f(MeshHandler::new(Source::mesh(&parsed), version).process(None, &Meter::unlimited()))
}

/// A float as the fixture writes it, its bits.
fn bits(x: &J) -> f64 {
    f64::from_bits(u64::from_str_radix(x["f"].as_str().unwrap(), 16).unwrap())
}

/// UnityPy's array `name` of the case's recorded `process`, each a tuple of
/// `f64`s.
fn theirs(body: &J, case_name: &str, name: &str) -> Vec<Vec<f64>> {
    case(body, case_name)["process"]["ok"]["attrs"][name]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t.as_array().unwrap().iter().map(bits).collect())
        .collect()
}

/// The crate's array of floats, a tuple each.
fn floats(a: &VertexArray) -> Vec<Vec<f64>> {
    let Values::Float(v) = a.values() else {
        panic!("not floats: {:?}", a.values())
    };
    v.chunks(a.dimension()).map(<[f64]>::to_vec).collect()
}

/// Items `first..` of a vector whose item k is `k + start`, `dim` a tuple,
/// two vertices.
fn items(first: u32, dim: u32, start: f64) -> Vec<Vec<f64>> {
    (0..2)
        .map(|v| {
            (0..dim)
                .map(|d| f64::from(first + v * dim + d) + start)
                .collect()
        })
        .collect()
}

/// `PARITY.md` section 19.1's UV offset (Tier C): UnityPy sets the offset
/// to the channel before's items (`uvSrcOffset = uvDim * m_VertexCount`),
/// so a third channel reads from where the second did; the crate adds.
#[test]
fn the_uv_offset_adds_where_unitypy_assigns() {
    let body = body();
    // Three channels of two components over items 10 to 21, two vertices.
    assert_eq!(
        theirs(&body, "compressed_values", "m_UV1"),
        items(4, 2, 10.0)
    );
    let third = theirs(&body, "compressed_values", "m_UV2");
    assert_eq!(
        third,
        items(4, 2, 10.0),
        "UnityPy's third channel is its second"
    );
    ours(&body, "compressed_values", |p| {
        let p = p.unwrap();
        assert_eq!(floats(p.uv(1).unwrap()), items(4, 2, 10.0));
        assert_eq!(floats(p.uv(2).unwrap()), items(8, 2, 10.0));
    });
    // Channels 0, 2, 5 and 7 of 1, 3, 4 and 2 components, item k = k - 2:
    // UnityPy's 2 is right, its 5 and 7 start where 2 and 5 did.
    let name = "compressed_uv_dims";
    assert_eq!(theirs(&body, name, "m_UV2"), items(2, 3, -2.0));
    assert_eq!(theirs(&body, name, "m_UV5"), items(6, 4, -2.0));
    assert_eq!(theirs(&body, name, "m_UV7"), items(8, 2, -2.0));
    ours(&body, name, |p| {
        let p = p.unwrap();
        assert_eq!(floats(p.uv(2).unwrap()), items(2, 3, -2.0));
        assert_eq!(floats(p.uv(5).unwrap()), items(8, 4, -2.0));
        assert_eq!(floats(p.uv(7).unwrap()), items(16, 2, -2.0));
    });
    // Ten items: UnityPy's third channel reads items 4 to 7; the crate's
    // runs past the vector.
    assert_eq!(
        theirs(&body, "compressed_uv_short", "m_UV2"),
        items(4, 2, 0.0)
    );
    ours(&body, "compressed_uv_short", |p| {
        let e = p.unwrap_err();
        let want = ("IndexError", "list index out of range".to_owned());
        assert_eq!(e.python(), Some(want));
    });
}

/// `PARITY.md` section 19.1's fourth bone weight (Tier C): UnityPy gives
/// `1 - sum`, `sum` in 31sts, an `int`; the crate `(31 - sum) / 31`.
#[test]
fn the_fourth_bone_weight_is_in_31sts_where_unitypy_subtracts_from_1() {
    let body = body();
    let float = |x: f64| json!({ "f": format!("{:016x}", x.to_bits()) });
    let unitypy = |name: &str, v: usize| {
        case(&body, name)["process"]["ok"]["attrs"]["m_BoneWeights"][v].clone()
    };
    let fourth = |p: &Processed<'_>, v: usize| p.bone_weights().unwrap().component(v, 3).unwrap();
    // Weights 10, 10 and 10: UnityPy's fourth is 1 - 30.
    let tenth = float(10.0 / 31.0);
    assert_eq!(
        unitypy("compressed_values", 1),
        json!([tenth, tenth, tenth, -29])
    );
    ours(&body, "compressed_values", |p| {
        let p = p.unwrap();
        assert_eq!(fourth(&p, 1), Component::Float(1.0 / 31.0));
        assert_eq!(
            p.bone_weights().unwrap().component(1, 0),
            Some(Component::Float(10.0 / 31.0))
        );
    });
    // Weights 10, 10, 5, then 0, 0, 0: UnityPy's 1 - 25 and 1 - 0.
    let name = "compressed_skin_weights";
    assert_eq!(unitypy(name, 2)[3], json!(-24));
    assert_eq!(
        unitypy(name, 3),
        json!([float(0.0), float(0.0), float(0.0), 1])
    );
    ours(&body, name, |p| {
        let p = p.unwrap();
        assert_eq!(fourth(&p, 2), Component::Float(6.0 / 31.0));
        assert_eq!(fourth(&p, 3), Component::Float(1.0));
    });
}

/// `PARITY.md` section 15.3's `unpack_floats` row: `x * (range / (2^bits -
/// 1)) + start`, the division first, which the oracle's answer for 30
/// items of 24 bits shows (`tests/oracle.rs` compares every bit);
/// multiplying first would round otherwise for some of them.
#[test]
fn unpack_floats_divides_first() {
    let body = body();
    let recorded = theirs(&body, "compressed_divides_first", "m_Vertices");
    let range = f64::from(f32::from_bits(0x4215_3333));
    let start = f64::from(f32::from_bits(0xC13B_3333));
    let steps = f64::from((1u32 << 24) - 1);
    let scale = range / steps;
    ours(&body, "compressed_divides_first", |p| {
        assert_eq!(floats(p.unwrap().vertices().unwrap()), recorded);
    });
    let mut differ = 0;
    for &v in recorded.iter().flatten() {
        let q = ((v - start) / scale).round();
        assert_eq!((q * scale + start).to_bits(), v.to_bits());
        differ += usize::from((q * range / steps + start).to_bits() != v.to_bits());
    }
    assert_eq!(recorded.len(), 10);
    assert!(differ > 0, "no item tells the two orders apart");
}

/// `PARITY.md` section 15.3's `unpack_ints` row: an item past 32 bits,
/// which UnityPy unpacks (its answer, pinned) and the crate refuses.
#[test]
fn a_packed_item_past_32_bits_is_refused() {
    let body = body();
    let unitypy = &case(&body, "compressed_ints_33")["process"];
    assert_eq!(
        unitypy["ok"]["index_buffer"],
        json!([1, (1u64 << 33) - 1, 5])
    );
    ours(&body, "compressed_ints_33", |p| {
        let e = p.unwrap_err();
        assert_eq!(
            format!("{e:?}"),
            "Malformed(\"a packed item wider than 32 bits\")"
        );
    });
}
