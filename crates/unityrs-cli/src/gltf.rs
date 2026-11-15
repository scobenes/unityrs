//! One explicit scene or prefab unit, using the library's two-phase exporter.
mod args;
mod output;
mod report;
use std::{path::PathBuf, process::ExitCode, sync::Arc};
use unityrs_core::{
    env::{CabResolver, EnvOptions, Environment, Source, read_path},
    files::{File, SerializedFile},
    provider::ProviderChain,
};
use unityrs_scene::Unit;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug)]
struct BuiltinFile(PathBuf);
impl CabResolver for BuiltinFile {
    fn resolve(&self, name: &[u8]) -> Option<Source> {
        let name = name.rsplit(|b| matches!(b, b'/' | b'\\')).next()?;
        if name.eq_ignore_ascii_case(b"unity default resources")
            || name.eq_ignore_ascii_case(b"unity_builtin_extra")
        {
            Some(Source::Path(self.0.clone()))
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
fn select(
    env: &Environment,
    args: &args::Args<'_>,
    scene: Option<Arc<SerializedFile>>,
) -> Result<Unit> {
    use args::Selection;
    if let Selection::Prefab(requested) = args.selection {
        let mut selected = None;
        for entry in env.container()? {
            let entry = entry?;
            if entry
                .row
                .path
                .as_ref()
                .is_some_and(|p| p.as_bytes() == requested)
            {
                let unit = unityrs_scene::unit_from_container(env, &entry.file, &entry.row)?
                    .ok_or("container entry is not a prefab or SceneAsset")?;
                if selected.replace(unit).is_some() {
                    return Err("ambiguous prefab container path".into());
                }
            }
        }
        return selected.ok_or_else(|| "no prefab container entry with that path".into());
    }
    if matches!(args.selection, Selection::Scene(_)) {
        return scene
            .map(Unit::Scene)
            .ok_or_else(|| "no scene file with that name".into());
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
fn run(raw: &[Vec<u8>]) -> Result<()> {
    let args = args::Args::parse(raw)?;
    let peak = Arc::new(report::Peak::default());
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
    if let Some(path) = args.builtin_file {
        options.resolver = Some(Arc::new(BuiltinFile(path.to_owned())));
    }
    let env = Environment::new(options);
    let mut scene = None;
    for input in args.inputs() {
        let file = env.load(Source::Path(input.to_owned()))?;
        if let args::Selection::Scene(requested) = args.selection {
            scene_in_file(&mut scene, &file, requested)?;
        }
    }
    let unit = select(&env, &args, scene)?;
    let meter = unit.meter();
    let profile = if let Some(path) = args.profile {
        let bytes = read_path(path, &args.limits, meter)?;
        unityrs_gltf::Profile::parse(&bytes, meter)?
    } else {
        unityrs_gltf::Profile::new(meter)
    };
    let graph = unit.graph(&env, &unityrs_scene::GraphOptions::default(), meter)?;
    let mapper: &dyn unityrs_gltf::MaterialMapper = if args.pbr {
        &unityrs_gltf::CorePbr
    } else {
        &unityrs_gltf::UnlitExtras
    };
    let scene = unityrs_gltf::collect_with_mapper(&env, &graph, &profile, mapper, meter)?;
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
            report::write(out, &written, &graph, peak.peak(), meter)
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
