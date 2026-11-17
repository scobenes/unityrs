//! Borrow argv; selectors and option values need no input-sized scratch.
use super::Result;
use caseless::Caseless;
use std::{ffi::OsStr, os::unix::ffi::OsStrExt, path::Path};
use unityrs_core::limits::Limits;

use crate::args::option;
pub(crate) const OPTIONS: &[crate::agent::options::OptionSpec] = &[
    option(
        "--scene",
        Some("FILE"),
        None,
        "Select a SerializedFile or SceneAsset.",
    ),
    option(
        "--path-id",
        Some("ID"),
        None,
        "Select a prefab root or SceneAsset.",
    ),
    option(
        "--prefab",
        Some("PATH"),
        None,
        "Select a container prefab path.",
    ),
    option("-o", Some("FILE"), None, "Required output GLB."),
    option(
        "--profile",
        Some("FILE"),
        None,
        "Strict profile_version 1 JSON.",
    ),
    option(
        "--builtin-file",
        Some("FILE"),
        None,
        "Supplied built-in resources.",
    ),
    option(
        "--report",
        Some("FILE"),
        None,
        "Also write detailed report JSON.",
    ),
    option(
        "--mapper",
        Some("unlit|pbr"),
        Some("unlit"),
        "Material mapper.",
    ),
];
pub(crate) fn options() -> Vec<crate::agent::options::OptionSpec> {
    let mut specs = OPTIONS.to_vec();
    specs.extend_from_slice(crate::agent::options::BUDGET);
    specs.push(crate::args::COMMAND_HELP);
    specs
}

pub(super) const USAGE: &str = "usage: unityrs gltf INPUT... (--scene FILE | --path-id ID | --prefab PATH) -o OUTPUT.glb\n\
  --scene FILE           select a SerializedFile by name or exactly one SceneAsset at a container path\n\
  --path-id ID           select one GameObject prefab root or SceneAsset's owning file\n\
  --prefab PATH          select one GameObject container entry by its exact path\n\
  --profile FILE         strict profile_version: 1 JSON (default policy otherwise)\n\
  --mapper unlit|pbr     material mapper (default unlit)\n\
  --builtin-file FILE    user resource file, resolved through CabResolver\n\
  --tpk FILE             schema provider for stripped trees\n\
  --report FILE          detailed JSON report, also embedded in GLB asset extras\n\
  --max-input-memory N   charged bytes per input (default 8589934592)\n\
  --max-input-bytes N    source bytes per input (default 4294967296)\n\
  --max-decode-work N    work bytes per input (default 34359738368)\n\
  --max-tree-depth N     graph/tree depth (default 256)\n\
Load dependencies, including .sharedAssets siblings, as additional INPUTs.\n\
Profile builtin: placeholder (default empty nodes + report), resolve (supplied file),\n\
procedural (opt-in approximate Cube, Quad, Plane). Scene roots follow object-table order.\n\
Outputs overwrite files and symlinks by temp-sibling rename after successful export.\n\
GLB/report names in the same canonical directory must differ under canonical Unicode case folding.\n\
A publish failure keeps earlier complete outputs; interrupted runs may leave\n\
.unityrs-gltf-*.tmp siblings, safe to delete. Status 0: success/help; 1: any error.";

#[derive(Clone, Copy)]
pub(super) enum Selection<'a> {
    Scene(&'a [u8]),
    PathId(i64),
    Prefab(&'a [u8]),
}
pub(super) struct Args<'a> {
    raw: &'a [Vec<u8>],
    pub output: &'a Path,
    pub selection: Selection<'a>,
    pub profile: Option<&'a Path>,
    pub tpk: Option<&'a Path>,
    pub builtin_file: Option<&'a Path>,
    pub report: Option<&'a Path>,
    pub pbr: bool,
    pub limits: Limits,
}
pub(super) fn path(bytes: &[u8]) -> &Path {
    Path::new(OsStr::from_bytes(bytes))
}
fn parent(p: &Path) -> &Path {
    p.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}
fn same_name(a: &Path, b: &Path) -> bool {
    a.file_name().zip(b.file_name()).is_some_and(|(a, b)| {
        a == b
            || match (a.to_str(), b.to_str()) {
                // Unicode canonical caseless match: NFD, case fold, NFD again.
                (Some(a), Some(b)) => caseless::canonical_caseless_match_str(a, b),
                // Preserve invalid filename bytes without lossy UTF-8 conversion.
                _ => folded(a).eq(folded(b)),
            }
    })
}
fn folded(name: &OsStr) -> impl Iterator<Item = u32> {
    name.as_bytes().utf8_chunks().flat_map(|chunk| {
        chunk
            .valid()
            .chars()
            .default_case_fold()
            .map(u32::from)
            .chain(
                // Keep invalid Unix filename bytes distinct from Unicode scalars.
                chunk.invalid().iter().map(|&b| 0x11_0000 + u32::from(b)),
            )
    })
}
fn unique<T>(slot: &mut Option<T>, value: T) -> Result<()> {
    if slot.replace(value).is_some() {
        return Err("duplicate option or unit selector".into());
    }
    Ok(())
}
impl<'a> Args<'a> {
    pub fn parse(raw: &'a [Vec<u8>]) -> Result<Self> {
        let (
            mut output,
            mut selection,
            mut profile,
            mut tpk,
            mut builtin_file,
            mut report,
            mut mapper,
        ) = (None, None, None, None, None, None, None);
        let mut limits = Limits::default();
        let mut inputs = 0;
        let mut iter = raw.iter();
        while let Some(arg) = iter.next() {
            if !arg.starts_with(b"-") {
                inputs += 1;
                continue;
            }
            if !OPTIONS
                .iter()
                .chain(crate::agent::options::BUDGET)
                .any(|s| s.name.as_bytes() == arg)
            {
                return Err("unexpected argument".into());
            }
            let value = iter.next().ok_or("missing option value")?.as_slice();
            let numeric = || -> Result<u64> { Ok(std::str::from_utf8(value)?.parse()?) };
            match arg.as_slice() {
                b"-o" => unique(&mut output, path(value))?,
                b"--scene" => unique(&mut selection, Selection::Scene(value))?,
                b"--path-id" => unique(
                    &mut selection,
                    Selection::PathId(std::str::from_utf8(value)?.parse()?),
                )?,
                b"--prefab" => unique(&mut selection, Selection::Prefab(value))?,
                b"--profile" => unique(&mut profile, path(value))?,
                b"--tpk" => unique(&mut tpk, path(value))?,
                b"--builtin-file" => unique(&mut builtin_file, path(value))?,
                b"--report" => unique(&mut report, path(value))?,
                b"--mapper" => unique(
                    &mut mapper,
                    match value {
                        b"unlit" => false,
                        b"pbr" => true,
                        _ => return Err("unknown material mapper".into()),
                    },
                )?,
                b"--max-input-memory" => limits.max_input_memory = numeric()?,
                b"--max-input-bytes" => limits.max_input_bytes = numeric()?,
                b"--max-decode-work" => limits.max_decode_work = numeric()?,
                b"--max-tree-depth" => limits.max_tree_depth = u32::try_from(numeric()?)?,
                _ => return Err("unexpected argument".into()),
            }
        }
        if inputs == 0 {
            return Err("missing INPUT".into());
        }
        let output = output.ok_or("missing OUTPUT")?;
        if let Some(report) = report
            && (output == report
                || (same_name(output, report)
                    && std::fs::canonicalize(parent(output))
                        .ok()
                        .zip(std::fs::canonicalize(parent(report)).ok())
                        .is_some_and(|(a, b)| a == b)))
        {
            return Err("report and GLB destinations must differ".into());
        }
        Ok(Self {
            raw,
            output,
            selection: selection.ok_or("missing unit selector")?,
            profile,
            tpk,
            builtin_file,
            report,
            pbr: mapper.unwrap_or(false),
            limits,
        })
    }
    pub fn inputs(&self) -> impl Iterator<Item = &'a Path> {
        let mut iter = self.raw.iter();
        std::iter::from_fn(move || {
            loop {
                let arg = iter.next()?;
                if arg.starts_with(b"-") {
                    iter.next();
                } else {
                    return Some(path(arg));
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{path, same_name};
    #[test]
    fn destination_folding_matches_canonical_equivalents() {
        assert!(same_name(
            path("café.glb".as_bytes()),
            path("cafe\u{301}.glb".as_bytes())
        ));
    }
    #[test]
    fn destination_folding_matches_case_and_canonical_equivalents() {
        assert!(same_name(
            path("café.glb".as_bytes()),
            path("CAFE\u{301}.GLB".as_bytes())
        ));
    }
    #[test]
    fn destination_folding_preserves_invalid_unix_filename_bytes() {
        assert!(!same_name(path(b"\xff.glb"), path(b"\xfe.glb")));
        assert!(!same_name(path(b"\xff.glb"), path("�.glb".as_bytes())));
        assert!(same_name(path(b"scene\xff.glb"), path(b"SCENE\xff.GLB")));
    }
}
