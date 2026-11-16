//! Legacy parsers remain untouched; their public surfaces are described here
//! and checked against their source and observable parser defaults in tests.
use super::{
    Document, Result, fixed,
    options::{self, OptionSpec},
};
use serde_json::{Value, json};
fn option(s: OptionSpec) -> Value {
    json!({"name":s.name,"value":s.value,"default":s.default,"help":s.help})
}
fn legacy(
    name: &'static str,
    form: Option<&'static str>,
    default: Option<&'static str>,
    help: &'static str,
) -> OptionSpec {
    OptionSpec {
        name,
        value: form,
        default,
        help,
    }
}
#[allow(clippy::too_many_lines, reason = "declarative legacy command table")]
fn legacy_options(command: &str) -> Vec<OptionSpec> {
    let mut opts = Vec::new();
    let mut add = |name, form, default, help| opts.push(legacy(name, form, default, help));
    match command {
        "dump" => {
            for name in ["-o", "--output"] {
                add(
                    name,
                    Some("PATH"),
                    None,
                    "Required dump JSONL output, optionally gzip.",
                );
            }
            add(
                "--index",
                None,
                Some("false"),
                "Inputs are index JSON files.",
            );
            add(
                "--kind",
                Some("K[,K...]"),
                Some("bundle,unitypy-sample"),
                "Index kinds to select.",
            );
            add("--build", Some("B[,B...]"), None, "Index builds to select.");
            add(
                "--allow-index-mismatch",
                None,
                Some("false"),
                "Warn rather than refuse an index hash mismatch.",
            );
            add(
                "--name",
                Some("GLOB"),
                None,
                "Repeatable glob on dump-relative input path.",
            );
            add("--sample", Some("N"), None, "Evenly sample sorted inputs.");
            add("--limit", Some("N"), None, "First N sorted inputs.");
            add(
                "--levels",
                Some("L0,L1,L2,L3"),
                Some("L0,L1,L2,L3"),
                "Canonical levels to write.",
            );
            for name in ["-j", "--jobs"] {
                add(
                    name,
                    Some("N"),
                    Some("1"),
                    "Concurrent inputs; values below one mean one.",
                );
            }
            add("--stats", Some("PATH"), None, "Save timing JSON.");
            for name in ["-q", "--quiet"] {
                add(name, None, Some("false"), "Suppress per-input progress.");
            }
            add(
                "--reverse-prepass",
                None,
                Some("false"),
                "Visit entries in reverse before dumping.",
            );
            add(
                "--unbounded",
                Some("KIND[,KIND...]"),
                None,
                "Lift named bounds for parity experiments.",
            );
            add("--mesh", None, Some("false"), "Include mesh digests.");
            add(
                "--tpk",
                Some("FILE"),
                None,
                "Schema provider for stripped files.",
            );
        }
        "gltf" => {
            for (name, form, help) in [
                ("--scene", "FILE", "Select a SerializedFile or SceneAsset."),
                ("--path-id", "ID", "Select a prefab root or SceneAsset."),
                ("--prefab", "PATH", "Select a container prefab path."),
                ("-o", "FILE", "Required output GLB."),
                ("--profile", "FILE", "Strict profile_version 1 JSON."),
                ("--builtin-file", "FILE", "Supplied built-in resources."),
                ("--report", "FILE", "Also write detailed report JSON."),
            ] {
                add(name, Some(form), None, help);
            }
            add(
                "--mapper",
                Some("unlit|pbr"),
                Some("unlit"),
                "Material mapper.",
            );
            opts.extend_from_slice(options::BUDGET);
        }
        "obj" => {
            add(
                "--path-id",
                Some("ID"),
                None,
                "Required Mesh or renderer ID.",
            );
            add(
                "-o",
                Some("PATH"),
                None,
                "Required OBJ file or renderer output directory.",
            );
            add(
                "--tpk",
                Some("FILE"),
                None,
                "Schema provider for stripped files.",
            );
        }
        "extract" => {
            add("-o", Some("DIR"), None, "Required output directory.");
            add(
                "--no-container",
                None,
                Some("false"),
                "Export every object by name.",
            );
            add(
                "--no-use-container",
                None,
                Some("false"),
                "Alias for --no-container.",
            );
            add(
                "--use-container",
                None,
                Some("true"),
                "Export container assets.",
            );
            add(
                "--ignore-first-container-dirs",
                Some("N"),
                Some("0"),
                "Drop slash components from container paths.",
            );
            add(
                "--append-path-id",
                None,
                Some("false"),
                "Append UnityPy's _None suffix.",
            );
            add(
                "--export-unknown-as-typetree",
                None,
                Some("false"),
                "Other classes as JSON or raw binary.",
            );
            add(
                "--asset-filter",
                Some("CLASS"),
                None,
                "Repeatable class name or integer ID.",
            );
            add(
                "--vorbis-headers",
                Some("FILE"),
                None,
                "Runtime VST1 headers; never fetched.",
            );
            add(
                "--report",
                Some("PATH"),
                None,
                "Also save JSON beneath output directory.",
            );
            opts.extend_from_slice(options::BUDGET);
        }
        "batch" => {
            add(
                "--check",
                Some("JOB.json"),
                None,
                "Validate the job without running it.",
            );
            add(
                "--schema",
                None,
                None,
                "Print the existing batch job schema; see unityrs batch --schema.",
            );
        }
        _ => {}
    }
    if command != "batch" {
        opts.push(legacy(
            "--help",
            None,
            None,
            "Print command help (also -h).",
        ));
    }
    opts
}
const COMMANDS: &[(&str, &str, &str, &str, &[i32])] = &[
    (
        "dump",
        "Write the canonical parity dump.",
        "unityrs dump INPUT... -o OUT.jsonl[.gz] [options]",
        "Canonical JSONL; dump -h documents parity semantics.",
        &[0, 1, 2, 3],
    ),
    (
        "values",
        "Run the parity value-case protocol.",
        "unityrs values CASES.json",
        "Value-case JSON lines.",
        &[0, 1, 2],
    ),
    (
        "gltf",
        "Export a scene or prefab to GLB.",
        "unityrs gltf INPUT... (--scene FILE | --path-id ID | --prefab PATH) -o OUT.glb [options]",
        "No stdout document; writes GLB and optional report files.",
        &[0, 1],
    ),
    (
        "obj",
        "Export a Mesh or renderer to OBJ/MTL.",
        "unityrs obj INPUT --path-id ID -o OUTPUT [--tpk FILE]",
        "No stdout document; writes OBJ/MTL files.",
        &[0, 1],
    ),
    (
        "extract",
        "Export assets to a directory.",
        "unityrs extract INPUT... -o DIR [options]",
        "Extraction JSON report including object failures.",
        &[0, 1],
    ),
    (
        "batch",
        "Run the budgeted batch engine.",
        "unityrs batch --check JOB.json | --schema | JOB.json",
        "NDJSON progress or validation, or job schema with --schema; see docs/BATCH_SPEC.md.",
        &[0, 3, 4, 5, 6, 7, 69, 70, 74, 78],
    ),
    (
        "inspect",
        "List objects with filters and pagination.",
        "unityrs inspect INPUT... [options]",
        "One unityrs.inspect or unityrs.error JSON document.",
        &[0, 1, 2],
    ),
    (
        "show",
        "Read one object with bounded context.",
        "unityrs show INPUT... --path-id N [options]",
        "One unityrs.show or unityrs.error JSON document.",
        &[0, 1, 2],
    ),
    (
        "describe",
        "Describe commands or print an output schema.",
        "unityrs describe [--schema NAME]",
        "One unityrs.describe or unityrs.error JSON document, or a JSON Schema.",
        &[0, 1, 2],
    ),
];
pub fn run() -> Result<Document> {
    let commands: Vec<Value> = COMMANDS.iter().map(|(name, purpose, usage, stdout, exits)| {
        let opts = if matches!(*name, "inspect" | "show" | "describe") { options::specs(name) } else { legacy_options(name) };
        json!({"name":name,"purpose":purpose,"usage":usage,"options":opts.into_iter().map(option).collect::<Vec<_>>(),"stdout":stdout,"exit_codes":exits})
    }).collect();
    fixed(
        &json!({"format":"unityrs.describe","version":1,"commands":commands,
        "exit_codes":{"0":"Success; inspect may contain per-input errors.","1":"New commands: operation failed. Legacy commands: command-specific failure.","2":"Usage error for the new commands, dump and values.","3":"Dump: every input crashed. Batch: complete with skips.","4":"Batch: incomplete.","5":"Batch: budget refusal.","6":"Batch: stopped.","7":"Batch: superseded.","69":"Batch: could not start.","74":"Batch: host failure.","70":"Batch: internal failure.","78":"Batch: invalid job specification."},
        "schemas":["inspect","show","describe","error"],"batch_schema":"unityrs batch --schema"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn advertised_budget_defaults_match_the_shared_parser() {
        let limits = crate::extract::limits::parse(&[]).unwrap();
        let actual = [
            ("--max-input-memory", limits.max_input_memory.to_string()),
            ("--max-input-bytes", limits.max_input_bytes.to_string()),
            ("--max-decode-work", limits.max_decode_work.to_string()),
            ("--max-tree-depth", limits.max_tree_depth.to_string()),
        ];
        for command in ["inspect", "show", "extract", "gltf"] {
            let specs = if matches!(command, "inspect" | "show") {
                options::specs(command)
            } else {
                legacy_options(command)
            };
            for (name, value) in &actual {
                assert_eq!(
                    specs.iter().find(|s| s.name == *name).unwrap().default,
                    Some(value.as_str()),
                    "{command} {name}"
                );
            }
        }
    }
    #[test]
    fn legacy_defaults_match_parser_values() {
        let raw = [b"input".to_vec(), b"-o".to_vec(), b"output".to_vec()];
        let crate::args::Parsed::Dump(args) = crate::args::parse(&raw).unwrap() else {
            panic!("dump args")
        };
        let defaults = [
            ("--index", args.index.to_string()),
            (
                "--kind",
                args.kind
                    .iter()
                    .map(|s| std::str::from_utf8(s).unwrap())
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            (
                "--allow-index-mismatch",
                args.allow_index_mismatch.to_string(),
            ),
            ("--levels", args.levels.names().join(",")),
            ("--jobs", args.jobs.to_string()),
            ("-j", args.jobs.to_string()),
            ("--quiet", args.quiet.to_string()),
            ("-q", args.quiet.to_string()),
            ("--reverse-prepass", args.lazy_reverse.to_string()),
            ("--mesh", args.mesh.to_string()),
        ];
        for (key, default) in defaults {
            assert_eq!(
                legacy_options("dump")
                    .iter()
                    .find(|o| o.name == key)
                    .unwrap()
                    .default,
                Some(default.as_str())
            );
        }
        let extract = unityrs_export::extractor::Options::default();
        let defaults = [
            ("--use-container", extract.use_container.to_string()),
            ("--no-container", (!extract.use_container).to_string()),
            ("--no-use-container", (!extract.use_container).to_string()),
            ("--append-path-id", extract.append_path_id.to_string()),
            (
                "--export-unknown-as-typetree",
                extract.export_unknown_as_typetree.to_string(),
            ),
            (
                "--ignore-first-container-dirs",
                extract.ignore_first_container_dirs.to_string(),
            ),
        ];
        for (key, default) in defaults {
            assert_eq!(
                legacy_options("extract")
                    .iter()
                    .find(|o| o.name == key)
                    .unwrap()
                    .default,
                Some(default.as_str())
            );
        }
        // The legacy glTF parser is intentionally private and unchanged.
        // Pin its default expression as well as exercising omission/explicit
        // defaults through the real binary in the integration contract test.
        assert!(include_str!("../gltf/args.rs").contains("pbr: mapper.unwrap_or(false)"));
        assert_eq!(
            legacy_options("gltf")
                .iter()
                .find(|o| o.name == "--mapper")
                .unwrap()
                .default,
            Some("unlit")
        );
    }
}
