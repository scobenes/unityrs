//! Shared bounded input and streaming digest for the 3D and animation targets.
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    io::{self, Write},
    sync::Arc,
};
use unityrs_core::{
    Bytes, RawStr,
    budget::{Reservation, Unbounded},
    env::{EnvOptions, Environment, Source},
    files::{File, SerializedFile},
    limits::Meter,
};
use unityrs_export::gltf::{ExportError, walk_files};
use unityrs_scene::{Graph, GraphOptions, graph_from_game_object, graph_from_scene_file};
use unityrs_testkit::fuzz::SceneCase;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Done(u64),
    Refused,
    Error(String),
}
impl Outcome {
    pub(crate) fn scene_error(e: unityrs_scene::SceneError) -> Self {
        use unityrs_core::limits::ChargeError;
        use unityrs_scene::SceneError;
        match e {
            SceneError::Charge(ChargeError::Limit(_)) => Self::Refused,
            e => Self::error(e),
        }
    }
    pub(crate) fn mesh_error(e: unityrs_mesh::MeshError) -> Self {
        use unityrs_core::{limits::ChargeError, objmode::ViewError};
        use unityrs_mesh::MeshError;
        use unityrs_meshdecode::MeshError as DecodeError;
        match e {
            MeshError::Charge(ChargeError::Limit(_))
            | MeshError::Decode(DecodeError::Charge(ChargeError::Limit(_)))
            | MeshError::View(ViewError::Charge(ChargeError::Limit(_)))
            | MeshError::Texture(unityrs_texture::DecodeError::Charge(ChargeError::Limit(_))) => {
                Self::Refused
            }
            MeshError::Scene(e) => Self::scene_error(e),
            e => Self::error(e),
        }
    }
    pub(crate) fn gltf_error(e: unityrs_gltf::GltfError) -> Self {
        match e {
            unityrs_gltf::GltfError::Charge(unityrs_core::limits::ChargeError::Limit(_)) => {
                Self::Refused
            }
            unityrs_gltf::GltfError::Mesh(e) => Self::mesh_error(e),
            e => Self::error(e),
        }
    }
    /// A resource refusal anywhere in the cause chain is `Refused`.
    pub(crate) fn anim_error(e: unityrs_anim::AnimError) -> Self {
        if anim_refusal(&e) {
            Self::Refused
        } else {
            Self::error(e)
        }
    }
    pub(crate) fn export_error(e: ExportError) -> Self {
        use unityrs_core::limits::ChargeError;
        match e {
            ExportError::Gltf(e) => Self::gltf_error(e),
            ExportError::Animation(e) => Self::anim_error(e),
            ExportError::Charge(ChargeError::Limit(_)) => Self::Refused,
            ExportError::Unresolved(e) if anim_refusal(&e.cause) => Self::Refused,
            e => Self::error(e),
        }
    }
    pub(crate) fn error(e: impl std::fmt::Debug) -> Self {
        Self::Error(format!("{e:?}"))
    }
}
/// The operation meter's refusals, and a lazy read's resource refusal.
pub(crate) fn anim_refusal(e: &unityrs_anim::AnimError) -> bool {
    use unityrs_anim::AnimError;
    use unityrs_core::limits::ChargeError;
    use unityrs_scene::SceneError;
    match e {
        AnimError::Charge(ChargeError::Limit(_)) => true,
        AnimError::Parse(e) => SceneError::Parse(e.clone()).resource_refusal(),
        AnimError::Deref(e) | AnimError::OutsideView(e) => {
            SceneError::Deref(e.clone()).resource_refusal()
        }
        _ => false,
    }
}
/// Counts every output byte without retaining a whole GLB or OBJ in memory.
#[derive(Default)]
pub(crate) struct Digest {
    hash: DefaultHasher,
    pub(crate) bytes: u64,
}
impl Digest {
    pub(crate) fn tag(&mut self, tag: impl Hash) {
        tag.hash(&mut self.hash);
    }
    pub(crate) fn finish(self) -> u64 {
        self.hash.finish()
    }
}
impl Write for Digest {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.hash.write(bytes);
        self.bytes += bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
pub(crate) fn run(
    case: &SceneCase,
    limit: u64,
    export: impl FnOnce(&Environment, &Arc<SerializedFile>, &Arc<Meter>) -> Outcome,
) -> Outcome {
    let mut options = EnvOptions::unitypy();
    options.context = crate::context();
    let env = Environment::new(options);
    let file = match env.load(Source::Bytes {
        name: RawStr::from_static(b"synthetic-scene"),
        data: Bytes::from(case.data.clone()),
    }) {
        Ok(File::Serialized(f)) => f,
        Ok(_) => return Outcome::Error("not a serialized scene".to_owned()),
        Err(e) => return Outcome::error(e),
    };
    let meter =
        Meter::with_decode_work(limit, crate::WORK, Arc::new(Unbounded), Reservation::Input);
    let answer = export(&env, &file, &meter);
    assert_eq!(
        meter.used(),
        0,
        "the operation retained charges after its outputs dropped"
    );
    answer
}
pub(crate) fn check(input: &[u8], export: impl Fn(&SceneCase, u64) -> Outcome + Sync) -> bool {
    if input.len() as u64 > crate::MEMORY {
        return false;
    }
    let case = SceneCase::decode(input);
    crate::with_stack(|| {
        let full = export(&case, crate::MEMORY);
        assert_eq!(
            export(&case, crate::MEMORY),
            full,
            "a fresh export changed its answer"
        );
        if case.meter != 0 {
            let limit = u64::from(case.meter).pow(2) * 64;
            let small = export(&case, limit);
            assert_eq!(
                export(&case, limit),
                small,
                "a fresh limited export changed its answer"
            );
            if matches!(full, Outcome::Done(_)) && small != Outcome::Refused {
                assert_eq!(
                    small, full,
                    "a successful smaller-meter export changed the bytes"
                );
            }
        }
        matches!(full, Outcome::Done(_))
    })
}
/// Like [`run`], but the input may be a bundle (or any container): the
/// operation gets the loaded root and the first serialized file, in
/// depth-first inspection order, that holds a `GameObject`.
pub(crate) fn run_root(
    case: &SceneCase,
    limit: u64,
    export: impl FnOnce(&Environment, &File, &Arc<SerializedFile>, &Arc<Meter>) -> Outcome,
) -> Outcome {
    let mut options = EnvOptions::unitypy();
    options.context = crate::context();
    let env = Environment::new(options);
    let root = match env.load(Source::Bytes {
        name: RawStr::from_static(b"synthetic-scene"),
        data: Bytes::from(case.data.clone()),
    }) {
        Ok(root) => root,
        Err(e) => return Outcome::error(e),
    };
    let meter =
        Meter::with_decode_work(limit, crate::WORK, Arc::new(Unbounded), Reservation::Input);
    let answer = (|| {
        let mut scene = None;
        let walk = walk_files(root.clone(), &meter, |file| -> Result<(), ExportError> {
            if let (None, File::Serialized(f)) = (&scene, file)
                && f.objects().any(|o| o.class_id() == 1)
            {
                scene = Some(f.clone());
            }
            Ok(())
        });
        if let Err(e) = walk {
            return Outcome::export_error(e);
        }
        let Some(file) = scene else {
            return Outcome::Error("no GameObject".to_owned());
        };
        export(&env, &root, &file, &meter)
    })();
    assert_eq!(
        meter.used(),
        0,
        "the operation retained charges after its outputs dropped"
    );
    answer
}
/// The scene file's graph, or (flag 1) the first `GameObject`'s prefab graph.
pub(crate) fn graph(
    env: &Environment,
    file: &Arc<SerializedFile>,
    flags: u8,
    meter: &Arc<Meter>,
) -> Result<Graph, Outcome> {
    let graph = if flags & 1 != 0 {
        let Some(root) = file.objects().find(|o| o.class_id() == 1) else {
            return Err(Outcome::Error("no prefab root".to_owned()));
        };
        graph_from_game_object(env, &root, &GraphOptions::default(), meter)
    } else {
        graph_from_scene_file(env, file, &GraphOptions::default(), meter)
    };
    graph.map_err(Outcome::scene_error)
}
