//! Synthetic corpus rows: the same protocol as `tools/parity/mesh_dump.py`.
use std::sync::Arc;
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::{Bytes, RawStr};
use unityrs_testkit::ngui::{nodes, write};
use unityrs_testkit::scene3d::mesh::{MeshVersion, mesh};
use unityrs_testkit::serialized::{ObjectSpec, SerializedSpec, TypeSpec};

fn mesh_bytes(indices: &[u32]) -> Vec<u8> {
    mesh_bytes_stream(indices, false)
}

fn mesh_bytes_stream(indices: &[u32], streamed: bool) -> Vec<u8> {
    let mut obj = mesh(
        7,
        b"caf\xc3\xa9",
        &[[0.; 3], [1., 0., 0.], [0., 1., 0.]],
        indices,
        MeshVersion::V2022,
    );
    if streamed {
        use unityrs_testkit::ngui::{V, st};
        if let V::Struct(_, fields) = &mut obj.value {
            fields.push((
                "m_StreamData".into(),
                st(
                    "StreamingInfo",
                    vec![
                        ("offset", V::UInt64(0)),
                        ("size", V::UInt(48)),
                        ("path", V::Str(b"mesh.resS".to_vec())),
                    ],
                ),
            ));
        }
    }
    let mut data = unityrs_testkit::ByteWriter::le();
    write(&mut data, &obj.value).unwrap();
    let spec = SerializedSpec {
        unity_version: MeshVersion::V2022.version().as_bytes().to_vec(),
        types: vec![TypeSpec {
            class_id: 43,
            nodes: nodes(0, "Base", &obj.value).unwrap(),
            ..TypeSpec::default()
        }],
        objects: vec![ObjectSpec {
            path_id: 7,
            data: data.into_inner(),
            ..ObjectSpec::default()
        }],
        ..SerializedSpec::default()
    };
    unityrs_testkit::serialized::build(&spec)
}

fn environment(indices: &[u32]) -> Environment {
    let env = Environment::new(EnvOptions::unitypy());
    env.load(Source::Bytes {
        name: RawStr::from("mesh.assets"),
        data: Bytes::from(mesh_bytes(indices)),
    })
    .unwrap();
    env
}

#[test]
fn mesh_rows_keep_unicode_and_match_the_python_digest_protocol() {
    let env = environment(&[0, 1, 2]);
    let object = env.objects().next().unwrap().unwrap();
    let meter = object.file().meter();
    let (line, charge) = unityrs_dump::mesh::record(b"caf\xc3\xa9", &object, &env, meter).unwrap();
    let row: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(row["input"], "café");
    assert!(line.contains("café"));
    assert_eq!(row["path_id"], 7);
    assert_eq!(row["result"]["arrays"]["vertex_count"], 3);
    assert_eq!(
        row["result"]["arrays"]["attrs"]["m_Vertices"],
        "4db2576f22fee6c42a5094a6dec708c6ee65936f8ae5ac5a3d8cdeb0ebc52bf9"
    );
    assert_eq!(row["result"]["obj"].as_str().unwrap().len(), 64);
    assert!(charge.bytes() >= line.len() as u64);
}

#[test]
fn a_cut_triangle_is_an_exception_row() {
    let env = environment(&[0, 1]);
    let object = env.objects().next().unwrap().unwrap();
    let (line, _) =
        unityrs_dump::mesh::record(b"input", &object, &env, object.file().meter()).unwrap();
    let row: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(row["result"], serde_json::json!({"error":"ValueError"}));
}

#[test]
fn record_growth_is_charged_and_refused_without_panicking() {
    let env = environment(&[0, 1, 2]);
    let object = env.objects().next().unwrap().unwrap();
    let meter = unityrs_core::limits::Meter::new(
        1,
        Arc::new(unityrs_core::budget::Unbounded),
        unityrs_core::budget::Reservation::Input,
    );
    assert!(unityrs_dump::mesh::record(b"input", &object, &env, &meter).is_err());
    assert_eq!(meter.used(), 0);
}

#[test]
fn mesh_mode_reuses_atomic_output_pins_and_parallel_order_without_header_records() {
    let scratch = unityrs_testkit::scratch::Scratch::new("mesh-corpus");
    let input = scratch.path().join("mesh.assets");
    std::fs::write(&input, mesh_bytes(&[0, 1, 2])).unwrap();
    let mut opts = unityrs_dump::run::RunOptions {
        selection: unityrs_dump::select::Selection {
            inputs: vec![input.clone()],
            ..Default::default()
        },
        output: scratch.path().join("serial.jsonl"),
        dump: unityrs_dump::DumpOptions {
            mesh: true,
            ..Default::default()
        },
        stats: None,
        quiet: true,
        allow_index_mismatch: false,
    };
    assert_eq!(unityrs_dump::run::run(&opts).unwrap().objects, 1);
    let serial = std::fs::read_to_string(&opts.output).unwrap();
    assert_eq!(serial.lines().count(), 1);
    let row: serde_json::Value = serde_json::from_str(serial.trim()).unwrap();
    assert_eq!(row["path_id"], 7);
    let other = scratch.path().join("other.assets");
    std::fs::copy(&input, &other).unwrap();
    opts.selection.inputs.push(other);
    opts.output = scratch.path().join("parallel.jsonl");
    opts.dump.jobs = 4;
    assert_eq!(unityrs_dump::run::run(&opts).unwrap().objects, 2);
    let parallel = std::fs::read(&opts.output).unwrap();
    opts.output = scratch.path().join("serial-two.jsonl");
    opts.dump.jobs = 1;
    unityrs_dump::run::run(&opts).unwrap();
    assert_eq!(std::fs::read(&opts.output).unwrap(), parallel);
    let bad = scratch.path().join("index.json");
    std::fs::write(
        &bad,
        serde_json::to_vec(
            &serde_json::json!({"files":[{"path":"mesh.assets","kind":"bundle","sha256":"wrong"}]}),
        )
        .unwrap(),
    )
    .unwrap();
    opts.selection.inputs = vec![bad];
    opts.selection.index = true;
    opts.output = scratch.path().join("bad.jsonl");
    assert!(unityrs_dump::run::run(&opts).is_err());
    assert!(!opts.output.exists());
}

#[test]
fn standalone_mesh_arrays_resolve_a_sibling_resource() {
    let scratch = unityrs_testkit::scratch::Scratch::new("mesh-resource");
    let input = scratch.path().join("mesh.assets");
    std::fs::write(&input, mesh_bytes_stream(&[0, 1, 2], true)).unwrap();
    let resource = [[0_f32; 3], [1., 0., 0.], [0., 1., 0.]]
        .into_iter()
        .flatten()
        .flat_map(f32::to_le_bytes)
        .chain([0; 12])
        .collect::<Vec<_>>();
    std::fs::write(scratch.path().join("mesh.resS"), resource).unwrap();
    let opts = unityrs_dump::run::RunOptions {
        selection: unityrs_dump::select::Selection {
            inputs: vec![input],
            ..Default::default()
        },
        output: scratch.path().join("mesh.jsonl"),
        dump: unityrs_dump::DumpOptions {
            mesh: true,
            ..Default::default()
        },
        stats: None,
        quiet: true,
        allow_index_mismatch: false,
    };
    unityrs_dump::run::run(&opts).unwrap();
    let rows = std::fs::read_to_string(&opts.output).unwrap();
    let row: serde_json::Value = serde_json::from_str(rows.trim()).unwrap();
    assert_eq!(
        row["result"]["arrays"]["attrs"]["m_Vertices"],
        "4db2576f22fee6c42a5094a6dec708c6ee65936f8ae5ac5a3d8cdeb0ebc52bf9"
    );
}

#[test]
fn a_missing_stream_resource_stops_without_publishing_partial_rows() {
    let scratch = unityrs_testkit::scratch::Scratch::new("mesh-missing-resource");
    let input = scratch.path().join("mesh.assets");
    std::fs::write(&input, mesh_bytes_stream(&[0, 1, 2], true)).unwrap();
    let opts = unityrs_dump::run::RunOptions {
        selection: unityrs_dump::select::Selection {
            inputs: vec![input],
            ..Default::default()
        },
        output: scratch.path().join("mesh.jsonl"),
        dump: unityrs_dump::DumpOptions {
            mesh: true,
            ..Default::default()
        },
        stats: None,
        quiet: true,
        allow_index_mismatch: false,
    };
    assert!(unityrs_dump::run::run(&opts).is_err());
    assert!(!opts.output.exists());
}

#[test]
fn input_hash_parse_and_output_share_one_decode_work_allowance() {
    let scratch = unityrs_testkit::scratch::Scratch::new("mesh-combined-work");
    let input = scratch.path().join("mesh.assets");
    std::fs::write(&input, mesh_bytes(&[0, 1, 2])).unwrap();
    let mut opts = unityrs_dump::run::RunOptions {
        selection: unityrs_dump::select::Selection {
            inputs: vec![input],
            ..Default::default()
        },
        output: scratch.path().join("baseline.jsonl"),
        dump: unityrs_dump::DumpOptions {
            mesh: true,
            ..Default::default()
        },
        stats: None,
        quiet: true,
        allow_index_mismatch: false,
    };
    let work = unityrs_dump::run::run(&opts).unwrap().decode_work;
    let mut context = (*opts.dump.context).clone();
    context.limits.max_decode_work = work - 1;
    opts.dump.context = Arc::new(context);
    opts.output = scratch.path().join("refused.jsonl");
    assert!(unityrs_dump::run::run(&opts).is_err());
    assert!(!opts.output.exists());
}

#[test]
fn one_input_memory_allowance_covers_read_parse_and_digest_rows_together() {
    let scratch = unityrs_testkit::scratch::Scratch::new("mesh-combined-memory");
    let input = scratch.path().join("mesh.assets");
    std::fs::write(&input, mesh_bytes(&[0, 1, 2])).unwrap();
    let mut opts = unityrs_dump::run::RunOptions {
        selection: unityrs_dump::select::Selection {
            inputs: vec![input],
            ..Default::default()
        },
        output: scratch.path().join("mesh.jsonl"),
        dump: unityrs_dump::DumpOptions {
            mesh: true,
            ..Default::default()
        },
        stats: None,
        quiet: true,
        allow_index_mismatch: false,
    };
    // Find the complete input's minimum combined memory budget. The input
    // meter must refuse the same boundary, even when the shared budget is
    // unlimited. Two independent source/output meters incorrectly pass it.
    let (mut lo, mut hi) = (1, 1 << 20);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let budget = Arc::new(unityrs_core::budget::Counting::new(mid));
        let mut context = (*opts.dump.context).clone();
        context.budget = budget.clone();
        opts.dump.context = Arc::new(context);
        if unityrs_dump::run::run(&opts).is_ok() {
            hi = mid;
        } else {
            lo = mid + 1;
        }
        assert_eq!(budget.used(), 0);
    }
    let mut context = (*opts.dump.context).clone();
    context.budget = Arc::new(unityrs_core::budget::Unbounded);
    context.limits.max_input_memory = lo - 1;
    opts.dump.context = Arc::new(context);
    assert!(unityrs_dump::run::run(&opts).is_err());
}
