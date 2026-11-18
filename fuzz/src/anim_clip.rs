//! Discover, read, bind and sample every clip of a mutated animated scene.
use crate::scene::{Digest, Outcome, anim_refusal};
use std::sync::Arc;
use unityrs_anim::{
    AnimError, Attachment, RendererChannel, RendererChannels, RigClips, RigPaths, bind_clip,
    discover, read_clip, sample_clip,
};
use unityrs_core::{
    ObjectRef,
    limits::{Meter, MeteredVec},
};
use unityrs_scene::{Component, Graph, Trs};

/// The IEEE CRC32 of the bare channel name `bend`, as a mesh's `nameHash`.
const BEND: u32 = crc32(b"bend");

/// Panics on a nondeterministic result or retained operation charge.
/// Flags: 1 selects a prefab graph, 2 filters every renderer's attachment,
/// 4 samples the clip's own schedule instead of its ends and midpoint.
pub fn check(input: &[u8]) -> bool {
    crate::scene::check(input, |case, limit| {
        crate::scene::run_root(case, limit, |env, _, file, meter| {
            let graph = match crate::scene::graph(env, file, case.flags, meter) {
                Ok(g) => g,
                Err(outcome) => return outcome,
            };
            let mut digest = Digest::default();
            match walk(env, &graph, case.flags, meter, &mut digest) {
                Ok(()) => Outcome::Done(digest.finish()),
                Err(e) => Outcome::anim_error(e),
            }
        })
    })
}

fn walk(
    env: &unityrs_core::env::Environment,
    graph: &Graph,
    flags: u8,
    meter: &Arc<Meter>,
    digest: &mut Digest,
) -> Result<(), AnimError> {
    let discovery = discover(env, graph, meter)?;
    let channels = [RendererChannel {
        name: b"bend",
        hash: BEND,
        target_slot: 0,
        frame: 0,
        full_weight: 100.,
    }];
    let mut renderers = MeteredVec::new(meter);
    let mut rest = MeteredVec::with_capacity(meter, graph.nodes().len())?;
    for node in graph.nodes() {
        meter.count_decode(1 + node.components.len() as u64)?;
        rest.push(node.local)?;
        for component in node.components.as_slice() {
            if let Component::SkinnedMeshRenderer(skin) = component {
                renderers.push(RendererChannels {
                    renderer: &skin.renderer.object,
                    attachment: if flags & 2 != 0 {
                        Attachment::Filtered
                    } else {
                        Attachment::Present(&channels)
                    },
                })?;
            }
        }
    }
    for rig in discovery.rigs() {
        meter.count_decode(1)?;
        digest.tag((rig.node, rig.slots().len(), rig.issues().len()));
        digest.tag(rig.controller().is_some());
        let paths = RigPaths::new(graph, rig.node, meter)?;
        for slot in rig.slots() {
            meter.count_decode(1)?;
            digest.tag((slot.is_first(), slot.replacement.is_some()));
            let Ok(object) = slot.clip() else {
                digest.tag("unresolved");
                continue;
            };
            let clip = Clip {
                rig,
                paths: &paths,
                rest: rest.as_slice(),
                renderers: renderers.as_slice(),
                flags,
            };
            if let Err(e) = clip.run(object, meter, digest) {
                if anim_refusal(&e) {
                    return Err(e);
                }
                digest.tag(format!("{e:?}"));
            }
        }
    }
    Ok(())
}

struct Clip<'a> {
    rig: &'a RigClips,
    paths: &'a RigPaths<'a>,
    rest: &'a [Trs],
    renderers: &'a [RendererChannels<'a>],
    flags: u8,
}

impl Clip<'_> {
    fn run(
        &self,
        object: &ObjectRef,
        meter: &Arc<Meter>,
        digest: &mut Digest,
    ) -> Result<(), AnimError> {
        let clip = read_clip(object, meter)?;
        digest.tag(clip.issue().map(|e| format!("{e:?}")));
        digest.tag((clip.has_metadata_tracks(), clip.bindings().is_some()));
        digest.tag(clip.legacy(meter)?);
        digest.tag(clip.endpoint_fields(meter)?);
        let mut keys = 0u64;
        clip.visit_key_times(meter, |time| {
            meter.count_decode(1)?;
            keys = keys.wrapping_add(time.to_bits());
            Ok(())
        })?;
        digest.tag(keys);
        if let Ok(rotation) = clip.packed_rotation(0, meter) {
            digest.tag(format!("{rotation:?}"));
        }
        let bound = bind_clip(&clip, self.paths, &[], self.renderers, meter)?;
        digest.tag((self.rig.node, bound.tracks().len()));
        let Some(timing) = bound.timing() else {
            return Ok(());
        };
        let mut times = MeteredVec::new(meter);
        if self.flags & 4 != 0 {
            times = timing.schedule(None, meter)?;
        } else {
            let (start, stop) = (timing.start(), timing.stop());
            let middle = start + (stop - start) / 2.;
            times.push(start)?;
            for time in [middle, stop] {
                if times.as_slice().last().is_some_and(|&last| time > last) {
                    times.push(time)?;
                }
            }
        }
        let samples = sample_clip(&bound, self.rest, times.as_slice(), meter)?;
        for time in samples.times() {
            digest.tag((time.source.to_bits(), time.local.to_bits()));
        }
        for row in samples.transforms() {
            let pose = row.pose;
            digest.tag(row.node);
            for x in pose
                .position
                .iter()
                .chain(&pose.rotation)
                .chain(&pose.scale)
            {
                digest.tag(x.to_bits());
            }
        }
        for row in samples.morphs() {
            digest.tag((row.node, row.target_slot, row.weight.to_bits()));
        }
        Ok(())
    }
}

const fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    let mut i = 0;
    while i < bytes.len() {
        crc ^= bytes[i] as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
            bit += 1;
        }
        i += 1;
    }
    !crc
}
