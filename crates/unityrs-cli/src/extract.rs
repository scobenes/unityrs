//! `extract` uses one Environment for INPUT..., writes a JSON report on
//! stdout (and optionally beneath -o), and continues after object failures.
pub(crate) mod files;
pub(crate) mod limits;
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

use crate::args::option;
pub(crate) const OPTIONS: &[crate::agent::options::OptionSpec] = &[
    option("-o", Some("DIR"), None, "Required output directory."),
    option(
        "--no-container",
        None,
        Some("false"),
        "Export every object by name.",
    ),
    option(
        "--no-use-container",
        None,
        Some("false"),
        "Alias for --no-container.",
    ),
    option(
        "--use-container",
        None,
        Some("true"),
        "Export container assets.",
    ),
    option(
        "--ignore-first-container-dirs",
        Some("N"),
        Some("0"),
        "Drop slash components from container paths.",
    ),
    option(
        "--append-path-id",
        None,
        Some("false"),
        "Append UnityPy's _None suffix.",
    ),
    option(
        "--export-unknown-as-typetree",
        None,
        Some("false"),
        "Other classes as JSON or raw binary.",
    ),
    option(
        "--asset-filter",
        Some("CLASS"),
        None,
        "Repeatable class name or integer ID.",
    ),
    option(
        "--vorbis-headers",
        Some("FILE"),
        None,
        "Runtime VST1 headers; never fetched.",
    ),
    option(
        "--report",
        Some("PATH"),
        None,
        "Also save JSON beneath output directory.",
    ),
];
pub(crate) fn options() -> Vec<crate::agent::options::OptionSpec> {
    let mut specs = OPTIONS.to_vec();
    specs.extend_from_slice(crate::agent::options::BUDGET);
    specs.push(crate::args::COMMAND_HELP);
    specs
}

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
fn recognized_option(arg: &[u8]) -> bool {
    OPTIONS
        .iter()
        .chain(crate::agent::options::BUDGET)
        .any(|s| s.name.as_bytes() == arg)
}
#[allow(
    clippy::too_many_lines,
    reason = "shared legacy extraction parser and transaction"
)]
pub(crate) fn execute(
    raw: &[Vec<u8>],
    roots: Option<&Arc<crate::mcp::roots::Roots>>,
    confined_output: Option<(&files::Files, &[u8])>,
) -> Result<unityrs_mesh::obj::ChargedBytes, Box<dyn std::error::Error>> {
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
        if arg.starts_with(b"-") && arg != b"--" && !recognized_option(arg) {
            return Err("unexpected argument".into());
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
        .map(|path| -> Result<_, Box<dyn std::error::Error>> {
            Ok(if let Some(roots) = roots {
                unityrs_export::VorbisHeaders::from_bytes(
                    &roots.read(path, &limits, &meter)?,
                    &meter,
                )?
            } else {
                unityrs_export::VorbisHeaders::load(path, &meter)?
            })
        })
        .transpose()?;
    options.vorbis = headers.as_ref();
    let mut env_options = EnvOptions::unitypy();
    let mut context = (*env_options.context).clone();
    context.limits = limits;
    if roots.is_some() {
        context.budget = Arc::new(unityrs_core::budget::Counting::new(limits.max_input_memory));
    }
    if let Some(tpk) = tpk {
        context.providers = Arc::new(ProviderChain::new(vec![Arc::new(
            if let Some(roots) = roots {
                unityrs_tpk::TpkProvider::from_bytes(&roots.read(tpk, &limits, &meter)?, &limits)?
            } else {
                unityrs_tpk::TpkProvider::open_with(tpk, &limits)?
            },
        )]));
    }
    env_options.context = Arc::new(context);
    let env = Environment::new(env_options);
    for input in inputs.as_slice() {
        env.load(roots.map_or_else(
            || Source::Path((*input).to_owned()),
            |roots| roots.source(input, limits, env.options().context.budget.clone()),
        ))?;
    }
    let mut files = match confined_output {
        Some((files, directory)) => files.subdirectory(directory, &meter)?,
        None => files::Files::new(output, &meter)?,
    };
    let report = report::checked(extractor::extract_assets(
        &env, &options, &mut files, &meter,
    )?)?;
    let json = report::json(&report, &meter)?;
    if let Some(path) = report_path {
        files.file(path, json.as_bytes())?;
    }
    Ok(json)
}
pub(super) fn main(raw: &[Vec<u8>]) -> ExitCode {
    if matches!(raw, [a] if a == b"-h" || a == b"--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let result = execute(raw, None, None).and_then(|json| {
        std::io::stdout().lock().write_all(json.as_bytes())?;
        Ok(())
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{USAGE}\nunityrs extract: {e}");
            ExitCode::from(1)
        }
    }
}
