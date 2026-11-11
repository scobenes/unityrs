//! The bounds UnityPy does not have (`docs/plans/extract.md` E59, core.md
//! section 3.15): every array charged to the caller's meter before it is
//! allocated and counted as decode work, each refusal the meter's named
//! error with nothing held after it; a gather whose wrapped offsets
//! UnityPyBoost would read outside its buffers refused; and no panic on
//! any value. Each handler case is the oracle fixture's
//! (`tests/fixtures/process.json`), so every path the oracle covers is run
//! at each bound.

use std::fmt::Write;
use std::sync::Arc;

use serde_json::Value as J;
use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::error::LimitKind;
use unityrs_core::limits::{ChargeError, Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::walk::{self, ReadCtx, ReadOptions, RefTypes};
use unityrs_core::{Bytes, Parsed, RawStr};
use unityrs_meshdecode::{
    Gather, IndexBuffer, MeshError, MeshHandler, Processed, Resources, Source, Values, Version,
    VertexArray, unpack_vertexdata,
};
use unityrs_testkit::Rng;

struct Resource(Bytes);

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

/// A fixture case: its value, how the handler is called, and the resource.
struct Case {
    name: String,
    rows: J,
    data: Vec<u8>,
    parsed: Parsed,
    sprite: bool,
    little: bool,
    version: Version,
    resource: Option<Resource>,
}

fn parse(rows: &J, data: Vec<u8>) -> Option<Parsed> {
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
    walk::boost::read(&Bytes::from(data), &tree, &ctx).ok()
}

fn cases() -> Vec<Case> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/process.json");
    let body: J = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let part = |v: &J, i: usize| v[i].as_u64().unwrap();
    body["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let v = &c["version"];
            let reader = c["reader"].as_bool().unwrap_or(false);
            let rows = body["trees"][c["tree"].as_str().unwrap()].clone();
            let data = unhex(c["hex"].as_str().unwrap());
            Case {
                name: c["name"].as_str().unwrap().to_owned(),
                parsed: parse(&rows, data.clone()).unwrap(),
                rows,
                data,
                sprite: c["source"] == "sprite",
                little: c["endian"] == "<",
                version: Version::new(
                    u16::try_from(part(v, 0)).unwrap(),
                    u16::try_from(part(v, 1)).unwrap(),
                    u16::try_from(part(v, 2)).unwrap(),
                    u8::try_from(part(v, 3)).unwrap(),
                    u8::try_from(part(v, 4)).unwrap(),
                ),
                resource: c["resource"]
                    .as_str()
                    .filter(|_| reader)
                    .map(|h| Resource(Bytes::from(unhex(h)))),
            }
        })
        .collect()
}

/// The bytes a value holds: `f64`s and `i64`s at 8 bytes each.
fn held(a: Option<&VertexArray>) -> u64 {
    a.map_or(0, |a| match a.values() {
        Values::Float(v) => 8 * v.len() as u64,
        Values::Int(v) => 8 * v.len() as u64,
        other => panic!("a form no case records: {other:?}"),
    })
}

fn arrays_held(p: &Processed<'_>) -> u64 {
    let mut n = p
        .index_buffer()
        .map_or(0, |b| 4 * b.as_slice().len() as u64);
    for a in [
        p.vertices(),
        p.normals(),
        p.tangents(),
        p.colors(),
        p.bone_weights(),
        p.bone_indices(),
    ] {
        n += held(a);
    }
    (0..8).map(|i| held(p.uv(i))).sum::<u64>() + n
}

/// `process` and `get_triangles` under `meter`, as one comparable answer.
fn answer(c: &Case, meter: &Arc<Meter>) -> Result<String, MeshError> {
    let src = if c.sprite {
        Source::sprite(&c.parsed)
    } else {
        Source::mesh(&c.parsed)
    };
    let resources = c.resource.as_ref().map(|r| r as &dyn Resources);
    let p = MeshHandler::new(src, c.version)
        .little_endian(c.little)
        .process(resources, meter)?;
    // Every array is charged to the meter, at least the bytes it holds.
    assert!(
        meter.used() >= arrays_held(&p),
        "{}: {} charged",
        c.name,
        meter.used()
    );
    let t = match p.get_triangles(meter) {
        Ok(t) => format!(
            "{:?}",
            t.submeshes()
                .iter()
                .map(|s| (s.triangles(), s.cut(), s.empty()))
                .collect::<Vec<_>>()
        ),
        Err(e @ MeshError::Charge(_)) => return Err(e),
        Err(e) => format!("{e:?}"),
    };
    let mut a = format!(
        "{} {} {:?}",
        p.vertex_count(),
        p.use_16bit_indices(),
        p.index_buffer().map(IndexBuffer::as_slice)
    );
    for v in [
        p.vertices(),
        p.normals(),
        p.tangents(),
        p.colors(),
        p.bone_weights(),
        p.bone_indices(),
    ] {
        write!(a, " {:?}", v.map(VertexArray::values)).unwrap();
    }
    for i in 0..8 {
        write!(a, " {:?}", p.uv(i).map(VertexArray::values)).unwrap();
    }
    Ok(a + &t)
}

fn kind(e: &MeshError) -> Option<LimitKind> {
    match e {
        MeshError::Charge(ChargeError::Limit(l)) => Some(l.kind),
        _ => None,
    }
}

/// An answer whole: the outputs, or the error's `Debug` form, so an error
/// other than the expected refusal is compared too.
fn whole(a: &Result<String, MeshError>) -> String {
    match a {
        Ok(s) => s.clone(),
        Err(e) => format!("error {e:?}"),
    }
}

#[test]
fn under_a_smaller_meter_each_case_answers_as_before_or_the_meter_refuses() {
    let cases = cases();
    let (mut refused, mut same) = (0, 0);
    for c in &cases {
        let full = answer(c, &Meter::unlimited());
        assert!(!matches!(full, Err(MeshError::Charge(_))), "{}", c.name);
        for limit in [0, 64, 1 << 10, 16 << 10, 256 << 10] {
            let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
            match answer(c, &meter) {
                Err(e) if kind(&e) == Some(LimitKind::InputMemory) => refused += 1,
                got => {
                    assert_eq!(whole(&got), whole(&full), "{} at {limit}", c.name);
                    same += 1;
                }
            }
            assert_eq!(meter.used(), 0, "{}: {limit} bytes still held", c.name);
        }
    }
    assert!(
        refused > 100 && same > 100,
        "refused {refused}, same {same}"
    );
}

#[test]
fn past_the_work_bound_each_case_is_refused_and_within_it_answers_as_before() {
    let cases = cases();
    let mut refused = 0;
    for c in &cases {
        let full = answer(c, &Meter::unlimited());
        for work in [0, 16, 1 << 10, 64 << 10] {
            let meter =
                Meter::with_decode_work(u64::MAX, work, Arc::new(Unbounded), Reservation::Input);
            match answer(c, &meter) {
                Err(e) if kind(&e) == Some(LimitKind::DecodeWork) => refused += 1,
                got => assert_eq!(whole(&got), whole(&full), "{} at {work}", c.name),
            }
            assert_eq!(meter.used(), 0);
        }
    }
    assert!(refused > 100, "refused {refused}");
}

#[test]
fn a_gather_of_no_bytes_runs_no_loop_and_one_past_the_meter_allocates_nothing() {
    // 2^32 - 1 vertices of 15 components of 0 bytes: UnityPyBoost's loop
    // copies nothing 64 billion times; here it returns at once.
    let meter = Meter::unlimited();
    let none = Gather {
        component_byte_size: 0,
        vertex_count: u32::MAX,
        stream_offset: 0,
        stream_stride: 0,
        channel_offset: 0,
        channel_dimension: 15,
        swap: true,
    };
    assert_eq!(
        unpack_vertexdata(&[], &none, &meter).unwrap().as_slice(),
        b""
    );
    // 1 MiB of output under a meter of 64 KiB: refused before it is made.
    let data = vec![0u8; 1 << 20];
    let big = Gather {
        component_byte_size: 4,
        vertex_count: 1 << 16,
        stream_stride: 16,
        channel_dimension: 4,
        ..none
    };
    let small = Meter::new(64 << 10, Arc::new(Unbounded), Reservation::Input);
    let e = unpack_vertexdata(&data, &big, &small).unwrap_err();
    assert_eq!(kind(&e), Some(LimitKind::InputMemory));
    assert_eq!(small.used(), 0);
    let ok = unpack_vertexdata(&data, &big, &meter).unwrap();
    assert_eq!(ok.as_slice().len(), 1 << 20);
}

#[test]
fn a_gather_past_its_buffers_is_refused_where_unitypyboost_would_read_outside_them() {
    let meter = Meter::unlimited();
    // The bounds check passes on the wrapped sum ((2 - 1) x (2^32 - 4) + 4
    // + 4 is 4, mod 2^32) though the second vertex starts 2^32 - 4 bytes on.
    let g = Gather {
        component_byte_size: 4,
        vertex_count: 2,
        stream_offset: 0,
        stream_stride: u32::MAX - 3,
        channel_offset: 0,
        channel_dimension: 2,
        swap: false,
    };
    assert!(matches!(
        unpack_vertexdata(&[0; 8], &g, &meter),
        Err(MeshError::WrappedAccess)
    ));
    // (vertex count - 1) x stride alone reaches 2^32: 65536 x 65536 + 4 is
    // 4, mod 2^32, so 65537 vertices of a stride of 65536 pass the check
    // over 4 bytes of data, and the second vertex is 64 KiB past them.
    let g = Gather {
        component_byte_size: 4,
        vertex_count: 65537,
        stream_offset: 0,
        stream_stride: 65536,
        channel_offset: 0,
        channel_dimension: 1,
        swap: true,
    };
    assert!(matches!(
        unpack_vertexdata(&[0; 4], &g, &meter),
        Err(MeshError::WrappedAccess)
    ));
    // The output's length wraps (2^31 x 2 x 1 bytes is 2^32, 0 in u32).
    let g = Gather {
        component_byte_size: 1,
        vertex_count: 1 << 31,
        stream_stride: 0,
        channel_dimension: 2,
        ..g
    };
    assert!(matches!(
        unpack_vertexdata(&[0; 2], &g, &meter),
        Err(MeshError::WrappedAccess)
    ));
    assert_eq!(meter.used(), 0);
}

#[test]
fn no_value_panics_the_handler() {
    // Each fixture value with bytes flipped, cut and grown, under each
    // version the fixtures use, as a mesh and as a sprite.
    let cases = cases();
    let versions: Vec<Version> = cases.iter().map(|c| c.version).collect();
    let mut rng = Rng::new(0x6d65_7368);
    for c in &cases {
        for _ in 0..8 {
            let mut bytes = c.data.clone();
            let flips = 1 + rng.below(4);
            rng.flip_bits(&mut bytes, flips);
            match rng.below(3) {
                0 => rng.truncate(&mut bytes),
                1 => {
                    let grow = rng.below(64);
                    bytes.extend((0..grow).map(|_| u8::try_from(rng.below(256)).unwrap()));
                }
                _ => {}
            }
            let Some(parsed) = parse(&c.rows, bytes) else {
                continue;
            };
            let v = versions[rng.below(versions.len())];
            for src in [Source::mesh(&parsed), Source::sprite(&parsed)] {
                // A meter of its own each run: work is never given back, so
                // a shared one, once spent, would refuse every later run at
                // its first count.
                let meter = Meter::with_decode_work(
                    64 << 20,
                    16 << 20,
                    Arc::new(Unbounded),
                    Reservation::Input,
                );
                let resources = c.resource.as_ref().map(|r| r as &dyn Resources);
                if let Ok(p) = MeshHandler::new(src, v)
                    .little_endian(rng.below(2) == 0)
                    .process(resources, &meter)
                {
                    let _ = p.get_triangles(&meter);
                }
                assert_eq!(meter.used(), 0);
            }
        }
    }
}
