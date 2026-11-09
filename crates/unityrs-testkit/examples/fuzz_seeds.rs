//! The fuzz targets' seed corpora (`fuzz/`, the fuzz plan of
//! `docs/plans/core.md` section 6), written from the testkit's builders:
//! every output `unityrs_testkit::mutate::builder_outputs` lists (the format
//! matrix, every format, codec and flag set, and every file of every
//! Environment scene), the files each bundle holds, every object's tree and
//! bytes, every tree stored in each format both ways round, the deep values
//! of `unityrs_testkit::depth`, and LZ4 blocks of every output. Nothing comes
//! from a corpus. One folder per target:
//!
//! ```text
//! cargo run -p unityrs-testkit --example fuzz_seeds -- DIR
//! cargo +nightly fuzz run TARGET CORPUS DIR/TARGET    # in fuzz/
//! ```
//!
//! `DIR` must be empty or new. The seeds are generated, never committed:
//! CI's fuzz workflow runs this before each target.
//!
//! The core splits what the builders made (a bundle into its files, a file
//! into its objects); the inputs are the builders' alone.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use unityrs_core::budget::Reservation;
use unityrs_core::detect::{FileType, check_file_type};
use unityrs_core::files::{BundleFile, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::stream::Endian;
use unityrs_core::typetree::NodeList;
use unityrs_core::typetree::commonstring::COMMON_STRINGS;
use unityrs_core::{FileContext, RawStr};
use unityrs_testkit::blob::{NodeSpec, Strings, blob_tree, old_tree};
use unityrs_testkit::bundle::{TestCodec, compress};
use unityrs_testkit::depth::{Depth, Wrap, Z_ROWS};
use unityrs_testkit::fuzz::{RefType, Row, TreeCase, blob_case, lz4_case};

/// The targets, each a folder of `DIR`.
const TARGETS: [&str; 8] = [
    "detect",
    "bundle",
    "serialized",
    "typetree",
    "boost_read",
    "python_read",
    "write_after_read",
    "lz4",
];

/// Old-format and blob-format tree formats a stored tree is written in:
/// format 2 (with `m_VariableCount`), 3 (no index, no meta flag), the other
/// old formats, the blob at 10 and from 12, with `m_RefTypeHash` from 19.
const TREE_FORMATS: [u8; 10] = [2, 3, 5, 9, 11, 10, 12, 15, 19, 22];

/// Bytes an LZ4 seed compresses at most of an output.
const LZ4_MAX: usize = 16 << 10;

struct Seeds {
    dir: PathBuf,
    count: [usize; TARGETS.len()],
    trees: BTreeSet<Vec<u8>>,
    nodes: Vec<Vec<NodeSpec>>,
}

impl Seeds {
    fn add(&mut self, target: &str, name: &str, bytes: &[u8]) -> std::io::Result<()> {
        let t = TARGETS.iter().position(|x| *x == target).unwrap_or(0);
        let n = self.count[t];
        self.count[t] += 1;
        let name: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .take(96)
            .collect();
        std::fs::write(self.dir.join(target).join(format!("{n:05}-{name}")), bytes)
    }

    /// One object's tree and bytes, for the three value targets.
    fn value(&mut self, name: &str, case: &TreeCase) -> std::io::Result<()> {
        let bytes = case.encode();
        self.add("boost_read", name, &bytes)?;
        self.add("python_read", name, &bytes)?;
        self.add("write_after_read", name, &bytes)?;
        let mut python = case.clone();
        python.flags |= TreeCase::PYTHON;
        self.add("write_after_read", &format!("{name}-py"), &python.encode())
    }

    /// A tree, kept once, for the `typetree` target.
    fn tree(&mut self, nodes: Vec<NodeSpec>) {
        let key: Vec<u8> = nodes
            .iter()
            .flat_map(|n| {
                let mut k = vec![n.level];
                k.extend_from_slice(&n.ty);
                k.push(0);
                k.extend_from_slice(&n.name);
                k.push(0);
                k
            })
            .collect();
        if self.trees.insert(key) {
            self.nodes.push(nodes);
        }
    }

    /// A file the builders made, or one a bundle holds: the targets that
    /// read it, and what it holds.
    fn file(&mut self, name: &str, bytes: &[u8], depth: u32) -> std::io::Result<()> {
        self.add("detect", name, bytes)?;
        let ctx = Arc::new(FileContext::unitypy());
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        let data = unityrs_core::Bytes::copy_from_slice(bytes);
        match check_file_type(bytes, &ctx.limits) {
            Ok(FileType::BundleFile) => {
                self.add("bundle", name, bytes)?;
                let Ok(b) = BundleFile::open(RawStr::from(name), data, None, &meter, &ctx) else {
                    return Ok(());
                };
                for (i, e) in b.directory().iter().enumerate() {
                    if let (Ok(entry), true) = (b.entry_bytes(i), depth < 4) {
                        let path = String::from_utf8_lossy(e.path.as_bytes()).into_owned();
                        self.file(&format!("{name}~{path}"), &entry, depth + 1)?;
                    }
                }
            }
            // A resource file too: detection takes an old format's
            // SerializedFile for one, and the target parses whatever it gets.
            Ok(FileType::AssetsFile | FileType::ResourceFile) => {
                self.add("serialized", name, bytes)?;
                if let Ok(f) = SerializedFile::parse(RawStr::from(name), data, None, &meter, &ctx) {
                    self.objects(name, &f)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn objects(&mut self, name: &str, f: &Arc<SerializedFile>) -> std::io::Result<()> {
        let mut flags = 0;
        if f.header().endian == Endian::Big {
            flags |= TreeCase::BIG_ENDIAN;
        }
        if f.ref_types().is_none() {
            flags |= TreeCase::REFS_ABSENT;
        }
        let refs: Vec<RefType> = f
            .ref_types()
            .unwrap_or(&[])
            .iter()
            .map(|t| RefType {
                class: raw(t.class_name.as_ref()),
                namespace: raw(t.namespace.as_ref()),
                assembly: raw(t.assembly.as_ref()),
                rows: t.nodes.as_deref().map(rows),
            })
            .collect();
        for t in f.types().iter().chain(f.ref_types().unwrap_or(&[])) {
            if let Some(nodes) = &t.nodes {
                self.tree(specs(nodes));
            }
        }
        for o in f.objects() {
            let (Some(nodes), Ok(data)) = (
                o.serialized_type().and_then(|t| t.nodes.as_deref()),
                o.raw_data(),
            ) else {
                continue;
            };
            let case = TreeCase {
                flags,
                start: 0,
                rows: rows(nodes),
                refs: refs.clone(),
                data: data.to_vec(),
            };
            self.value(&format!("{name}~{}", o.path_id()), &case)?;
        }
        Ok(())
    }
}

fn raw(s: Option<&RawStr>) -> Vec<u8> {
    s.map(|s| s.as_bytes().to_vec()).unwrap_or_default()
}

/// A meta flag's low 32 bits, as a blob or an old-format tree stores it.
fn low32(m: i64) -> i32 {
    let b = m.to_le_bytes();
    i32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn rows(nodes: &NodeList) -> Vec<Row> {
    nodes
        .rows()
        .iter()
        .map(|r| {
            Row::new(
                u8::try_from(r.level).unwrap_or(u8::MAX),
                r.ty.as_bytes(),
                r.name.as_bytes(),
                r.meta_flag.map_or(0, low32),
            )
        })
        .collect()
}

fn specs(nodes: &NodeList) -> Vec<NodeSpec> {
    nodes
        .rows()
        .iter()
        .map(|r| {
            let mut n = NodeSpec::new(u8::try_from(r.level).unwrap_or(u8::MAX), "", "");
            n.ty = r.ty.as_bytes().to_vec();
            n.name = r.name.as_bytes().to_vec();
            n.byte_size = r.byte_size;
            n.version = r.version;
            n.type_flags = r.type_flags.unwrap_or(0);
            n.meta_flag = r.meta_flag.map_or(0, low32);
            n.ref_type_hash = r.ref_type_hash.unwrap_or(0);
            n.index = r.index;
            n
        })
        .collect()
}

/// `depth`'s shapes, a few levels deep, through ref types `A` and `Z`.
fn depth_cases() -> Vec<(String, TreeCase)> {
    let to_rows = |rows: &[(u8, &str, &str, i32)]| -> Vec<Row> {
        rows.iter()
            .map(|(l, t, n, m)| Row::new(*l, t.as_bytes(), n.as_bytes(), *m))
            .collect()
    };
    let shapes = [
        Wrap::Class,
        Wrap::Vector,
        Wrap::Aligned,
        Wrap::ClassArray,
        Wrap::Pair,
        Wrap::PairArray,
    ];
    let mut out = Vec::new();
    for (i, w) in shapes.iter().enumerate() {
        let next = shapes[(i + 1) % shapes.len()];
        for wraps in [vec![*w], vec![*w, next]] {
            let d = Depth::new(&wraps);
            let a = to_rows(&d.rows);
            let refs = vec![
                RefType {
                    class: b"A".to_vec(),
                    namespace: b"n".to_vec(),
                    assembly: b"a".to_vec(),
                    rows: Some(a.clone()),
                },
                RefType {
                    class: b"Z".to_vec(),
                    namespace: b"n".to_vec(),
                    assembly: b"a".to_vec(),
                    rows: Some(to_rows(&Z_ROWS)),
                },
            ];
            for n in [1, 3] {
                let case = TreeCase {
                    flags: 0,
                    start: 0,
                    rows: a.clone(),
                    refs: refs.clone(),
                    data: d.data(n),
                };
                out.push((format!("depth-{wraps:?}-{n}"), case));
            }
        }
    }
    out
}

fn write(dir: &Path) -> std::io::Result<[usize; TARGETS.len()]> {
    for t in TARGETS {
        std::fs::create_dir_all(dir.join(t))?;
    }
    let mut s = Seeds {
        dir: dir.to_path_buf(),
        count: [0; TARGETS.len()],
        trees: BTreeSet::new(),
        nodes: Vec::new(),
    };
    let outputs = unityrs_testkit::mutate::builder_outputs();
    for (name, bytes) in &outputs {
        s.file(name, bytes, 0)?;
        let plain = &bytes[..bytes.len().min(LZ4_MAX)];
        let block = compress(TestCodec::Lz4, plain);
        let exact = u32::try_from(plain.len()).unwrap_or(u32::MAX);
        for (tag, declared) in [
            ("exact", exact),
            ("over", exact.saturating_add(1)),
            ("short", exact.saturating_sub(1)),
            ("zero", 0),
        ] {
            s.add("lz4", &format!("{name}-{tag}"), &lz4_case(declared, &block))?;
        }
    }
    // `lz4.block.compress(b"")`, the empty block, declared 0 and 1.
    s.add("lz4", "empty", &lz4_case(0, &[0]))?;
    s.add("lz4", "empty-over", &lz4_case(1, &[0]))?;
    for (name, case) in depth_cases() {
        for r in case.refs.iter().filter_map(|r| r.rows.as_ref()) {
            let nodes = r
                .iter()
                .map(|x| {
                    let mut n = NodeSpec::new(x.level, "", "");
                    n.ty.clone_from(&x.ty);
                    n.name.clone_from(&x.name);
                    n.meta_flag = x.meta_flag;
                    n
                })
                .collect();
            s.tree(nodes);
        }
        s.value(&name, &case)?;
    }
    let nodes = std::mem::take(&mut s.nodes);
    for (i, tree) in nodes.iter().enumerate() {
        for format in TREE_FORMATS {
            for big in [false, true] {
                let f = u32::from(format);
                let bytes = if unityrs_core::typetree::parse::is_blob_format(f) {
                    blob_tree(tree, f, big, Strings::Local, &[])
                } else {
                    old_tree(tree, f, big)
                };
                let end = if big { "be" } else { "le" };
                s.add(
                    "typetree",
                    &format!("t{i}-f{format}-{end}"),
                    &blob_case(format, big, &bytes),
                )?;
            }
        }
        let common = blob_tree(tree, 19, false, Strings::Common, COMMON_STRINGS);
        s.add(
            "typetree",
            &format!("t{i}-f19-common"),
            &blob_case(19, false, &common),
        )?;
    }
    Ok(s.count)
}

fn main() -> ExitCode {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [dir] = args.as_slice() else {
        eprintln!("usage: fuzz_seeds DIR (an empty or new folder)");
        return ExitCode::from(2);
    };
    if std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some()) {
        eprintln!("fuzz_seeds: {} is not empty", dir.display());
        return ExitCode::from(2);
    }
    match write(dir) {
        Ok(count) => {
            for (t, n) in TARGETS.iter().zip(count) {
                println!("{t}: {n} seeds");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("fuzz_seeds: {e}");
            ExitCode::FAILURE
        }
    }
}
