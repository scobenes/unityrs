//! Same-built-bundle generic and legacy replay. See `docs/anim_reference.md`.
#![cfg(feature = "gltf")]
#![allow(clippy::float_cmp)]
mod anim_support;
mod anim_replay {
    pub mod clock;
    pub mod fixture;
    pub mod ranges;
    pub mod zero_origin;
}
use anim_replay::{clock, ranges};
use serde_json::Value as J;
use sha2::{Digest, Sha256};
use std::{fmt::Write as _, path::Path, process::Command, sync::Arc};
use unityrs_anim::{
    Attachment, BindingTarget, BoundClip, BoundTrack, LocalTrs, RendererChannel, RendererChannels,
    RigPaths, TransformProperty, bind_clip, discover, read_clip, sample_clip,
};
use unityrs_core::{
    ObjectRef,
    env::{EnvOptions, Environment, FileOccurrence, PPtr, Source},
    files::{File, SerializedFile},
    limits::Meter,
};
use unityrs_scene::{Component, Graph, GraphOptions, PPtrTarget, Trs, Unit};
use unityrs_testkit::child;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
type Files = Vec<Vec<Option<Arc<SerializedFile>>>>;

fn require(ok: bool, message: &'static str) -> Result<()> {
    if ok { Ok(()) } else { Err(message.into()) }
}

fn number(row: &J) -> Result<f64> {
    row.as_f64()
        .filter(|x| x.is_finite())
        .ok_or_else(|| "expected a finite number".into())
}

fn index(row: &J) -> Result<usize> {
    Ok(usize::try_from(row.as_u64().ok_or("expected index")?)?)
}

fn string(row: &J) -> Result<&str> {
    row.as_str().ok_or_else(|| "expected text".into())
}

fn list(row: &J) -> Result<&[J]> {
    row.as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| "expected array".into())
}

fn f32_bits(row: &J) -> Result<f64> {
    let value = f32::from_bits(u32::from_str_radix(string(row)?, 16)?);
    require(value.is_finite(), "nonfinite captured component")?;
    Ok(f64::from(value))
}

fn same(a: &ObjectRef, b: &ObjectRef) -> bool {
    a.path_id() == b.path_id() && FileOccurrence::of(a.file()) == FileOccurrence::of(b.file())
}

fn object(files: &Files, id: &J) -> Result<ObjectRef> {
    let file = files
        .get(index(&id["bundle"])?)
        .and_then(|files| files.get(index(&id["file_index"]).ok()?))
        .and_then(Option::as_ref)
        .ok_or("missing recorded file occurrence")?;
    require(
        file.name().as_bytes() == string(&id["file"])?.as_bytes(),
        "recorded file name",
    )?;
    file.object(string(&id["path_id"])?.parse()?)
        .ok_or_else(|| "missing recorded object".into())
}

fn address(graph: &Graph, path: &J) -> Result<usize> {
    let mut node = *graph.roots().first().ok_or("missing prefab root")?;
    let path = string(path)?;
    if !path.is_empty() {
        for child in path.split('/') {
            node = *graph.nodes()[node]
                .children
                .as_slice()
                .get(child.parse::<usize>()?)
                .ok_or("missing child at recorded address")?;
        }
    }
    Ok(node)
}

fn check_asset(env: &Environment, key: &J, asset: &ObjectRef) -> Result<()> {
    let key = string(key)?.as_bytes();
    let mut hits = 0;
    for item in env.container()? {
        let item = item?;
        if item
            .row
            .path
            .as_ref()
            .is_some_and(|path| path.as_bytes() == key)
        {
            let target = env.deref(&item.file, item.row.asset().ok_or("invalid asset pointer")?)?;
            if target.class_id() == asset.class_id() {
                require(same(asset, &target), "bundle asset identity")?;
                hits += 1;
            }
        }
    }
    require(hits == 1, "bundle asset key has no unique object")
}

fn digest(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(text, "{byte:02x}").unwrap();
    }
    text
}

fn load(spec: &J, dir: &Path) -> Result<(Environment, Files)> {
    let env = Environment::new(EnvOptions::unitypy());
    let mut files = Vec::new();
    for bundle in list(&spec["bundles"])? {
        let name = string(&bundle["name"])?;
        require(
            Path::new(name).file_name().is_some_and(|base| base == name),
            "invalid bundle basename",
        )?;
        let pin = string(&bundle["sha256"])?;
        let root = env.load_observed(Source::Path(dir.join(name)), |bytes, meter| {
            meter.count_decode(bytes.len() as u64)?;
            if digest(bytes) != pin {
                return Err(unityrs_core::Error::Malformed {
                    what: "replay bundle",
                    detail: "bundle digest mismatch".into(),
                });
            }
            Ok(())
        })?;
        let mut entries = Vec::new();
        unityrs_export::gltf::walk_files(root.clone(), root.meter(), |file| -> Result<()> {
            entries.push(if let File::Serialized(file) = file {
                Some(file.clone())
            } else {
                None
            });
            Ok(())
        })?;
        files.push(entries);
    }
    Ok((env, files))
}

struct Renderer<'a> {
    object: &'a ObjectRef,
    node: usize,
    mesh: unityrs_mesh::Mesh,
    rest: &'a [f32],
}

fn renderer<'a>(
    env: &Environment,
    files: &Files,
    graph: &'a Graph,
    spec: &J,
    meter: &Arc<Meter>,
) -> Result<Renderer<'a>> {
    let node = address(graph, &spec["address"])?;
    let skin = graph.nodes()[node]
        .components
        .as_slice()
        .iter()
        .filter_map(|component| {
            if let Component::SkinnedMeshRenderer(skin) = component {
                Some(skin)
            } else {
                None
            }
        })
        .nth(index(&spec["component"])?)
        .ok_or("missing recorded renderer component")?;
    require(
        same(&skin.renderer.object, &object(files, &spec["id"])?),
        "renderer identity",
    )?;
    let Some(PPtrTarget::Object(mesh)) = &skin.mesh else {
        return Err("unresolved recorded mesh".into());
    };
    require(same(mesh, &object(files, &spec["mesh"])?), "mesh identity")?;
    let mesh = unityrs_mesh::read_mesh_for_gltf(mesh, env, meter)?;
    let channels = mesh.blend_shapes.channels.as_slice();
    let rows = list(&spec["channels"])?;
    require(rows.len() == channels.len(), "renderer channel coverage")?;
    for (i, (row, channel)) in rows.iter().zip(channels).enumerate() {
        require(
            index(&row["index"])? == i
                && string(&row["name"])?.as_bytes() == channel.name.as_bytes(),
            "renderer channel identity",
        )?;
        let start = usize::try_from(channel.frame_index)?;
        let stop = start
            .checked_add(usize::try_from(channel.frame_count)?)
            .ok_or("frame overflow")?;
        let weights = mesh
            .blend_shapes
            .full_weights
            .as_slice()
            .get(start..stop)
            .ok_or("frame range")?;
        require(
            weights
                .iter()
                .copied()
                .map(f64::from)
                .eq(list(&row["full_weights"])?
                    .iter()
                    .map(number)
                    .collect::<Result<Vec<_>>>()?),
            "renderer frame weights",
        )?;
    }
    Ok(Renderer {
        object: &skin.renderer.object,
        node,
        mesh,
        rest: skin.blend_weights.as_slice(),
    })
}

fn derive(manifest: &Path, dir: &Path) -> Result<J> {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/parity/anim_replay.py");
    let output = child::output(
        Command::new("python3")
            .args(["-I"])
            .arg(script)
            .arg("derive")
            .arg(manifest)
            .arg("--bundle-dir")
            .arg(dir),
    )?;
    if !output.status.success() {
        return Err(format!(
            "replay validator: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn import_settings(spec: &J, clip: &unityrs_anim::Clip) -> Result<()> {
    use unityrs_core::value::Value;
    for setting in list(&spec["evidence"]["import_settings"])? {
        let path = string(&setting["serialized_path"])?
            .split('.')
            .map(str::as_bytes)
            .collect::<Vec<_>>();
        let actual = clip.serialized().get(&path);
        let recorded = &setting["serialized"];
        let same = match actual {
            None => recorded.is_null(),
            Some(Value::Bool(value)) => recorded.as_bool() == Some(*value),
            Some(Value::F32(_) | Value::F64(_)) => {
                recorded.as_f64() == actual.and_then(Value::as_f64)
            }
            Some(Value::Int(value)) => recorded.as_i64() == Some(*value),
            Some(Value::UInt(value)) => recorded.as_u64() == Some(*value),
            Some(Value::Str(value)) => recorded
                .as_str()
                .is_some_and(|s| s.as_bytes() == value.as_bytes()),
            _ => false,
        };
        require(same, "bundle serialized import setting")?;
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn compare(document: &J, dir: &Path, tolerance: &J) -> Result<()> {
    let spec = &document["spec"];
    let capture = &document["capture"];
    require(
        spec["evidence"].get("humanoid").is_none()
            && capture.get("humanoid").is_none()
            && spec["mode"] != "human_pose",
        "unsupported_humanoid_playback",
    )?;
    let (env, files) = load(spec, dir)?;
    let prefab = object(&files, &spec["prefab"])?;
    let clip_object = object(&files, &spec["clip"])?;
    check_asset(&env, &spec["prefab_asset"], &prefab)?;
    check_asset(&env, &spec["clip_asset"], &clip_object)?;
    let unit = Unit::Prefab(prefab);
    let meter = unit.meter();
    let graph = unit.graph(&env, &GraphOptions::default(), meter)?;
    let nodes = list(&spec["nodes"])?
        .iter()
        .map(|row| {
            let node = address(&graph, &row["address"])?;
            let actual = &graph.nodes()[node];
            require(
                same(&actual.transform, &object(&files, &row["id"])?)
                    && actual.name.as_bytes() == string(&row["name"])?.as_bytes(),
                "node identity",
            )?;
            Ok(node)
        })
        .collect::<Result<Vec<_>>>()?;
    require(nodes.len() == graph.nodes().len(), "node coverage")?;
    let renderers = list(&spec["renderers"])?
        .iter()
        .map(|row| renderer(&env, &files, &graph, row, meter))
        .collect::<Result<Vec<_>>>()?;
    let renderer_count = graph
        .nodes()
        .iter()
        .flat_map(|node| node.components.as_slice())
        .filter(|c| matches!(c, Component::SkinnedMeshRenderer(_)))
        .count();
    require(renderers.len() == renderer_count, "renderer coverage")?;
    let facts = renderers
        .iter()
        .map(|renderer| {
            renderer
                .mesh
                .blend_shapes
                .channels
                .as_slice()
                .iter()
                .enumerate()
                .map(|(slot, row)| {
                    let frame = usize::try_from(row.frame_index + row.frame_count - 1)?;
                    Ok(RendererChannel {
                        name: row.name.as_bytes(),
                        hash: row.name_hash,
                        target_slot: slot,
                        frame,
                        full_weight: renderer.mesh.blend_shapes.full_weights.as_slice()[frame],
                    })
                })
                .collect::<Result<Vec<_>>>()
        })
        .collect::<Result<Vec<_>>>()?;
    let channels = renderers
        .iter()
        .zip(&facts)
        .map(|(renderer, channels)| RendererChannels {
            renderer: renderer.object,
            attachment: Attachment::Present(channels),
        })
        .collect::<Vec<_>>();
    let discovery = discover(&env, &graph, meter)?;
    let binding = object(&files, &spec["binding"])?;
    let binding_node = address(&graph, &spec["binding_address"])?;
    let class = if spec["mode"] == "legacy" { 111 } else { 95 };
    let rig = discovery
        .rigs()
        .iter()
        .filter(|rig| rig.node == binding_node && rig.component.class_id() == class)
        .nth(index(&spec["binding_component"])?)
        .ok_or("missing recorded binding")?;
    require(same(&rig.component, &binding), "binding identity")?;
    if !spec["avatar"].is_null() {
        let avatar = object(&files, &spec["avatar"])?;
        check_asset(&env, &spec["avatar_asset"], &avatar)?;
        let parsed = binding.parse_as_dict(true)?;
        let ptr = PPtr {
            file_id: i32::try_from(
                parsed
                    .get(&[b"m_Avatar", b"m_FileID"])
                    .and_then(unityrs_core::value::Value::as_i64)
                    .ok_or("Avatar file ID")?,
            )?,
            path_id: parsed
                .get(&[b"m_Avatar", b"m_PathID"])
                .and_then(unityrs_core::value::Value::as_i64)
                .ok_or("Avatar path ID")?,
        };
        require(
            same(&env.deref(binding.file(), ptr)?, &avatar),
            "Avatar identity",
        )?;
    }
    discovery
        .select(
            &rig.root,
            &rig.component,
            unityrs_anim::ClipSelector::Clip(&clip_object),
            meter,
        )
        .map_err(|error| format!("replay clip selection: {error:?}"))?;
    let clip = read_clip(&clip_object, meter)?;
    import_settings(spec, &clip)?;
    require(
        clip.legacy(meter)? == (spec["mode"] == "legacy"),
        "serialized clip playback mode",
    )?;
    require(
        clip.serialized()
            .get(&[b"m_Name"])
            .and_then(unityrs_core::value::Value::as_str)
            == spec["clip_name"].as_str(),
        "recorded clip name",
    )?;
    let paths = RigPaths::new(&graph, rig.node, meter)?;
    let bound = bind_clip(&clip, &paths, &[], &channels, meter)?;
    let rest = graph
        .nodes()
        .iter()
        .map(|node| node.local)
        .collect::<Vec<_>>();
    let targets = targets(spec, &nodes, &renderers, &bound, &rest)?;
    let times = clock::check(spec, capture, tolerance, &bound, &targets, meter)?;
    for target in &targets {
        ranges::check(
            target,
            bound.timing().ok_or("missing clip interval")?,
            meter,
        )?;
    }
    let expected = list(&capture["samples"])?;
    let tolerances = list(&tolerance["samples"])?;
    require(
        expected.len() == times.len() && tolerances.len() == times.len(),
        "sample coverage",
    )?;
    require(
        list(&tolerance["targets"])? == targets.iter().map(|t| t.id.clone()).collect::<Vec<_>>(),
        "frozen tolerance target order",
    )?;
    for target in &targets {
        require(
            target.error(&target.rest, &target.reference(&capture["rest"])?)? <= 0.,
            "serialized rest pose mismatch",
        )?;
    }
    let mut worst = vec![(0., times[0]); targets.len()];
    let mut failed = None;
    let mut replay = None;
    for (row, (&time, observed)) in times.iter().zip(expected).enumerate() {
        if replay
            .as_ref()
            .is_none_or(|(previous, _)| *previous != time)
        {
            replay = Some((time, sample_clip(&bound, &rest, &[time], meter)?));
        }
        let (_, samples) = replay.as_ref().unwrap();
        let bounds = list(&tolerances[row]["bounds"])?;
        require(bounds.len() == targets.len(), "tolerance coverage")?;
        for (i, target) in targets.iter().enumerate() {
            let actual = target.sampled(samples.transforms(), samples.morphs());
            let reference = target.reference(&observed["pose"])?;
            let error = target.error(&actual, &reference)?;
            let tolerance = number(&bounds[i])?;
            let excess = error - tolerance;
            let within_tolerance = error <= tolerance;
            if !within_tolerance
                && failed.is_none_or(|(worst, _, _, _, _)| excess.total_cmp(&worst).is_gt())
            {
                failed = Some((excess, i, time, error, tolerance));
            }
            if error > worst[i].0 {
                worst[i] = (error, time);
            }
        }
    }
    for (target, (error, time)) in targets.iter().zip(worst) {
        eprintln!(
            "rig {} {} {} source {time}: worst error {error}",
            rig.root.path_id(),
            target.label,
            target.property()
        );
    }
    if let Some((_, i, time, error, tolerance)) = failed {
        return Err(format!(
            "rig {} {} {} source {time}: {error} > {tolerance}",
            rig.root.path_id(),
            targets[i].label,
            targets[i].property()
        )
        .into());
    }
    Ok(())
}

struct Target<'a, 'c, 'r> {
    id: J,
    label: String,
    budget: &'a J,
    node: usize,
    channel: Option<usize>,
    rest: Vec<f64>,
    tracks: Vec<&'a BoundTrack<'c, 'r>>,
}

impl Target<'_, '_, '_> {
    fn property(&self) -> &str {
        self.id[0].as_str().unwrap()
    }
    fn observed<'a>(&self, pose: &'a J) -> Result<&'a [J]> {
        let index = index(&self.id[1])?;
        if let Some(channel) = self.channel {
            let weights = list(&pose["weights"][index]["channels"])?;
            return weights
                .get(channel..channel + 1)
                .ok_or_else(|| "weight coverage".into());
        }
        list(&pose["transforms"][index][self.property()])
    }

    fn reference(&self, pose: &J) -> Result<Vec<f64>> {
        self.observed(pose)?.iter().map(f32_bits).collect()
    }

    fn error(&self, sampled: &[f64], reference: &[f64]) -> Result<f64> {
        require(sampled.len() == reference.len(), "pose arity")?;
        if self.property() == "rotation" {
            require(
                reference.iter().any(|&x| x != 0.),
                "zero-norm captured quaternion",
            )?;
            return Ok(angle(sampled, reference));
        }
        Ok(sampled
            .iter()
            .zip(reference)
            .map(|(a, b)| (a - b).abs())
            .fold(0., f64::max))
    }

    fn sampled(
        &self,
        transforms: &[unityrs_anim::TransformSample],
        morphs: &[unityrs_anim::MorphSample],
    ) -> Vec<f64> {
        if let Some(channel) = self.channel {
            return morphs
                .iter()
                .find(|m| m.node == self.node && m.target_slot == channel)
                .map_or_else(|| self.rest.clone(), |m| vec![m.weight]);
        }
        transforms.iter().find(|s| s.node == self.node).map_or_else(
            || self.rest.clone(),
            |s| {
                let LocalTrs {
                    position,
                    rotation,
                    scale,
                } = s.pose;
                match self.property() {
                    "translation" => position.to_vec(),
                    "rotation" => rotation.to_vec(),
                    _ => scale.to_vec(),
                }
            },
        )
    }
}

fn angle(a: &[f64], b: &[f64]) -> f64 {
    let norm = |v: &[f64]| v.iter().map(|x| x * x).sum::<f64>().sqrt();
    let (an, bn) = (norm(a), norm(b));
    let sign = if a.iter().zip(b).map(|(a, b)| a * b).sum::<f64>() < 0. {
        -1.
    } else {
        1.
    };
    let mut lo = 0.;
    let mut hi = 0.;
    for (&a, &b) in a.iter().zip(b) {
        lo += (a / an - sign * b / bn).powi(2);
        hi += (a / an + sign * b / bn).powi(2);
    }
    4. * lo.sqrt().atan2(hi.sqrt())
}

fn targets<'a, 'c, 'r>(
    spec: &'a J,
    nodes: &[usize],
    renderers: &[Renderer<'_>],
    bound: &'a BoundClip<'c, 'r>,
    rest: &[Trs],
) -> Result<Vec<Target<'a, 'c, 'r>>> {
    let budgets = list(&spec["budgets"])?;
    let mut targets = Vec::new();
    let mut target =
        |prop: &str, i: usize, channel: Option<usize>, node, rest, label, tracks| -> Result<()> {
            let channel_id = channel.map_or(-1, |c| i64::try_from(c).unwrap());
            let budget = budgets
                .iter()
                .find(|row| {
                    row["property"] == prop && row["target"] == i && row["channel"] == channel_id
                })
                .ok_or("missing playback budget")?;
            targets.push(Target {
                id: serde_json::json!([prop, i, channel_id]),
                label,
                budget,
                node,
                channel,
                rest,
                tracks,
            });
            Ok(())
        };
    for (index, &node) in nodes.iter().enumerate() {
        for (property, values) in [
            ("translation", rest[node].position.map(f64::from).to_vec()),
            ("rotation", rest[node].rotation.map(f64::from).to_vec()),
            ("scale", rest[node].scale.map(f64::from).to_vec()),
        ] {
            let tracks = bound.tracks().iter().filter(|track| {
                matches!(track.target, BindingTarget::Transform { node: owner, property: kind }
                    if owner == node && match kind {
                        TransformProperty::Translation => property == "translation",
                        TransformProperty::Quaternion | TransformProperty::Euler => property == "rotation",
                        TransformProperty::Scale => property == "scale",
                    })
            }).collect();
            target(
                property,
                index,
                None,
                node,
                values,
                format!("node {}", spec["nodes"][index]["address"]),
                tracks,
            )?;
        }
    }
    for (index, renderer) in renderers.iter().enumerate() {
        for (slot, channel) in renderer
            .mesh
            .blend_shapes
            .channels
            .as_slice()
            .iter()
            .enumerate()
        {
            let tracks = bound
                .tracks()
                .iter()
                .filter(|track| {
                    matches!(track.target, BindingTarget::Morph { renderer: owner, channel, .. }
                    if same(owner,renderer.object) && channel.target_slot == slot)
                })
                .collect();
            target(
                "weight",
                index,
                Some(slot),
                renderer.node,
                vec![f64::from(renderer.rest.get(slot).copied().unwrap_or(0.))],
                format!(
                    "renderer {} {}",
                    renderer.object.path_id(),
                    String::from_utf8_lossy(channel.name.as_bytes())
                ),
                tracks,
            )?;
        }
    }
    Ok(targets)
}

#[test]
#[ignore = "owner-built Editor replay manifests and frozen tolerances are required"]
fn recorded_generic_or_legacy() {
    let path = std::env::var("UNITYRS_ANIM_REPLAY").unwrap();
    let dir = std::env::var("UNITYRS_ANIM_BUNDLES").unwrap();
    let frozen = std::env::var("UNITYRS_ANIM_TOLERANCES").unwrap();
    let manifest = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let tolerance = serde_json::from_slice(&std::fs::read(frozen).unwrap()).unwrap();
    let derived = derive(Path::new(&path), Path::new(&dir)).unwrap();
    assert_eq!(
        derived, tolerance,
        "frozen tolerances differ from replay derivation"
    );
    compare(&manifest, Path::new(&dir), &tolerance).unwrap();
}
