//! The canonical dump (`docs/CANONICAL_DUMP.md`, format 1) from `unityrs-core`.
//!
//! One input gives its records depth-first (section 5): its `file` record,
//! then a `SerializedFile`'s objects in ascending `path_id`, or a container's
//! entries in `files` dict order, recursively. A failing entry fails its input
//! (section 4.2), so an input is dumped whole or as one error record. Inputs
//! are independent: `-j N` dumps them in parallel and writes them in path
//! order, so the output does not depend on `N`.
//!
//! The dump reads the corpus with [`unityrs_core::env::EnvOptions::unitypy`]'s
//! file context and nothing else: no shim, no tolerance, no override. It is
//! the parity gate's candidate side (plan section 6).
//!
//! The pieces, each a port of the oracle's (`tools/parity/unitypy_dump.py`,
//! `tools/parity/canonical.py`) and checked against it by the fixtures
//! `tools/fixtures/dump.py` records:
//! - [`json`]: the canonical JSON layer (section 2), and [`mod@encode`]:
//!   values (section 3) from a [`unityrs_core::Parsed`] alone;
//! - [`record`]: the records (section 4) and their lines;
//! - [`input`]: one input's records from the core's public API;
//! - [`select`]: which inputs, named how, in which order (section 4.2
//!   "Names", section 5), with the oracle's selection flags;
//! - [`run`]: the whole dump: header, inputs in order at any `-j`, `end`,
//!   written to a `.partial` file (gzip level 9, mtime 0, for `.gz`) renamed
//!   when complete, outside the repository;
//! - [`values`]: the value-case protocol `tools/parity/value_cases.py` drives.
//!
//! Owner: package E.

pub mod encode;
pub mod input;
pub mod json;
pub mod record;
pub mod run;
pub mod select;
pub mod values;

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use unityrs_core::FileContext;
use unityrs_core::provider::ProviderChain;
use unityrs_tpk::{TpkError, TpkProvider};

/// Every public type of this crate is `Send + Sync`, as the core's are (plan
/// section 3.16): `-j N` shares the options and the records across rayon
/// workers. Each type is asserted beside its definition.
pub(crate) const fn send_sync<T: Send + Sync>() {}

const _: () = send_sync::<Levels>();
const _: () = send_sync::<DumpOptions>();
const _: () = send_sync::<InputDump>();

/// Which levels to write beyond L0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Levels {
    /// Structure: headers, tables, containers.
    pub l1: bool,
    /// Values.
    pub l2: bool,
    /// Re-save digests.
    pub l3: bool,
}

impl Levels {
    /// L0-L3.
    pub const ALL: Self = Self {
        l1: true,
        l2: true,
        l3: true,
    };

    /// The header's `levels` array, in order.
    #[must_use]
    pub fn names(self) -> Vec<&'static str> {
        let mut v = vec!["L0"];
        if self.l1 {
            v.push("L1");
        }
        if self.l2 {
            v.push("L2");
        }
        if self.l3 {
            v.push("L3");
        }
        v
    }

    /// `--levels`, as `unitypy_dump.py` reads it: a comma list of L0-L3 in
    /// any order, which must hold L0, and L2 wherever it holds L3 (the
    /// resave writes the parsed value).
    pub fn parse(list: &str) -> Result<Self, String> {
        let asked: Vec<&str> = list.split(',').collect();
        let unknown: Vec<String> = asked
            .iter()
            .filter(|l| !["L0", "L1", "L2", "L3"].contains(l))
            .map(|l| format!("'{l}'"))
            .collect();
        if !unknown.is_empty() {
            return Err(format!(
                "unknown level(s) {}; the levels are L0, L1, L2, L3",
                unknown.join(", ")
            ));
        }
        if !asked.contains(&"L0") {
            return Err("every dump holds L0 (each file record carries its detection)".into());
        }
        let levels = Self {
            l1: asked.contains(&"L1"),
            l2: asked.contains(&"L2"),
            l3: asked.contains(&"L3"),
        };
        if levels.l3 && !levels.l2 {
            return Err("L3 needs L2 (the resave writes the parsed value)".into());
        }
        Ok(levels)
    }
}

/// Dump options.
#[derive(Clone, Debug)]
pub struct DumpOptions {
    /// Levels.
    pub levels: Levels,
    /// Inputs dumped at once.
    pub jobs: usize,
    /// The core context every input is read with.
    pub context: Arc<FileContext>,
    /// The TPK the typetree provider loads for stripped objects (`--tpk`).
    pub tpk_path: Option<std::path::PathBuf>,
}

impl Default for DumpOptions {
    fn default() -> Self {
        Self {
            levels: Levels::ALL,
            jobs: 1,
            context: Arc::new(FileContext::unitypy()),
            tpk_path: None,
        }
    }
}

impl DumpOptions {
    /// The options with the TPK at `path` (`--tpk`) as the context's one
    /// typetree provider, as UnityPy's bundled class database answers a
    /// stripped object (`files/ObjectReader.py:285-307`, `helpers/Tpk.py`):
    /// the dump's `l2.source` is then `"tpk"` for an object whose file
    /// embeds no tree. Its trees are as deep as the context's limits allow.
    pub fn with_tpk(mut self, path: &Path) -> Result<Self, TpkError> {
        let tpk = TpkProvider::open_with(path, &self.context.limits)?;
        let mut context = FileContext::clone(&self.context);
        context.providers = Arc::new(ProviderChain::new(vec![Arc::new(tpk)]));
        self.context = Arc::new(context);
        self.tpk_path = Some(path.to_path_buf());
        Ok(self)
    }
}

/// One input's records, serialised, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputDump {
    /// JSON lines, each without its newline.
    pub lines: Vec<String>,
    /// `file` records among them.
    pub files: u64,
    /// `object` records among them.
    pub objects: u64,
    /// The sha256 of the input's bytes (checked against a corpus index).
    pub sha256: [u8; 32],
    /// The input's size.
    pub bytes_in: u64,
}

impl InputDump {
    /// Appends a `file` record's line.
    pub fn push_file(&mut self, line: String) {
        self.lines.push(line);
        self.files += 1;
    }

    /// The records as text, each line ended by `\n`.
    #[must_use]
    pub fn text(&self) -> String {
        let mut s = String::with_capacity(self.lines.iter().map(|l| l.len() + 1).sum());
        for l in &self.lines {
            s.push_str(l);
            s.push('\n');
        }
        s
    }
}

/// The `dump` header line, for a producer named `producer`.
#[must_use]
pub fn header_line(levels: Levels, producer: &str) -> String {
    let mut members = String::from("\"name\":");
    json::write_str(&mut members, producer);
    members.push_str(",\"unityrs\":");
    json::write_str(&mut members, unityrs_core::VERSION);
    record::header(levels, &members)
}

/// The `end` line.
#[must_use]
pub fn end_line(files: u64, objects: u64) -> String {
    record::end(files, objects)
}

/// One input's records. `rel` is its path relative to the corpus root, as
/// bytes, which is how the dump names it.
#[must_use]
pub fn dump_input(rel: &[u8], data: unityrs_core::Bytes, opts: &DumpOptions) -> InputDump {
    input::dump_input(rel, data, opts)
}

/// Dumps every file under `root`, in bytewise path order, to `out` (gzip,
/// level 9, mtime 0, as the oracle writes). Returns the `file` and `object`
/// record counts.
pub fn dump_tree(
    root: &Path,
    opts: &DumpOptions,
    out: &mut dyn Write,
) -> std::io::Result<(u64, u64)> {
    let sel = select::Selection {
        inputs: vec![root.to_path_buf()],
        ..select::Selection::default()
    };
    let inputs = select::collect(&sel).map_err(std::io::Error::other)?;
    let mut gz = run::gzip(out);
    let counts = run::write_dump(&inputs, opts, &mut gz, &mut |_, _| Ok(()), &|rel, data| {
        input::dump_input(rel, data, opts)
    })
    .map_err(std::io::Error::other)?;
    gz.finish()?;
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::Levels;

    #[test]
    fn level_names() {
        assert_eq!(Levels::ALL.names(), ["L0", "L1", "L2", "L3"]);
        let l1 = Levels {
            l1: true,
            l2: false,
            l3: false,
        };
        assert_eq!(l1.names(), ["L0", "L1"]);
    }

    #[test]
    fn levels_parse_as_the_oracle_reads_them() {
        assert_eq!(Levels::parse("L3,L0,L2,L1"), Ok(Levels::ALL));
        assert_eq!(
            Levels::parse("L0"),
            Ok(Levels {
                l1: false,
                l2: false,
                l3: false
            })
        );
        assert!(Levels::parse("L1,L2").unwrap_err().contains("L0"));
        assert!(Levels::parse("L0,L3").unwrap_err().contains("L3 needs L2"));
        assert!(Levels::parse("L0,L4").unwrap_err().contains("'L4'"));
    }

    #[test]
    fn the_header_names_the_producer() {
        let h = super::header_line(Levels::ALL, "unityrs");
        assert_eq!(
            h,
            format!(
                "{{\"rec\":\"dump\",\"format\":\"unityrs-canonical-dump\",\"version\":1,\"levels\":[\"L0\",\"L1\",\"L2\",\"L3\"],\"producer\":{{\"name\":\"unityrs\",\"unityrs\":\"{}\"}}}}",
                unityrs_core::VERSION
            )
        );
    }

    /// `--tpk`: the TPK answers a stripped class through the context's
    /// chain, named `tpk` (the dump's `l2.source`); a file that is no TPK is
    /// an error, never a dump without the provider.
    #[test]
    fn with_tpk_puts_the_tpk_in_the_context_chain() {
        use unityrs_core::UnityVersion;
        use unityrs_core::provider::{TreeRequest, TreeSource};

        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../unityrs-tpk/tests/fixtures/tpk/synthetic.json");
        let all: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture).unwrap()).unwrap();
        let case = all["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "meta_flag_u32")
            .unwrap();
        let hex = case["tpk"].as_str().unwrap();
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let dir = std::env::temp_dir().join(format!("unityrs-dump-tpk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (good, bad) = (dir.join("good.tpk"), dir.join("bad.tpk"));
        std::fs::write(&good, &bytes).unwrap();
        std::fs::write(&bad, b"not a tpk").unwrap();

        let plain = super::DumpOptions::default();
        assert!(plain.context.providers.is_empty());
        let opts = plain.with_tpk(&good).unwrap();
        assert_eq!(opts.tpk_path.as_deref(), Some(good.as_path()));
        let version =
            UnityVersion::from_packed(case["versions"][0].as_str().unwrap().parse().unwrap());
        let found = |class_id| {
            opts.context
                .providers
                .lookup(&TreeRequest {
                    class_id,
                    version: &version,
                    script: None,
                })
                .unwrap()
        };
        let tree = found(80).expect("class 80 has a tree at this version");
        assert_eq!(tree.source, TreeSource::Provider("tpk"));
        assert!(found(81).is_none(), "class 81 is not in the TPK");
        assert!(super::DumpOptions::default().with_tpk(&bad).is_err());
        assert!(
            super::DumpOptions::default()
                .with_tpk(&dir.join("missing.tpk"))
                .is_err()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Plan section 3.16: the dump's public types are asserted as the core's
    /// are, by full path, and a new one fails this test until it is.
    #[test]
    fn every_public_type_is_asserted() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let missing = unityrs_testkit::public_types::unasserted(&src);
        assert!(missing.is_empty(), "not asserted Send + Sync: {missing:?}");
    }
}
