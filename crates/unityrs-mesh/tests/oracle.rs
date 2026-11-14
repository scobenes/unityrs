//! L6 replay through `read_mesh`, using the existing oracle's synthetic bytes.
//! The fixture's corrected answers are guarded by meshdecode's explicit
//! divergence-mask test; this replay uses those exact f64 bits and Python tags.
use serde_json::{Value as J, json};
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, File, RawStr};
use unityrs_mesh::{ProcessedOwned, Triangles, read_mesh};
use unityrs_meshdecode::{Component, VertexArray};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::serialized::{self, SerializedSpec, one_object};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
fn array(a: &VertexArray) -> J {
    (0..a.len())
        .map(|i| {
            (0..a.tuple_len(i).unwrap())
                .map(|j| match a.component(i, j).unwrap() {
                    Component::Int(i) => json!(i),
                    Component::Float(f) => json!({"f": format!("{:016x}", f.to_bits())}),
                    v => panic!("unexpected {v:?}"),
                })
                .collect::<J>()
        })
        .collect()
}
fn arrays(p: &ProcessedOwned) -> J {
    let mut attrs = serde_json::Map::new();
    for (name, a) in [
        ("m_Vertices", p.vertices()),
        ("m_Normals", p.normals()),
        ("m_Tangents", p.tangents()),
        ("m_Colors", p.colors()),
        ("m_BoneWeights", p.bone_weights()),
        ("m_BoneIndices", p.bone_indices()),
    ] {
        if let Some(a) = a {
            attrs.insert(name.to_owned(), array(a));
        }
    }
    for i in 0..8 {
        if let Some(a) = p.uv(i) {
            attrs.insert(format!("m_UV{i}"), array(a));
        }
    }
    if let Some(a) = p.bind_pose() {
        let flat = array(a);
        attrs.insert(
            "m_BindPose".to_owned(),
            flat.as_array()
                .unwrap()
                .iter()
                .map(|m| {
                    let m = m.as_array().unwrap();
                    (0..4)
                        .map(|r| json!(m.get(r * 4..m.len().min(r * 4 + 4)).unwrap_or(&[])))
                        .collect::<J>()
                })
                .collect(),
        );
    }
    json!({"vertex_count":p.vertex_count(), "use16":p.use_16bit_indices(), "index_buffer":p.index_buffer().map(unityrs_meshdecode::IndexBuffer::as_slice), "attrs":attrs})
}
fn triangles(t: &Triangles) -> J {
    json!({"ok": {"submeshes":t.submeshes().iter().map(|s|json!({"tri":s.triangles(),"cut":s.cut(),"empty":s.empty()})).collect::<Vec<_>>(),
        "index_counts":t.submeshes().iter().map(unityrs_meshdecode::SubMeshTriangles::index_count).collect::<Vec<_>>()}})
}

#[test]
#[allow(clippy::too_many_lines)]
fn mesh_model_arrays_equal_the_masked_unitypy_oracle() {
    let body: J = serde_json::from_str(include_str!(
        "../../unityrs-meshdecode/tests/fixtures/process.json"
    ))
    .unwrap();
    let mut checked = 0;
    let mut compressed = 0;
    let mut partial = 0;
    let mut refused = 0;
    let mut deferred = 0;
    for c in body["cases"].as_array().unwrap() {
        let want = c.get("corrected").unwrap_or(&c["process"]);
        if c["source"] != "mesh" || c["endian"] != "<" {
            continue;
        }
        // The two enormous offsets use a mock Resources in meshdecode's
        // oracle; no real in-memory resource can be constructed at those offsets.
        // They remain covered by the owned decoder replay.
        if want["ok"]["resource_call"][1]
            .as_u64()
            .is_some_and(|n| n > 1024)
        {
            continue;
        }
        // ObjectRef always supplies a reader, and SerializedFile supplies a
        // recognised Unity version. The short-resource mock returns fewer
        // bytes than requested; Environment rejects it before mesh decoding.
        // These mock-only failures stay covered by meshdecode's oracle replay.
        if c["name"].as_str().unwrap().ends_with("_no_reader")
            || c["name"] == "version_type_6"
            || c["name"] == "stream_data_short"
        {
            continue;
        }
        let v = c["version"].as_array().unwrap();
        let part = |i: usize| v[i].as_u64().unwrap();
        let ty = match part(3) {
            0 => "a",
            1 => "b",
            2 => "c",
            3 => "f",
            4 => "p",
            5 => "x",
            255 => "u",
            _ => unreachable!(),
        };
        let version = format!("{}.{}.{}{}{}", part(0), part(1), part(2), ty, part(4));
        let nodes = body["trees"][c["tree"].as_str().unwrap()]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let mut node = NodeSpec::new(
                    r[0].as_u64().unwrap().try_into().unwrap(),
                    r[1].as_str().unwrap(),
                    r[2].as_str().unwrap(),
                )
                .size(-1);
                node.meta_flag = r[3].as_i64().unwrap().try_into().unwrap();
                node
            })
            .collect();
        let file = serialized::build(&SerializedSpec {
            unity_version: version.into_bytes(),
            ..one_object(43, nodes, unhex(c["hex"].as_str().unwrap()))
        });
        let mut entries = vec![("CAB-x", file)];
        if let Some(resource) = c["resource"].as_str().filter(|_| c["reader"] == true) {
            let mut bytes = vec![
                0xA5;
                want["ok"]["resource_call"][1]
                    .as_u64()
                    .unwrap_or(0)
                    .try_into()
                    .unwrap()
            ];
            bytes.extend(unhex(resource));
            entries.push(("CAB-x.resS", bytes));
        }
        let env = Environment::new(EnvOptions::unitypy());
        let File::Bundle(bundle) = env
            .load(Source::Bytes {
                name: RawStr::from("mesh.bundle"),
                data: Bytes::from(unityrs_testkit::env::bundle(&entries)),
            })
            .unwrap()
        else {
            panic!("not bundle")
        };
        let File::Serialized(file) = bundle.entry(0).unwrap() else {
            panic!("not file")
        };
        let meter = Meter::unlimited();
        if c["name"] == "compressed_ints_33" {
            assert!(matches!(
                read_mesh(&file.object(1).unwrap(), &env, &meter),
                Err(unityrs_mesh::MeshError::Decode(
                    unityrs_meshdecode::MeshError::Malformed(_)
                ))
            ));
            assert_eq!(meter.used(), 0);
            continue;
        }
        let result = read_mesh(&file.object(1).unwrap(), &env, &meter);
        if let Some(kind) = want.get("p2a") {
            let Err(unityrs_mesh::MeshError::Decode(unityrs_meshdecode::MeshError::Unsupported(u))) =
                result
            else {
                panic!("{}: expected deferred {kind}, got {result:?}", c["name"])
            };
            let actual = match u {
                unityrs_meshdecode::Unsupported::FixedChannels => "fixed_channels".to_owned(),
                unityrs_meshdecode::Unsupported::MeshArray(f) => format!("mesh_array:{f}"),
                other => panic!("unexpected {other:?}"),
            };
            assert_eq!(actual, kind.as_str().unwrap());
            deferred += 1;
            assert_eq!(meter.used(), 0);
            continue;
        }
        if let Some(want_error) = want.get("err").or_else(|| c["triangles"].get("err")) {
            let error = result.unwrap_err();
            let unityrs_mesh::MeshError::Decode(error) = error else {
                panic!("{}: {error:?}", c["name"])
            };
            let (class, message) = error.python().unwrap();
            assert_eq!(json!([class, message]), *want_error, "{}", c["name"]);
            refused += 1;
            assert_eq!(meter.used(), 0);
            continue;
        }
        let model = result.unwrap_or_else(|e| panic!("{}: {e:?}", c["name"]));
        let mut expected = want["ok"].clone();
        expected.as_object_mut().unwrap().remove("resource_call");
        if let Some(bind) = expected["attrs"]["m_BindPose"].as_array() {
            let values: Vec<_> = bind
                .iter()
                .flat_map(|rows| rows.as_array().unwrap())
                .flat_map(|row| row.as_array().unwrap())
                .collect();
            partial += usize::from(values.len() % 16 != 0);
            // Exact scalar count also pins the truncated rows: no padding or loss.
            assert_eq!(model.bind_poses.len(), values.len(), "{}", c["name"]);
            for (v, want) in model.bind_poses.as_slice().iter().zip(values) {
                let bits = u64::from_str_radix(want["f"].as_str().unwrap(), 16).unwrap();
                #[allow(clippy::cast_possible_truncation)]
                let f = f64::from_bits(bits) as f32;
                assert_eq!(v.to_bits(), f.to_bits());
            }
        }
        assert_eq!(arrays(&model.processed), expected, "{}", c["name"]);
        assert_eq!(triangles(&model.triangles), c["triangles"], "{}", c["name"]);
        compressed += usize::from(c["name"].as_str().unwrap().starts_with("compressed_"));
        drop(model);
        assert_eq!(meter.used(), 0);
        checked += 1;
    }
    assert_eq!(checked, 419);
    assert_eq!(partial, 5);
    assert_eq!(refused, 76);
    assert_eq!(deferred, 4);
    assert!(compressed >= 10, "{compressed} compressed cases");
}
