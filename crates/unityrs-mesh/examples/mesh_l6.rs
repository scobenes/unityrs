//! Ephemeral L6 sample replay for `tools/parity/mesh_samples.py`. JSON stays
//! in a pipe; no sample data is saved in the checkout.
#![allow(clippy::format_collect, reason = "test protocol hex encoding")]
use serde_json::{Value as J, json};
use std::sync::Arc;
use unityrs_core::{
    env::{EnvOptions, Environment, Source},
    limits::Meter,
    provider::ProviderChain,
};
use unityrs_mesh::{ProcessedOwned, Triangles, read_mesh_for_obj};
use unityrs_meshdecode::{Component, VertexArray};
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let mut opts = EnvOptions::unitypy();
    let mut context = (*opts.context).clone();
    context.providers = Arc::new(ProviderChain::new(vec![Arc::new(
        unityrs_tpk::TpkProvider::open(args.get(1).ok_or("TPK")?)?,
    )]));
    opts.context = Arc::new(context);
    let env = Environment::new(opts);
    env.load(Source::Path(args.first().ok_or("INPUT")?.into()))?;
    let mut records = Vec::new();
    for object in env.objects() {
        let object = object?;
        if object.class_id() != 43 {
            continue;
        }
        let meter = Meter::unlimited();
        let result = mesh_result(&object, &env, &meter);
        records.push(json!({"file":object.file().name().as_bytes().iter().map(|v|format!("{v:02x}")).collect::<String>(),"path_id":object.path_id(),"result":result}));
    }
    records.sort_by_key(|r| {
        (
            r["file"].as_str().unwrap_or_default().to_owned(),
            r["path_id"].as_i64().unwrap_or(0),
        )
    });
    println!("{}", serde_json::to_string(&records)?);
    Ok(())
}

fn mesh_result(object: &unityrs_core::ObjectRef, env: &Environment, meter: &Arc<Meter>) -> J {
    match read_mesh_for_obj(object, env, meter) {
        Ok(m) => match unityrs_mesh::obj::export_mesh_obj_owned(&m, None, meter) {
            Ok(obj) => {
                json!({"arrays":arrays(&m.processed),"triangles":triangles(&m.triangles),"obj":obj.map(|b|b.as_bytes().iter().map(|v|format!("{v:02x}")).collect::<String>())})
            }
            Err(unityrs_mesh::MeshError::Field("triangle tuple")) => json!({"error":"ValueError"}),
            Err(unityrs_mesh::MeshError::Field("attribute tuple")) => json!({"error":"IndexError"}),
            Err(_) => json!({"error":"export_error"}),
        },
        Err(unityrs_mesh::MeshError::Decode(e)) => {
            json!({"error":e.python().map_or("unsupported",|(class,_)|class)})
        }
        Err(_) => json!({"error":"read_error"}),
    }
}
#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod support;
#[test]
fn cut_triangle_records_an_error_without_ending_the_replay() {
    let m = unityrs_testkit::scene3d::mesh::mesh(
        1,
        b"cut",
        &[[0.; 3]; 3],
        &[0, 1],
        unityrs_testkit::scene3d::mesh::MeshVersion::V2022,
    );
    let (env, file) = support::load(unityrs_testkit::scene3d::mesh::MeshVersion::V2022, &[m]);
    assert_eq!(
        mesh_result(&file.object(1).unwrap(), &env, &Meter::unlimited()),
        json!({"error":"ValueError"})
    );
}
