//! Machine reference generated from the option metadata used by the parsers.
use super::{
    Document, Result, fixed,
    options::{self, OptionSpec},
};
use serde_json::{Value, json};
fn option(s: OptionSpec) -> Value {
    json!({"name":s.name,"value":s.value,"default":s.default,"help":s.help})
}
fn legacy_options(command: &str) -> Vec<OptionSpec> {
    match command {
        "dump" => crate::args::options(),
        "gltf" => crate::gltf::options(),
        "obj" => crate::obj::options(),
        "extract" => crate::extract::options(),
        "batch" => crate::batch::options(),
        "values" => vec![crate::args::COMMAND_HELP],
        _ => Vec::new(),
    }
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
