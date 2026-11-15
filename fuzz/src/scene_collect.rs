//! Collect a mutated testkit scene/prefab and stream its GLB twice.
use crate::scene::{Digest, Outcome};
use unityrs_gltf::{CorePbr, Profile, UnlitExtras, collect_with_mapper, write_glb};
use unityrs_scene::{GraphOptions, graph_from_game_object, graph_from_scene_file};

/// Panics on a nondeterministic result or retained operation charge.
pub fn check(input: &[u8]) -> bool {
    crate::scene::check(input, |case, limit| {
        crate::scene::run(case, limit, |env, file, meter| {
            let profile = if case.profile.is_empty() {
                Ok(Profile::new(meter))
            } else {
                Profile::parse(&case.profile, meter)
            };
            let profile = match profile {
                Ok(p) => p,
                Err(e) => return Outcome::gltf_error(e),
            };
            let graph = if case.flags & 1 != 0 {
                let Some(root) = file.objects().filter(|o| o.class_id() == 1).last() else {
                    return Outcome::Error("no prefab root".to_owned());
                };
                graph_from_game_object(env, &root, &GraphOptions::default(), meter)
            } else {
                graph_from_scene_file(env, file, &GraphOptions::default(), meter)
            };
            let graph = match graph {
                Ok(g) => g,
                Err(e) => return Outcome::scene_error(e),
            };
            let mapper: &dyn unityrs_gltf::MaterialMapper = if case.flags & 2 != 0 {
                &CorePbr
            } else {
                &UnlitExtras
            };
            let scene = match collect_with_mapper(env, &graph, &profile, mapper, meter) {
                Ok(s) => s,
                Err(e) => return Outcome::gltf_error(e),
            };
            let mut first = Digest::default();
            if let Err(e) = write_glb(&scene, &profile.options(), meter, &mut first) {
                return Outcome::gltf_error(e);
            }
            let bytes = first.bytes;
            let hash = first.finish();
            let mut second = Digest::default();
            if let Err(e) = write_glb(&scene, &profile.options(), meter, &mut second) {
                return Outcome::gltf_error(e);
            }
            assert_eq!(second.bytes, bytes);
            assert_eq!(
                second.finish(),
                hash,
                "writing the same scene twice changed the GLB"
            );
            Outcome::Done(hash)
        })
    })
}
