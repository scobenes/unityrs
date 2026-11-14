use unityrs_core::RawStr;
use unityrs_core::limits::Meter;
use unityrs_mesh::{
    obj::{ChargedBytes, Written, export_mesh_obj, export_mesh_obj_owned},
    read_mesh_for_obj,
};
use unityrs_testkit::scene3d::mesh::{MeshVersion, mesh};
mod support;

#[test]
fn obj_raw_geometry_materials_and_empty_false() {
    let (env, file) = support::load(
        MeshVersion::V2022,
        &[mesh(
            1,
            b"m",
            &[
                [0., 1e-10, f32::NAN],
                [f32::INFINITY, -1.5, 0.1],
                [1., 2., 3.],
            ],
            &[0, 1, 2],
            MeshVersion::V2022,
        )],
    );
    let m = read_mesh_for_obj(&file.object(1).unwrap(), &env, file.meter()).unwrap();
    let mat = RawStr::from("mat");
    let out = export_mesh_obj_owned(&m, Some(&[Some(&mat)]), &Meter::unlimited())
        .unwrap()
        .unwrap();
    assert_eq!(out.as_bytes(), b"g m\nmtllib m.mtl\nv -0 1.00000001E-10 NAN\nv -INF -1.5 0.100000001\nv -1 2 3\ng m_0\nusemtl mat\nf 3/3/3 2/2/2 1/1/1\n");
    let (env, file) = support::load(
        MeshVersion::V2022,
        &[mesh(1, b"empty", &[], &[], MeshVersion::V2022)],
    );
    let m = read_mesh_for_obj(&file.object(1).unwrap(), &env, file.meter()).unwrap();
    let mut out = ChargedBytes::new(&Meter::unlimited());
    assert_eq!(export_mesh_obj(&m, None, &mut out).unwrap(), Written::False);
    assert_eq!(out.as_bytes(), []);
}

#[test]
fn output_owner_retains_charge_and_refuses_before_growth() {
    use unityrs_mesh::obj::MeteredWrite;
    let meter = Meter::unlimited();
    let mut out = ChargedBytes::new(&meter);
    let before = meter.used();
    out.write_bytes(b"test").unwrap();
    assert!(meter.used() > before);
    drop(out);
    assert_eq!(meter.used(), before);
}

#[test]
fn metered_output_refuses_memory_work_and_io_and_false_needs_no_budget() {
    use std::sync::Arc;
    use unityrs_core::{
        budget::{Reservation, Unbounded},
        limits::ChargeError,
    };
    use unityrs_mesh::{
        MeshError,
        obj::{MeteredWrite, Writer},
    };
    let meter = Meter::new(0, Arc::new(Unbounded), Reservation::Input);
    let mut out = ChargedBytes::new(&meter);
    assert!(matches!(
        out.write_bytes(b"x"),
        Err(MeshError::Charge(ChargeError::Limit(_)))
    ));
    assert_eq!(out.as_bytes(), []);
    assert_eq!(meter.used(), 0);
    let work = Meter::with_decode_work(u64::MAX, 0, Arc::new(Unbounded), Reservation::Input);
    let mut out = ChargedBytes::new(&work);
    assert!(matches!(
        out.write_bytes(b"x"),
        Err(MeshError::Charge(ChargeError::Limit(_)))
    ));
    assert_eq!(out.as_bytes(), []);
    drop(out);
    assert_eq!(work.used(), 0);
    let mut empty: &mut [u8] = &mut [];
    let mut writer = Writer::new(&mut empty, &Meter::unlimited());
    assert!(matches!(writer.write_bytes(b"x"), Err(MeshError::Io(_))));
    let (env, file) = support::load(
        MeshVersion::V2022,
        &[mesh(1, b"", &[], &[], MeshVersion::V2022)],
    );
    let m = read_mesh_for_obj(&file.object(1).unwrap(), &env, file.meter()).unwrap();
    let mut out = ChargedBytes::new(&meter);
    assert_eq!(export_mesh_obj(&m, None, &mut out).unwrap(), Written::False);
    assert_eq!(meter.used(), 0);
}

#[test]
fn obj_normals_uv_winding_and_short_material_list_match_unitypy() {
    let mut case = export_cases::cases()
        .into_iter()
        .find(|c| c.name == "raw_specials")
        .unwrap();
    let sub = unityrs_testkit::scene3d::mesh::submesh(0, 3, 0, 0, 0, 3);
    unityrs_testkit::scene3d::set(
        &mut case.objects[0].value,
        "m_SubMeshes",
        unityrs_testkit::ngui::vector(sub.clone(), vec![sub.clone(), sub.clone(), sub]),
    );
    let (env, file) = support::load(MeshVersion::V6000, &case.objects);
    let m = read_mesh_for_obj(&file.object(4).unwrap(), &env, file.meter()).unwrap();
    let mat = RawStr::from("mat");
    let out = export_mesh_obj_owned(&m, Some(&[Some(&mat), None]), &Meter::unlimited())
        .unwrap()
        .unwrap();
    assert_eq!(out.as_bytes(),b"g m\nmtllib m.mtl\nv -0 1.00000001E-10 NAN\nv -INF -1.5 0.100000001\nv -1 2 3\nvt 0.5 NAN\nvt 0.25 1\nvt 0 0\nvn -1 0 0\nvn -0 1 0\nvn NAN 0 -1\ng m_0\nusemtl mat\nf 3/3/3 2/2/2 1/1/1\ng m_1\nf 3/3/3 2/2/2 1/1/1\ng m_2\nf 3/3/3 2/2/2 1/1/1\n");
}
#[allow(
    dead_code,
    reason = "shared inputs; this replay consumes only geometry"
)]
#[path = "support/export_cases.rs"]
mod export_cases;
