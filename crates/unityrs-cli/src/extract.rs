//! `extract` uses one Environment for INPUT..., writes a JSON report on
//! stdout (and optionally beneath -o), and continues after object failures.
mod files;
mod limits;
mod report;
use std::{
    ffi::OsStr, io::Write, os::unix::ffi::OsStrExt, path::Path, process::ExitCode, sync::Arc,
};
use unityrs_core::{
    ObjectRef, Parsed,
    env::{EnvOptions, Environment, Source},
    limits::MeteredVec,
    provider::ProviderChain,
};
use unityrs_export::extractor::{self, AssetFilter, Options, Sink};
const USAGE: &str = "usage: unityrs extract INPUT... -o DIR [options]\n\
  --no-container                       export every object by name\n\
  --ignore-first-container-dirs N       drop N slash components (default 0)\n\
  --append-path-id                     append UnityPy's _None suffix\n\
  --export-unknown-as-typetree          JSON or raw .bin for other classes\n\
  --asset-filter CLASS                 class name or integer id; repeat to include more\n\
  --tpk FILE                          schema provider for stripped files\n\
  --vorbis-headers FILE                runtime VST1 setup-header table; no default, never fetched\n\
  --report PATH                       also save JSON at this relative path beneath DIR\n\
  --max-input-memory N                 charged bytes per input and extraction budget (default 8589934592)\n\
  --max-input-bytes N                  source bytes per input (default 4294967296)\n\
  --max-decode-work N                  work bytes per input and extraction (default 34359738368)\n\
  --max-tree-depth N                   graph/tree depth (default 256)\n\
Writes a JSON report to stdout, including overwritten writes and failures.\n\
Exit 0: extraction completed, including reported object failures. Exit 1: invalid arguments,\n\
input/load, budget, traversal, output root or report failure prevents completion (as gltf).\n\
Later writes replace earlier leaves. Parent symlinks and unsafe names are refused.\n\
Writes are confined to directory inodes reached from the held -o descriptor by\n\
safe relative opens, each against its held parent, without symlinks or .. .\n\
Confinement is established on open, not against later pathnames: a concurrent\n\
rename moves a directory and its files together; held-descriptor writes continue\n\
in that inode, even outside the -o pathname, as with a rename after extraction.\n\
Interrupted runs may leave .unityrs-extract-*.tmp files, safe to delete.";
struct Filter(MeteredVec<i32>);
impl AssetFilter for Filter {
    fn accepts(&self, object: &ObjectRef, _: Option<&Parsed>) -> bool {
        self.0.as_slice().contains(&object.class_id())
    }
}
fn path(bytes: &[u8]) -> &Path {
    Path::new(OsStr::from_bytes(bytes))
}
fn run(raw: &[Vec<u8>]) -> Result<(), Box<dyn std::error::Error>> {
    let limits = limits::parse(raw)?;
    let meter = limits::extraction(&limits);
    let mut inputs = MeteredVec::new(&meter);
    let mut output = None;
    let mut tpk = None;
    let mut vorbis_headers = None;
    let mut report_path = None;
    let mut options = Options::default();
    let mut filter = Filter(MeteredVec::new(&meter));
    let mut args = raw.iter();
    let mut positional = false;
    while let Some(arg) = args.next() {
        if positional {
            inputs.push(path(arg))?;
            continue;
        }
        match arg.as_slice() {
            b"--" => positional = true,
            b"-o" => output = Some(path(args.next().ok_or("missing DIR")?)),
            b"--tpk" => tpk = Some(path(args.next().ok_or("missing TPK")?)),
            b"--vorbis-headers" => {
                vorbis_headers = Some(path(args.next().ok_or("missing Vorbis headers FILE")?));
            }
            b"--report" => report_path = Some(args.next().ok_or("missing report PATH")?.as_slice()),
            b"--max-input-memory"
            | b"--max-input-bytes"
            | b"--max-decode-work"
            | b"--max-tree-depth" => {
                args.next().ok_or("missing limit")?;
            }
            b"--no-container" | b"--no-use-container" => options.use_container = false,
            b"--use-container" => options.use_container = true,
            b"--append-path-id" => options.append_path_id = true,
            b"--export-unknown-as-typetree" => options.export_unknown_as_typetree = true,
            b"--ignore-first-container-dirs" => {
                options.ignore_first_container_dirs =
                    std::str::from_utf8(args.next().ok_or("missing N")?)?.parse()?;
            }
            b"--asset-filter" => {
                let value = std::str::from_utf8(args.next().ok_or("missing CLASS")?)?;
                let id = value
                    .parse::<i32>()
                    .ok()
                    .or_else(|| {
                        unityrs_core::classid::CLASS_NAMES
                            .iter()
                            .find(|(_, n)| *n == value)
                            .map(|(id, _)| *id)
                    })
                    .ok_or("unknown CLASS")?;
                filter.0.push(id)?;
            }
            _ if !arg.starts_with(b"-") => inputs.push(path(arg))?,
            _ => return Err("unexpected argument".into()),
        }
    }
    let output = output.ok_or("missing -o DIR")?;
    if inputs.is_empty() {
        return Err("missing INPUT".into());
    }
    if let Some(path) = report_path {
        extractor::paths::validate(path)?;
    }
    if !filter.0.is_empty() {
        options.asset_filter = Some(&filter);
    }
    let headers = vorbis_headers
        .map(|path| unityrs_export::VorbisHeaders::load(path, &meter))
        .transpose()?;
    options.vorbis = headers.as_ref();
    let mut env_options = EnvOptions::unitypy();
    let mut context = (*env_options.context).clone();
    context.limits = limits;
    if let Some(tpk) = tpk {
        context.providers = Arc::new(ProviderChain::new(vec![Arc::new(
            unityrs_tpk::TpkProvider::open_with(tpk, &limits)?,
        )]));
    }
    env_options.context = Arc::new(context);
    let env = Environment::new(env_options);
    for input in inputs.as_slice() {
        env.load(Source::Path((*input).to_owned()))?;
    }
    let mut files = files::Files::new(output, &meter)?;
    let report = report::checked(extractor::extract_assets(
        &env, &options, &mut files, &meter,
    )?)?;
    let json = report::json(&report, &meter)?;
    if let Some(path) = report_path {
        files.file(path, json.as_bytes())?;
    }
    std::io::stdout().lock().write_all(json.as_bytes())?;
    Ok(())
}
pub(super) fn main(raw: &[Vec<u8>]) -> ExitCode {
    if matches!(raw, [a] if a == b"-h" || a == b"--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    match run(raw) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{USAGE}\nunityrs extract: {e}");
            ExitCode::from(1)
        }
    }
}
