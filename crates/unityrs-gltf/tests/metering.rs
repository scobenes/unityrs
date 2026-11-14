use std::sync::Arc;
use unityrs_core::limits::Meter;
use unityrs_gltf::*;
mod support;

fn work(s: &Scene) -> u64 {
    let m = Meter::unlimited();
    layout(s, &Options::default(), &m).unwrap();
    m.decoded()
}
fn instance(s: &mut Scene, m: &Arc<Meter>, mesh: usize, skin: Option<usize>) {
    let mut n = Node::new(m);
    n.mesh = Some(mesh);
    n.skin = skin;
    s.roots.push(s.nodes.len()).unwrap();
    s.nodes.push(n).unwrap();
}
#[test]
fn per_instance_primitive_and_attribute_scans_are_charged() {
    let m = Meter::unlimited();
    for attributes in [1, 4] {
        let mut s = support::texcoords(&m, &[0, 1, 2]);
        s.meshes.as_mut_slice()[0].primitives.as_mut_slice()[0]
            .attributes
            .truncate(attributes);
        for _ in 1..100 {
            let mut p = support::primitive(&m).unwrap();
            p.attributes = array(
                &m,
                s.meshes.as_slice()[0].primitives.as_slice()[0]
                    .attributes
                    .as_slice(),
            )
            .unwrap();
            s.meshes.as_mut_slice()[0].primitives.push(p).unwrap();
        }
        let base = work(&s);
        for _ in 0..100 {
            instance(&mut s, &m, 0, None);
        }
        let added = work(&s) - base;
        assert!(
            added >= 100 * 100 * (1 + attributes as u64),
            "unmetered scans: {added}"
        );
        let limited = Meter::with_decode_work(
            u64::MAX,
            base + 100 * 100 * (1 + attributes as u64) - 1,
            Arc::new(unityrs_core::budget::Unbounded),
            unityrs_core::budget::Reservation::Input,
        );
        assert!(matches!(
            layout(&s, &Options::default(), &limited),
            Err(GltfError::Charge(_))
        ));
    }
}
fn skin_instance_work(m: &Arc<Meter>, repetitions: usize) -> u64 {
    let mut s = support::rich(m).unwrap();
    for a in s.accessors.as_mut_slice() {
        if matches!(a.kind, Kind::Indices | Kind::InverseBind) {
            continue;
        }
        a.count *= repetitions;
        if a.sparse_indices.is_some() {
            continue;
        }
        match &mut a.data {
            Data::Float(v) => {
                let original = v.as_slice().to_vec();
                for _ in 1..repetitions {
                    v.extend_from_slice(&original).unwrap();
                }
            }
            Data::U16(v) => {
                let original = v.as_slice().to_vec();
                for _ in 1..repetitions {
                    v.extend_from_slice(&original).unwrap();
                }
            }
            _ => unreachable!(),
        }
    }
    let base = work(&s);
    for _ in 0..20 {
        instance(&mut s, m, 0, Some(0));
    }
    work(&s) - base
}
#[test]
fn joint_maxima_are_computed_once_instead_of_per_instance() {
    let m = Meter::unlimited();
    let small = skin_instance_work(&m, 1);
    let large = skin_instance_work(&m, 1000);
    assert_eq!(
        large, small,
        "per-instance work depends on joint accessor length"
    );
}
fn skin_work(nodes: usize, skins: usize) -> u64 {
    let m = Meter::unlimited();
    let mut s = Scene::new(&m);
    for _ in 0..nodes {
        s.nodes.push(Node::new(&m)).unwrap();
    }
    for _ in 0..skins {
        s.skins
            .push(Skin {
                name: None,
                joints: array(&m, &[0]).unwrap(),
                inverse_bind_matrices: None,
                skeleton: None,
            })
            .unwrap();
    }
    work(&s)
}
#[test]
fn skin_duplicate_detection_clears_only_each_skins_joint_slots() {
    let per_skin_small = skin_work(10, 100) - skin_work(10, 1);
    let per_skin_large = skin_work(1000, 100) - skin_work(1000, 1);
    assert_eq!(
        per_skin_large, per_skin_small,
        "each skin reinitialises the node bitmap"
    );
}
