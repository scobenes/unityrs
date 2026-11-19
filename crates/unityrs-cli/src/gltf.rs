//! One explicit scene or prefab unit, using the library's two-phase exporter.
mod args;
pub(crate) use args::options;
mod output;
mod report;
use std::{
    path::PathBuf,
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use unityrs_core::{
    budget::Reservation,
    env::{CabResolver, EnvOptions, Environment, Source, read_path},
    files::{File, SerializedFile},
    limits::{Meter, MeteredVec},
    provider::ProviderChain,
};
use unityrs_scene::Unit;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The supplied built-in resources, offered only once a unit is selected.
/// Selection walks the container, whose preload table names the built-ins a
/// bundle's prefabs use; a file loaded under that walk ends it, as UnityPy's
/// pass ends when its CABs grow.
#[derive(Debug)]
struct BuiltinFile {
    path: PathBuf,
    armed: AtomicBool,
}
impl CabResolver for BuiltinFile {
    fn resolve(&self, name: &[u8]) -> Option<Source> {
        if !self.armed.load(Ordering::Acquire) {
            return None;
        }
        let name = name.rsplit(|b| matches!(b, b'/' | b'\\')).next()?;
        if name.eq_ignore_ascii_case(b"unity default resources")
            || name.eq_ignore_ascii_case(b"unity_builtin_extra")
        {
            Some(Source::Path(self.path.clone()))
        } else {
            None
        }
    }
}
fn named(file: &SerializedFile, requested: &[u8]) -> bool {
    file.name().as_bytes() == requested
        || file
            .name()
            .as_bytes()
            .rsplit(|b| matches!(b, b'/' | b'\\'))
            .next()
            == Some(requested)
}
fn scene_candidate(
    selected: &mut Option<Arc<SerializedFile>>,
    file: &Arc<SerializedFile>,
    requested: &[u8],
) -> Result<()> {
    if named(file, requested) {
        if selected.as_ref().is_some_and(|old| !Arc::ptr_eq(old, file)) {
            return Err("ambiguous scene file".into());
        }
        *selected = Some(file.clone());
    }
    Ok(())
}
// Container depth is bounded by the core before entries are exposed. Walking
// file tables also finds empty scenes, which the object iterator cannot yield.
fn scene_in_file(
    selected: &mut Option<Arc<SerializedFile>>,
    file: &File,
    requested: &[u8],
) -> Result<()> {
    match file {
        File::Serialized(file) => scene_candidate(selected, file, requested)?,
        File::Bundle(_) | File::Web(_) => {
            for entry in file.container_files() {
                scene_in_file(selected, &file.container_entry(entry.dir_index)?, requested)?;
            }
        }
        File::Raw(_) => (),
    }
    Ok(())
}

#[derive(Debug)]
struct ContainerCount {
    class: &'static str,
    count: usize,
}
impl std::fmt::Display for ContainerCount {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            out,
            "container path requires exactly one {}; found {} {} entries",
            self.class, self.count, self.class
        )
    }
}
impl std::error::Error for ContainerCount {}

fn container_unit(env: &Environment, requested: &[u8], prefab: bool) -> Result<Unit> {
    let mut selected = None;
    let mut count = 0usize;
    for entry in env.container()? {
        let entry = entry?;
        entry.file.meter().count_decode(requested.len() as u64)?;
        if entry
            .row
            .path
            .as_ref()
            .is_none_or(|p| p.as_bytes() != requested)
        {
            continue;
        }
        let unit = unityrs_scene::unit_from_container(env, &entry.file, &entry.row)?;
        if let Some(unit) = unit.filter(|unit| matches!(unit, Unit::Prefab(_)) == prefab) {
            count = count.checked_add(1).ok_or("container count overflow")?;
            selected = Some(unit);
        }
    }
    if count == 1 {
        return selected.ok_or_else(|| "missing selected container unit".into());
    }
    Err(ContainerCount {
        class: if prefab { "GameObject" } else { "SceneAsset" },
        count,
    }
    .into())
}

fn select(
    env: &Environment,
    args: &args::Args<'_>,
    scene: Option<Arc<SerializedFile>>,
) -> Result<Unit> {
    use args::Selection;
    if let Selection::Prefab(requested) = args.selection {
        return container_unit(env, requested, true);
    }
    if let Selection::Scene(requested) = args.selection {
        return match scene {
            Some(scene) => Ok(Unit::Scene(scene)),
            None => container_unit(env, requested, false),
        };
    }
    let Selection::PathId(id) = args.selection else {
        return Err("missing path id selector".into());
    };
    let mut object = None;
    for item in env.objects() {
        let item = item?;
        if item.path_id() == id
            && matches!(item.class_id(), 1 | 1032)
            && object.replace(item).is_some()
        {
            return Err("ambiguous path id".into());
        }
    }
    Ok(unityrs_scene::unit_from_object(
        &object.ok_or("no GameObject or SceneAsset with that path id")?,
    )?)
}
/// The input Environment, and the built-in resources it holds unarmed.
fn environment(
    args: &args::Args<'_>,
    peak: &Arc<report::Peak>,
) -> Result<(Environment, Option<Arc<BuiltinFile>>)> {
    let mut options = EnvOptions::unitypy();
    let mut context = (*options.context).clone();
    context.limits = args.limits;
    context.budget = peak.clone();
    if let Some(path) = args.tpk {
        context.providers = Arc::new(ProviderChain::new(vec![Arc::new(
            unityrs_tpk::TpkProvider::open_with(path, &args.limits)?,
        )]));
    }
    options.context = Arc::new(context);
    let builtin = args.builtin_file.map(|path| {
        Arc::new(BuiltinFile {
            path: path.to_owned(),
            armed: AtomicBool::new(false),
        })
    });
    if let Some(builtin) = &builtin {
        options.resolver = Some(builtin.clone());
    }
    Ok((Environment::new(options), builtin))
}
fn run(raw: &[Vec<u8>]) -> Result<()> {
    let args = args::Args::parse(raw)?;
    let peak = Arc::new(report::Peak::default());
    let (env, builtin) = environment(&args, &peak)?;
    // Read the profile before loading inputs so only explicit selectors
    // retain file occurrences in argument order, including repeated paths.
    let (profile, mut input_roots) = if let Some(path) = args.profile {
        let meter = Meter::for_input(&args.limits, peak.clone(), Reservation::Input);
        let bytes = read_path(path, &args.limits, &meter)?;
        let profile = unityrs_gltf::Profile::parse(&bytes, &meter)?;
        drop(bytes);
        let roots = if matches!(&profile.clips, unityrs_gltf::Clips::Explicit(rows) if !rows.is_empty())
        {
            Some(MeteredVec::with_capacity(&meter, args.inputs().count())?)
        } else {
            None
        };
        (Some(profile), roots)
    } else {
        (None, None)
    };
    let mut scene = None;
    for input in args.inputs() {
        let file = env.load(Source::Path(input.to_owned()))?;
        if let args::Selection::Scene(requested) = args.selection {
            scene_in_file(&mut scene, &file, requested)?;
        }
        if let Some(roots) = &mut input_roots {
            roots.push(file)?;
        }
    }
    let unit = select(&env, &args, scene)?;
    if let Some(builtin) = &builtin {
        builtin.armed.store(true, Ordering::Release);
    }
    peak.start_export();
    let meter = unit.meter();
    let profile = profile.unwrap_or_else(|| unityrs_gltf::Profile::new(meter));
    let roots = input_roots.as_ref().map_or(&[][..], MeteredVec::as_slice);
    let graph = unit.graph(&env, &unityrs_scene::GraphOptions::default(), meter)?;
    let mapper: &dyn unityrs_gltf::MaterialMapper = if args.pbr {
        &unityrs_gltf::CorePbr
    } else {
        &unityrs_gltf::UnlitExtras
    };
    let collected = unityrs_export::gltf::collect(&env, roots, &graph, &profile, mapper, meter)?;
    if let Some(discovery) = collected.discovery {
        for rig in discovery.rigs() {
            for issue in rig
                .issues()
                .iter()
                .chain(rig.slots().iter().filter_map(|slot| slot.clip().err()))
            {
                eprintln!(
                    "unityrs gltf: rig {} component {} {}[{}]: {}",
                    rig.root.path_id(),
                    rig.component.path_id(),
                    issue.source.field,
                    issue.source.ordinal,
                    issue.cause
                );
            }
        }
    }
    let scene = collected.scene;
    drop(collected.clips);
    let mut outputs = output::Outputs::new(meter);
    let mut written = None;
    outputs.write(args.output, |out| {
        written = Some(unityrs_gltf::write_glb(
            &scene,
            &profile.options(),
            meter,
            out,
        )?);
        Ok(())
    })?;
    let written = written.ok_or("missing writer report")?;
    if let Some(path) = args.report {
        outputs.write(path, |out| {
            report::write(out, &written, &graph, peak.peaks(), meter)
        })?;
    }
    outputs.finish()?;
    eprintln!(
        "unityrs gltf: {} collection issues, {} primitive/attribute issues, {} graph issues; peak charged bytes {}",
        written.collection_issues.len(),
        written.issues.len(),
        graph.issues().len(),
        peak.peak()
    );
    Ok(())
}
pub(super) fn main(raw: &[Vec<u8>]) -> ExitCode {
    if matches!(raw,[a] if a==b"-h" || a==b"--help") {
        println!("{}", args::USAGE);
        return ExitCode::SUCCESS;
    }
    match run(raw) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}\nunityrs gltf: {}", args::USAGE, e);
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;
    use unityrs_testkit::{ngui::Object, scene3d as s, scratch::Scratch};

    #[test]
    fn empty_clip_profile_does_not_allocate_an_input_root_list() {
        let scratch = Scratch::new("gltf-static-input-allocation");
        let resource = scratch.join("r.resource");
        let prefab = scratch.join("level0");
        let profile = scratch.join("static.json");
        let output = scratch.join("result.glb");
        std::fs::write(&resource, b"resource").unwrap();
        let bytes = s::file(
            s::Layout::Unity2022,
            &[],
            &[
                Object::new(1, 1, s::game_object(b"root", &[(0, 2)])),
                Object::new(2, 4, s::transform((0, 1), (0, 0), &[], false)),
            ],
        )
        .unwrap();
        std::fs::write(&prefab, bytes).unwrap();
        std::fs::write(&profile, br#"{"profile_version":1,"clips":[]}"#).unwrap();

        let padding = 8192;
        let raw_path = |path: &std::path::Path| path.as_os_str().as_bytes().to_vec();
        let mut args = vec![raw_path(&prefab)];
        args.extend(std::iter::repeat_n(raw_path(&resource), padding));
        args.extend([
            b"--path-id".to_vec(),
            b"1".to_vec(),
            b"--profile".to_vec(),
            raw_path(&profile),
            b"-o".to_vec(),
            raw_path(&output),
        ]);
        crate::test_alloc::LARGEST.set(Some(0));
        let result = run(&args);
        let largest = crate::test_alloc::LARGEST.replace(None).unwrap();
        result.unwrap();
        assert!(
            largest < (padding + 1) * size_of::<File>(),
            "largest allocation was {largest} bytes"
        );
    }
}
