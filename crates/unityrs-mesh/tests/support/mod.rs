use std::sync::Arc;
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::{Bytes, File, RawStr, SerializedFile};
use unityrs_testkit::ngui::{Object, nodes, write};
use unityrs_testkit::scene3d::mesh::MeshVersion;
use unityrs_testkit::serialized::{self, ObjectSpec, SerializedSpec, TypeSpec};

#[allow(dead_code)]
pub fn load(version: MeshVersion, objects: &[Object]) -> (Environment, Arc<SerializedFile>) {
    load_with_budget(version, objects, Arc::new(unityrs_core::budget::Unbounded))
}

#[allow(dead_code)]
pub fn load_with_budget(
    version: MeshVersion,
    objects: &[Object],
    budget: Arc<dyn unityrs_core::budget::MemoryBudget>,
) -> (Environment, Arc<SerializedFile>) {
    load_with_platform(version, objects, budget, 13)
}

#[allow(dead_code)]
pub fn load_with_platform(
    version: MeshVersion,
    objects: &[Object],
    budget: Arc<dyn unityrs_core::budget::MemoryBudget>,
    platform: i32,
) -> (Environment, Arc<SerializedFile>) {
    let spec = SerializedSpec {
        platform,
        unity_version: version.version().as_bytes().to_vec(),
        types: objects
            .iter()
            .map(|obj| TypeSpec {
                class_id: obj.class_id,
                nodes: nodes(0, "Base", &obj.value).unwrap(),
                ..TypeSpec::default()
            })
            .collect(),
        objects: objects
            .iter()
            .enumerate()
            .map(|(i, obj)| ObjectSpec {
                path_id: obj.path_id,
                type_id: i.try_into().unwrap(),
                data: {
                    let mut w = unityrs_testkit::ByteWriter::le();
                    write(&mut w, &obj.value).unwrap();
                    w.into_inner()
                },
                ..ObjectSpec::default()
            })
            .collect(),
        ..SerializedSpec::default()
    };
    let mut options = EnvOptions::unitypy();
    let mut context = (*options.context).clone();
    context.budget = budget;
    options.context = Arc::new(context);
    let env = Environment::new(options);
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from("mesh.assets"),
            data: Bytes::from(serialized::build(&spec)),
        })
        .unwrap()
    else {
        panic!("not a serialized file")
    };
    (env, file)
}

#[allow(dead_code)]
pub fn rich_object(n: u32) -> Object {
    use unityrs_testkit::ngui::{V, set_field, vector};
    use unityrs_testkit::scene3d::mesh as m;
    let mut obj = m::mesh(
        1,
        &vec![b'n'; n as usize],
        &vec![[1.0, 2.0, 3.0]; n as usize],
        &(0..n).collect::<Vec<_>>(),
        MeshVersion::V6000,
    );
    obj = set_field(
        obj,
        "m_BindPose",
        vector(
            m::matrix([0.0; 16]),
            (0..n).map(|_| m::matrix([1.0; 16])).collect(),
        ),
        None,
    );
    obj = set_field(
        obj,
        "m_BoneNameHashes",
        vector(V::UInt(0), (0..n).map(V::UInt).collect()),
        None,
    );
    obj = set_field(
        obj,
        "m_Shapes",
        m::blend_shapes(
            (0..n)
                .map(|i| m::blend_vertex(i, [1.0; 3], [2.0; 3], [3.0; 3]))
                .collect(),
            (0..n).map(|i| m::blend_frame(i, 1, true, true)).collect(),
            (0..n)
                .map(|i| m::blend_channel(&[b'c'; 64], i, i.try_into().unwrap(), 1))
                .collect(),
            vec![100.0; n as usize],
        ),
        None,
    );
    set_field(
        obj,
        "m_MeshLodInfo",
        m::lod_info(n, 1.0, 2.0, &vec![vec![(0, n); n as usize]; 4]),
        None,
    )
}
