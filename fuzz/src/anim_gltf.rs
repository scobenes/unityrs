//! Collect, bake and stream a mutated animated scene's GLB twice.
use crate::scene::{Digest, Outcome};
use std::slice;
use unityrs_gltf::{CorePbr, Profile, UnlitExtras, write_glb};

/// Panics on a nondeterministic result or retained operation charge.
/// Flags: 1 selects a prefab graph, 2 the `CorePbr` mapper.
pub fn check(input: &[u8]) -> bool {
    crate::scene::check(input, |case, limit| {
        crate::scene::run_root(case, limit, |env, root, file, meter| {
            let profile = if case.profile.is_empty() {
                Ok(Profile::new(meter))
            } else {
                Profile::parse(&case.profile, meter)
            };
            let profile = match profile {
                Ok(p) => p,
                Err(e) => return Outcome::gltf_error(e),
            };
            let graph = match crate::scene::graph(env, file, case.flags, meter) {
                Ok(g) => g,
                Err(outcome) => return outcome,
            };
            let mapper: &dyn unityrs_gltf::MaterialMapper = if case.flags & 2 != 0 {
                &CorePbr
            } else {
                &UnlitExtras
            };
            let collected = match unityrs_export::gltf::collect(
                env,
                slice::from_ref(root),
                &graph,
                &profile,
                mapper,
                meter,
            ) {
                Ok(c) => c,
                Err(e) => return Outcome::export_error(e),
            };
            let mut first = Digest::default();
            first.tag(collected.clips.len());
            first.tag(collected.discovery.as_ref().map(|d| d.rigs().len()));
            if let Err(e) = write_glb(&collected.scene, &profile.options(), meter, &mut first) {
                return Outcome::gltf_error(e);
            }
            let bytes = first.bytes;
            let hash = first.finish();
            let mut second = Digest::default();
            second.tag(collected.clips.len());
            second.tag(collected.discovery.as_ref().map(|d| d.rigs().len()));
            if let Err(e) = write_glb(&collected.scene, &profile.options(), meter, &mut second) {
                return Outcome::gltf_error(e);
            }
            assert_eq!(second.bytes, bytes);
            assert_eq!(
                second.finish(),
                hash,
                "writing the same animated scene twice changed the GLB"
            );
            Outcome::Done(hash)
        })
    })
}
