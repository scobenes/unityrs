//! The owned handler result preserves every component and its Python type.
use std::sync::Arc;

use serde_json::Value as J;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::objmode::{MESH, Obj};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::walk::{self, ReadCtx, ReadOptions, RefTypes};
use unityrs_core::{Bytes, RawStr};
use unityrs_meshdecode::{MeshHandler, Resources, Source, Version};

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

#[test]
#[allow(clippy::too_many_lines)]
fn ownership_moves_every_array_and_charge_without_copying() {
    let body: J = serde_json::from_str(include_str!("fixtures/process.json")).unwrap();
    let mut moved = 0;
    for c in body["cases"].as_array().unwrap() {
        let rows = body["trees"][c["tree"].as_str().unwrap()]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                NodeRow::new(
                    i32::try_from(r[0].as_i64().unwrap()).unwrap(),
                    r[1].as_str().unwrap(),
                    r[2].as_str().unwrap(),
                )
                .with_meta_flag(r[3].as_i64().unwrap())
            })
            .collect();
        let tree = Tree::compile(
            Arc::new(NodeList::trusted(rows).unwrap()),
            &Limits::default(),
        )
        .unwrap();
        let options = ReadOptions::default();
        let input = Meter::unlimited();
        let parsed = walk::boost::read(
            &Bytes::from(unhex(c["hex"].as_str().unwrap())),
            &tree,
            &ReadCtx {
                endian: Endian::Little,
                ref_types: RefTypes::List(&[]),
                options: &options,
                meter: &input,
            },
        )
        .unwrap();
        let meter = Meter::unlimited();
        if c["source"] == "mesh" {
            // The public core view is the same object-mode view the handler uses.
            let obj = Obj::root(&parsed, &MESH, &meter).unwrap();
            if obj.built() {
                assert!(obj.string("m_Name").unwrap().is_some());
            }
        }
        let v: Vec<u64> = c["version"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .collect();
        let version = Version::new(
            v[0].try_into().unwrap(),
            v[1].try_into().unwrap(),
            v[2].try_into().unwrap(),
            v[3].try_into().unwrap(),
            v[4].try_into().unwrap(),
        );
        let src = if c["source"] == "mesh" {
            Source::mesh(&parsed)
        } else {
            Source::sprite(&parsed)
        };
        let resource = c["resource"]
            .as_str()
            .filter(|_| c["reader"] == true)
            .map(|s| Resource(Bytes::from(unhex(s))));
        let Ok(p) = MeshHandler::new(src, version)
            .little_endian(c["endian"] == "<")
            .process(resource.as_ref().map(|r| r as &dyn Resources), &meter)
        else {
            continue;
        };
        let arrays = |p: &unityrs_meshdecode::Processed<'_>| {
            [
                p.vertices(),
                p.normals(),
                p.tangents(),
                p.colors(),
                p.bone_weights(),
                p.bone_indices(),
                p.bind_pose(),
            ]
            .into_iter()
            .chain((0..8).map(|i| p.uv(i)))
            .map(|a| a.map(|a| (std::ptr::from_ref(a), format!("{:?}", a.values()))))
            .collect::<Vec<_>>()
        };
        let before = arrays(&p);
        let indices = p.index_buffer().map(|b| b.as_slice().as_ptr());
        let count = p.vertex_count();
        let use16 = p.use_16bit_indices();
        let used = meter.used();
        let granted = meter.granted();
        let owned = p.into_owned();
        drop(parsed);
        assert_eq!((meter.used(), meter.granted()), (used, granted));
        assert_eq!(
            (owned.vertex_count(), owned.use_16bit_indices()),
            (count, use16)
        );
        assert_eq!(owned.index_buffer().map(|b| b.as_slice().as_ptr()), indices);
        let after = [
            owned.vertices(),
            owned.normals(),
            owned.tangents(),
            owned.colors(),
            owned.bone_weights(),
            owned.bone_indices(),
            owned.bind_pose(),
        ]
        .into_iter()
        .chain((0..8).map(|i| owned.uv(i)))
        .map(|a| a.map(|a| format!("{:?}", a.values())))
        .collect::<Vec<_>>();
        assert_eq!(
            after,
            before
                .into_iter()
                .map(|v| v.map(|(_, values)| values))
                .collect::<Vec<_>>()
        );
        assert!(owned.uv(8).is_none());
        drop(owned);
        assert_eq!(meter.used(), 0);
        moved += 1;
    }
    assert!(moved > 400, "{moved} successful cases moved");
}
