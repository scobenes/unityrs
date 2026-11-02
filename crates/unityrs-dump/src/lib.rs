// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/BundleFile.py, UnityPy/files/File.py, UnityPy/files/ObjectReader.py, UnityPy/helpers/ImportHelper.py. See THIRD_PARTY_LICENSES.
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
//! `tools/parity/dump_records.py`, `tools/parity/canonical.py`) and checked
//! against it by the fixtures `tools/fixtures/dump.py` records:
//! - [`json`]: the canonical JSON layer (section 2), and [`mod@encode`]:
//!   values (section 3) from a [`unityrs_core::Parsed`] alone, each written
//!   into a [`text::Sink`]: the records into a [`text::Text`], charged to
//!   the input's meter and fallible, so one input's records count toward
//!   its `max_input_memory` and their refusal is its own record;
//! - [`record`]: the records (section 4) and their lines;
//! - [`input`]: one input's records from the core's public API;
//! - [`read`]: one input's bytes, refused at its limits and charged to its
//!   meter, a refusal its own record;
//! - [`select`]: which inputs, named how, in which order (section 4.2
//!   "Names", section 5), with the oracle's selection flags;
//! - [`run`]: the whole dump: header, inputs in order at any `-j`, `end`,
//!   written to a `.partial` file (gzip level 9, mtime 0, for `.gz`) renamed
//!   when complete, outside the repository;
//! - [`values`]: the value-case protocol `tools/parity/value_cases.py` drives.
//!
//! Owner: package E.

// No panics on untrusted input (plan section 3.15): the dump reads files,
// index JSON and value-case files. Tests may opt out.
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::unreachable,
        clippy::indexing_slicing
    )
)]

pub mod encode;
pub mod input;
pub mod json;
pub mod read;
pub mod record;
pub mod run;
pub mod select;
pub mod text;
pub mod values;

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use unityrs_core::limits::{Charge, Limits};
use unityrs_core::provider::ProviderChain;
use unityrs_core::{FileContext, LimitKind};
use unityrs_tpk::{TpkError, TpkProvider};

/// Every public type of this crate is `Send + Sync`, as the core's are (plan
/// section 3.16): `-j N` shares the options and the records across rayon
/// workers. Each type is asserted beside its definition.
pub(crate) const fn send_sync<T: Send + Sync + ?Sized>() {}

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
    /// The gate's G1c (plan section 6, `--reverse-prepass`): before an input is
    /// dumped, every entry of every bundle in it is parsed in reverse
    /// directory order, nested bundles' included, with nothing decoded up
    /// front ([`input::prepass_reverse`]). The dump must not change.
    pub lazy_reverse: bool,
    /// The bounds lifted by [`DumpOptions::with_unbounded`] (`--unbounded`),
    /// in [`LIFTABLE`] order; the header's `producer` names them.
    pub unbounded: Vec<LimitKind>,
}

impl Default for DumpOptions {
    fn default() -> Self {
        Self {
            levels: Levels::ALL,
            jobs: 1,
            context: Arc::new(FileContext::unitypy()),
            tpk_path: None,
            lazy_reverse: false,
            unbounded: Vec::new(),
        }
    }
}

/// The bounds `--unbounded` can lift: every [`LimitKind`] but `Count`, which
/// bounds an index type, not what an input may cost.
pub const LIFTABLE: [LimitKind; 10] = [
    LimitKind::InputMemory,
    LimitKind::InputBytes,
    LimitKind::BlockOutput,
    LimitKind::DecodeWork,
    LimitKind::Inflate,
    LimitKind::ContainerDepth,
    LimitKind::TreeDepth,
    LimitKind::ValueDepth,
    LimitKind::ValuesPerObject,
    LimitKind::ZeroSizeElements,
];

/// The value depth [`lift`] sets: past what UnityPy reads, and a depth whose
/// stack a worker can be given. The dump sizes each worker's stack from
/// `Limits::max_value_depth` ([`run::worker_stack`]), so `u32::MAX` would ask
/// for terabytes and no worker would start
/// (`tests/deep_values.rs`, `a_dump_with_the_value_depth_lifted_reads_what_unitypyboost_reads`);
/// at this depth a worker gets 384.5 MiB in a release build, 2,050 MiB in a
/// debug one, reserved, not touched. UnityPy's pure-Python reader and its
/// writer raise `RecursionError` at Python's 1,000 frames. UnityPyBoost's
/// reader recurses on the C stack until the process dies: on an 8 MiB main
/// thread (the oracle's, in the dev container), past 174 `A`s of
/// `tools/fixtures/walk.py`'s `class` depth shape, 346 of `vector` and
/// 21,823 of `ref` (`walk.py --boost-reach`, every shape), and a dump at this
/// depth reads each shape at that reach (`tests/deep_values.rs`,
/// `lifted_depth_reads_unitypyboosts_reach`).
pub const LIFTED_VALUE_DEPTH: u32 = 1 << 16;

/// The container depth [`lift`] sets: past what UnityPy reads, and a depth
/// whose stack a worker can be given ([`run::worker_stack`] adds
/// [`run::CONTAINER_LEVEL_STACK`] a level). Each level of nesting is a
/// recursion in UnityPy: `File.read_files` calls `ImportHelper.parse_file`,
/// which builds the entry's `BundleFile`, whose `__init__` calls
/// `read_files` again (`files/File.py:76-90`,
/// `helpers/ImportHelper.py:127-156`, `files/BundleFile.py:53`). That is
/// three Python frames a level at least, and neither UnityPy nor the oracle
/// raises Python's recursion limit of 1,000 frames, so UnityPy raises
/// `RecursionError` (a `parse` error record) before 334 levels. Nesting is
/// a recursion in the core and the dump too, so a bound lifted to
/// `u32::MAX` let a nest a few thousand deep overflow the worker's stack and
/// abort the whole dump. A nest one level deeper than this is the
/// `ContainerDepth` refusal, a `parse` error record as UnityPy's is
/// (`tests/limits.rs`,
/// `a_lifted_container_depth_dumps_to_its_bound_and_refuses_past_it`).
pub const LIFTED_CONTAINER_DEPTH: u32 = 1 << 10;

/// Sets `kind`'s bound in `limits` so that it never trips where UnityPy
/// reads: to its type's maximum, or for `ValueDepth` to
/// [`LIFTED_VALUE_DEPTH`] and for `ContainerDepth` to
/// [`LIFTED_CONTAINER_DEPTH`], depths a worker's stack holds. `false` for a
/// kind not in [`LIFTABLE`].
pub fn lift(limits: &mut Limits, kind: LimitKind) -> bool {
    match kind {
        LimitKind::InputMemory => limits.max_input_memory = u64::MAX,
        LimitKind::InputBytes => limits.max_input_bytes = u64::MAX,
        LimitKind::BlockOutput => limits.max_block_output = u64::MAX,
        LimitKind::DecodeWork => limits.max_decode_work = u64::MAX,
        LimitKind::Inflate => limits.max_inflate = u64::MAX,
        LimitKind::ContainerDepth => limits.max_container_depth = LIFTED_CONTAINER_DEPTH,
        LimitKind::TreeDepth => limits.max_tree_depth = u32::MAX,
        LimitKind::ValueDepth => limits.max_value_depth = LIFTED_VALUE_DEPTH,
        LimitKind::ValuesPerObject => limits.max_values_per_object = u64::MAX,
        LimitKind::ZeroSizeElements => limits.max_zero_size_elements = u64::MAX,
        _ => return false,
    }
    true
}

impl DumpOptions {
    /// The options with each of `kinds`' bounds lifted ([`lift`]): G3b's
    /// check that a limit refusal is the only reason a record differs from
    /// UnityPy's (`tools/parity/g3b.py`). Never for a gate dump: without its
    /// bounds a hostile input costs what it asks for, so the caller bounds
    /// the process instead. Call it before [`DumpOptions::with_tpk`], which
    /// opens the TPK under the context's limits.
    ///
    /// # Errors
    /// The first kind not in [`LIFTABLE`].
    pub fn with_unbounded(mut self, kinds: &[LimitKind]) -> Result<Self, LimitKind> {
        let mut context = FileContext::clone(&self.context);
        for &kind in kinds {
            if !lift(&mut context.limits, kind) {
                return Err(kind);
            }
        }
        self.context = Arc::new(context);
        self.unbounded = LIFTABLE
            .into_iter()
            .filter(|k| kinds.contains(k) || self.unbounded.contains(k))
            .collect();
        Ok(self)
    }

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
#[derive(Debug, Default)]
pub struct InputDump {
    /// JSON lines, each without its newline.
    pub lines: Vec<String>,
    /// The charge for `lines` ([`input::dump_on`]'s records: each line and
    /// the list, charged to the input's meter as they were made), returned
    /// when they drop, once the run has written them. `None` for a record
    /// made outside a meter (a refusal's one record).
    pub charge: Option<Charge>,
    /// `file` records among them.
    pub files: u64,
    /// `object` records among them.
    pub objects: u64,
    /// The sha256 of the input's bytes (checked against a corpus index), or
    /// `None` when it was not taken: hashing them would have passed the
    /// input's `max_decode_work` ([`read`], [`input`]), and its record's
    /// `l0.sha256` is then [`read::NOT_TAKEN`].
    pub sha256: Option<[u8; 32]>,
    /// The input's size.
    pub bytes_in: u64,
    /// 1 when the input's one record is a `crash` (it ran its dumper out of
    /// memory, [`read`], [`input`]), else 0: the oracle's `crashes` count,
    /// which `unitypy_dump.py` exits 3 on when every input has one
    /// ([`run::Summary::every_input_crashed`]).
    pub crashes: u64,
}

/// Two dumps of an input are equal when their records are: the charges
/// they hold are not compared.
impl PartialEq for InputDump {
    fn eq(&self, other: &Self) -> bool {
        (
            &self.lines,
            self.files,
            self.objects,
            self.sha256,
            self.bytes_in,
            self.crashes,
        ) == (
            &other.lines,
            other.files,
            other.objects,
            other.sha256,
            other.bytes_in,
            other.crashes,
        )
    }
}

impl Eq for InputDump {}

impl InputDump {
    /// Appends a `file` record's line.
    pub fn push_file(&mut self, line: String) {
        self.lines.push(line);
        self.files += 1;
    }

    /// The records as text, each line ended by `\n` (a test's view: the run
    /// writes each line as it is, never a copy of them all).
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
    header_line_unbounded(levels, producer, &[])
}

/// The `dump` header line of a run with `unbounded`'s bounds lifted
/// ([`DumpOptions::with_unbounded`]): the producer's `unbounded` member
/// names them, so such a dump is never taken for a gate dump. With none, the
/// header is [`header_line`]'s.
#[must_use]
pub fn header_line_unbounded(levels: Levels, producer: &str, unbounded: &[LimitKind]) -> String {
    let mut members = String::from("\"name\":");
    json::write_str(&mut members, producer);
    members.push_str(",\"unityrs\":");
    json::write_str(&mut members, unityrs_core::VERSION);
    if !unbounded.is_empty() {
        members.push_str(",\"unbounded\":");
        let names: Vec<String> = unbounded.iter().map(|k| format!("{k:?}")).collect();
        json::write_array(&mut members, &names, |out, n| json::write_str(out, n));
    }
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
    let counts = run::write_dump(
        &inputs,
        opts,
        &mut gz,
        &mut |_, _| Ok(()),
        &|rel, data, m| input::dump_on(rel, data, m, opts),
    )
    .map_err(std::io::Error::other)?;
    gz.finish()?;
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::{DumpOptions, LIFTABLE, Levels, Limits, lift};

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
    fn unbounded_lifts_each_named_bound_and_the_header_names_them() {
        use unityrs_core::LimitKind as K;
        let d = Limits::default();
        let o = DumpOptions::default()
            .with_unbounded(&[K::ZeroSizeElements, K::TreeDepth])
            .unwrap();
        let l = &o.context.limits;
        assert_eq!(l.max_zero_size_elements, u64::MAX);
        assert_eq!(l.max_tree_depth, u32::MAX);
        assert_eq!(l.max_value_depth, d.max_value_depth);
        assert_eq!(l.max_input_memory, d.max_input_memory);
        assert_eq!(o.unbounded, [K::TreeDepth, K::ZeroSizeElements]);
        assert_eq!(
            DumpOptions::default()
                .with_unbounded(&[K::Count])
                .unwrap_err(),
            K::Count
        );
        for k in LIFTABLE {
            let mut l = Limits::default();
            assert!(lift(&mut l, k), "{k:?}");
            assert_ne!(l, Limits::default(), "{k:?}");
        }
        let h = super::header_line_unbounded(Levels::ALL, "unityrs", &o.unbounded);
        assert!(
            h.ends_with(&format!(
                "\"producer\":{{\"name\":\"unityrs\",\"unityrs\":\"{}\",\"unbounded\":[\"TreeDepth\",\"ZeroSizeElements\"]}}}}",
                unityrs_core::VERSION
            )),
            "{h}"
        );
        assert_eq!(
            super::header_line_unbounded(Levels::ALL, "unityrs", &[]),
            super::header_line(Levels::ALL, "unityrs")
        );
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
        // Removed when dropped, a failing assert's unwinding included
        // (review round 9: removed by the last line, it stayed on a failure).
        let dir = unityrs_testkit::scratch::Scratch::new("dump-tpk");
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
