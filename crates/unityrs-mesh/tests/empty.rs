use std::sync::Arc;
use unityrs_core::{
    budget::{Reservation, Unbounded},
    limits::{ChargeError, Meter},
};
use unityrs_mesh::{MeshError, read_mesh, read_mesh_for_gltf, read_mesh_for_obj};
use unityrs_testkit::scene3d::mesh::{MeshVersion, mesh};
mod support;

#[test]
fn gltf_empty_policy_leaves_strict_and_obj_triangle_behavior_unchanged() {
    let (env, file) = support::load(
        MeshVersion::V2022,
        &[mesh(1, b"empty", &[], &[], MeshVersion::V2022)],
    );
    let object = file.object(1).unwrap();
    let meter = Meter::unlimited();
    assert!(matches!(
        read_mesh(&object, &env, &meter),
        Err(MeshError::Decode(unityrs_meshdecode::MeshError::Assertion(
            ""
        )))
    ));
    assert_eq!(meter.used(), 0);
    assert!(matches!(
        read_mesh_for_gltf(&object, &env, &meter),
        Err(MeshError::Empty)
    ));
    assert_eq!(meter.used(), 0);
    let obj = read_mesh_for_obj(&object, &env, &meter).unwrap();
    assert!(obj.triangles.submeshes().is_empty());
    drop(obj);
    assert_eq!(meter.used(), 0);

    let meter = Meter::new(0, Arc::new(Unbounded), Reservation::Input);
    assert!(matches!(
        read_mesh_for_gltf(&object, &env, &meter),
        Err(MeshError::Decode(unityrs_meshdecode::MeshError::Charge(
            ChargeError::Limit(_)
        )))
    ));
    assert_eq!(meter.used(), 0);
}
