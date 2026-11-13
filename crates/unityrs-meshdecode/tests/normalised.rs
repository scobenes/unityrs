//! What `PARITY.md` section 19.1's UNorm/SNorm row changes, against the
//! oracle's fixture (`tests/fixtures/process.json`, the `normalised_` cases
//! of `tools/fixtures/meshdecode_cases.py`): for each normalised format of
//! each table, in a Mesh's positions, colour channel and UV0, UnityPy's
//! integers pinned and the normalised `float`s asserted (the divergence
//! test); and the sprite path, whose arrays stay UnityPy's bit for bit.
//! `tests/oracle.rs` replays every case (each against the corrected answer
//! the script records, its expected-divergence mask `NORMALISED`).

use std::sync::Arc;

use serde_json::{Value as J, json};
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::walk::{self, ReadCtx, ReadOptions, RefTypes};
use unityrs_core::{Bytes, Parsed};
use unityrs_meshdecode::{
    FormatTable, MeshError, MeshHandler, Norm, Processed, Source, StructCode, Values, Version,
    VertexArray,
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

/// The case `c` through the crate's handler, as a Mesh or a Sprite as the
/// case says, handed to `f`.
fn ours<R>(body: &J, c: &J, f: impl FnOnce(Result<Processed<'_>, MeshError>) -> R) -> R {
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
    let src = if c["source"] == "mesh" {
        Source::mesh(&parsed)
    } else {
        Source::sprite(&parsed)
    };
    let handler = MeshHandler::new(src, version).little_endian(c["endian"] == "<");
    f(handler.process(None, &Meter::unlimited()))
}

/// An array as the fixture writes it: an `int` as itself, a `float` as its
/// bits.
fn json_of(a: &VertexArray) -> J {
    let item = |i: usize| match a.values() {
        Values::Int(v) => json!(v[i]),
        Values::Float(v) => json!({ "f": format!("{:016x}", v[i].to_bits()) }),
        other => panic!("{other:?}"),
    };
    let n = match a.values() {
        Values::Int(v) => v.len(),
        Values::Float(v) => v.len(),
        other => panic!("{other:?}"),
    };
    let d = a.dimension();
    J::Array(
        (0..n.div_ceil(d))
            .map(|t| J::Array((t * d..n.min(t * d + d)).map(item).collect()))
            .collect(),
    )
}

/// Each width's extremes, as the cases lay them out: two vertices of four
/// components (`EXTREMES` in `meshdecode_cases.py`), by struct code.
fn extremes(code: StructCode) -> [i64; 8] {
    match code {
        StructCode::U8 => [0, 1, 127, 128, 254, 255, 64, 255],
        StructCode::I8 => [-128, -127, -1, 0, 1, 126, 127, -64],
        StructCode::U16 => [0, 1, 32767, 32768, 65534, 65535, 256, 65535],
        StructCode::I16 => [-32768, -32767, -1, 0, 1, 32766, 32767, -256],
        other => panic!("{other:?}"),
    }
}

/// The Vulkan and Direct3D conversion, written out here apart from the
/// crate's: `v / (2^n - 1)` unsigned, `max(v / (2^(n-1) - 1), -1)` signed.
fn reference(code: StructCode, v: i64) -> f64 {
    let v = f64::from(i32::try_from(v).unwrap());
    match code {
        StructCode::U8 => v / 255.0,
        StructCode::U16 => v / 65535.0,
        StructCode::I8 => (v / 127.0).max(-1.0),
        StructCode::I16 => (v / 32767.0).max(-1.0),
        other => panic!("{other:?}"),
    }
}

/// The normalised formats the cases cover, by the version that picks each
/// table: the colour bytes, `UNorm` and `SNorm`.
const FORMATS: &[(&str, [u16; 2], &[u8])] = &[
    ("5.6.7f1", [5, 6], &[2]),
    ("2017.4.0f1", [2017, 4], &[2, 3, 4, 5, 6]),
    ("2019.4.0f1", [2019, 4], &[2, 3, 4, 5]),
];

#[test]
fn the_rule_s_extremes_are_the_reference_s() {
    use StructCode::{I8, I16, U8, U16};
    assert_eq!(Norm::UNorm.value(U8, 0), 0.0);
    assert_eq!(Norm::UNorm.value(U8, 255), 1.0);
    assert_eq!(Norm::UNorm.value(U8, 128), 128.0 / 255.0);
    assert_eq!(Norm::UNorm.value(U16, 65535), 1.0);
    assert_eq!(Norm::UNorm.value(U16, 1), 1.0 / 65535.0);
    // The most negative value and the one above it are both -1.
    assert_eq!(Norm::SNorm.value(I8, -128), -1.0);
    assert_eq!(Norm::SNorm.value(I8, -127), -1.0);
    assert_eq!(Norm::SNorm.value(I8, 127), 1.0);
    assert_eq!(Norm::SNorm.value(I8, 0).to_bits(), 0.0f64.to_bits());
    assert_eq!(Norm::SNorm.value(I16, -32768), -1.0);
    assert_eq!(Norm::SNorm.value(I16, -32767), -1.0);
    assert_eq!(Norm::SNorm.value(I16, 32767), 1.0);
    assert_eq!(Norm::SNorm.value(I16, -1), -1.0 / 32767.0);
    // Every value of each width, against the reference.
    for (norm, code, range) in [
        (Norm::UNorm, U8, 0..=255),
        (Norm::SNorm, I8, -128..=127),
        (Norm::UNorm, U16, 0..=65535),
        (Norm::SNorm, I16, -32768..=32767),
    ] {
        for v in range {
            let have = norm.value(code, v);
            assert_eq!(have.to_bits(), reference(code, v).to_bits(), "{code:?} {v}");
            assert!((-1.0..=1.0).contains(&have));
        }
    }
}

/// Which formats each table normalises: those whose member is named
/// `UNorm`, `SNorm` or a colour, and no other.
#[test]
fn each_table_normalises_its_unorm_snorm_and_colour_formats() {
    for (table, names) in [
        (
            FormatTable::Channel,
            unityrs_meshdecode::VertexChannelFormat::ALL
                .map(|m| (m as u8, format!("{m:?}")))
                .to_vec(),
        ),
        (
            FormatTable::V2017,
            unityrs_meshdecode::VertexFormat2017::ALL
                .map(|m| (m as u8, format!("{m:?}")))
                .to_vec(),
        ),
        (
            FormatTable::V2019,
            unityrs_meshdecode::VertexFormat::ALL
                .map(|m| (m as u8, format!("{m:?}")))
                .to_vec(),
        ),
    ] {
        for (value, name) in names {
            let want = if name.contains("UNorm") || name.ends_with("FormatColor") {
                Some(Norm::UNorm)
            } else if name.contains("SNorm") {
                Some(Norm::SNorm)
            } else {
                None
            };
            assert_eq!(table.norm(value), want, "{name}");
        }
        assert_eq!(table.norm(255), None);
    }
}

/// `PARITY.md` section 19.1's UNorm/SNorm row (Tier C): UnityPy reads each
/// normalised format as the raw integers; the crate normalises a Mesh's.
#[test]
fn unorm_and_snorm_are_normalised_where_unitypy_keeps_the_integer() {
    let body = body();
    let mut checked = 0;
    for (version, [major, _], formats) in FORMATS {
        for &fmt in *formats {
            let table = FormatTable::for_major(*major);
            let code = table.struct_code(fmt).unwrap();
            let ext = extremes(code);
            let raw = json!([ext[..4], ext[4..]]);
            let want: Vec<f64> = ext.iter().map(|&v| reference(code, v)).collect();
            let mut arrays = vec![
                ("positions", "m_Vertices"),
                ("colour", "m_Colors"),
                ("uv0", "m_UV0"),
            ];
            if *major >= 2019 {
                arrays.push(("positions_big", "m_Vertices"));
            }
            for (suffix, attr) in arrays {
                let name = format!("normalised_{version}_{fmt}_{suffix}");
                let c = case(&body, &name);
                // UnityPy's side: the integers, unscaled.
                assert_eq!(c["process"]["ok"]["attrs"][attr], raw, "{name}");
                ours(&body, c, |p| {
                    let p = p.unwrap();
                    let a = match attr {
                        "m_Vertices" => p.vertices(),
                        "m_Colors" => p.colors(),
                        _ => p.uv(0),
                    }
                    .unwrap();
                    let Values::Float(v) = a.values() else {
                        panic!("{name}: not floats: {:?}", a.values())
                    };
                    let bits = |x: &[f64]| x.iter().map(|f| f.to_bits()).collect::<Vec<_>>();
                    assert_eq!(bits(v), bits(&want), "{name}");
                    // And the corrected answer the oracle records.
                    assert_eq!(json_of(a), c["corrected"]["ok"]["attrs"][attr], "{name}");
                });
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 34);
}

/// The sprite path (`unityrs-texture`'s renderer, `unityrs-export`'s NGUI
/// sprites) reads a Sprite's render data through `Source::sprite`: its
/// arrays stay UnityPy's bit for bit, integers kept, so what the renderer
/// reads is unchanged. No sprite case has a corrected answer, and the
/// `normalised_sprite_` cases' `UNorm` and `SNorm` positions and UV0 come
/// out as UnityPy's integers.
#[test]
fn the_sprite_path_reads_what_unitypy_reads() {
    let body = body();
    let mut read = 0;
    for c in body["cases"].as_array().unwrap() {
        if c["source"] != "sprite" {
            continue;
        }
        let name = c["name"].as_str().unwrap();
        assert!(c.get("corrected").is_none(), "{name}");
        let Some(attrs) = c["process"]["ok"]["attrs"].as_object() else {
            continue;
        };
        ours(&body, c, |p| {
            let p = p.unwrap();
            for (attr, a) in [("m_Vertices", p.vertices()), ("m_UV0", p.uv(0))] {
                let have = a.map_or(J::Null, json_of);
                let want = attrs.get(attr).cloned().unwrap_or(J::Null);
                assert_eq!(have, want, "{name} {attr}");
            }
        });
        read += 1;
    }
    assert!(read >= 5, "{read}");
    for (name, pos, uv) in [
        ("normalised_sprite_5_2", StructCode::I16, StructCode::U8),
        ("normalised_sprite_3_4", StructCode::I8, StructCode::U16),
    ] {
        ours(&body, case(&body, name), |p| {
            let p = p.unwrap();
            let (pe, ue) = (extremes(pos), extremes(uv));
            let ints = |array: &VertexArray| match array.values() {
                Values::Int(v) => v.clone(),
                other => panic!("{name}: {other:?}"),
            };
            let positions = [&pe[..3], &pe[3..6], &pe[5..8]].concat();
            assert_eq!(ints(p.vertices().unwrap()), positions, "{name}");
            assert_eq!(ints(p.uv(0).unwrap()), ue[..6].to_vec(), "{name}");
        });
    }
}

/// The blend indices (channel 13 from 2018) are bone indices: in a `UNorm`
/// format they stay UnityPy's integers, and the blend weights beside them
/// are normalised.
#[test]
fn the_blend_indices_stay_integers_and_the_weights_normalise() {
    let body = body();
    let c = case(&body, "normalised_2019.4.0f1_skin");
    let attrs = &c["process"]["ok"]["attrs"];
    assert_eq!(
        attrs["m_BoneIndices"],
        json!([[0, 1, 28, 255], [3, 2, 1, 0]])
    );
    let raw = extremes(StructCode::U8);
    assert_eq!(attrs["m_BoneWeights"], json!([raw[..4], raw[4..]]));
    assert_eq!(
        c["corrected"]["ok"]["attrs"]["m_BoneIndices"],
        attrs["m_BoneIndices"]
    );
    ours(&body, c, |p| {
        let p = p.unwrap();
        let indices = p.bone_indices().unwrap();
        assert_eq!(
            indices.values(),
            &Values::Int(vec![0, 1, 28, 255, 3, 2, 1, 0])
        );
        let weights = p.bone_weights().unwrap();
        let want: Vec<f64> = raw.iter().map(|&v| reference(StructCode::U8, v)).collect();
        assert_eq!(weights.values(), &Values::Float(want));
    });
}
