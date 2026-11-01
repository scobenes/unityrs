//! Which inputs a dump reads, and what it names them: a port of
//! `tools/parity/unitypy_dump.py`'s `collect_inputs` and its `--name`
//! globs (`fnmatch.fnmatchcase`), so both producers dump the same inputs
//! under the same names in the same order (`docs/CANONICAL_DUMP.md` section
//! 4.2 "Names", section 5). `tests/fixtures/dump/select.json` records the
//! oracle's selections over a synthetic tree and index, Python's
//! `fnmatchcase` over names and patterns, invalid UTF-8 included, and what
//! the oracle's run makes of an index entry's `sha256` of every JSON kind
//! ([`Pin`]) and of an index whose `files` is no list of objects.
//!
//! Names are bytes throughout: a path's name is its bytes as given (Python's
//! `os.fsencode` of the surrogate-escaped `str`), never resolved, so a
//! symlink is named after itself, and inputs sort by those bytes.
//!
//! A folder is walked as Python 3.11's `os.walk` walks it, one open folder
//! at a time and no deeper than [`MAX_FOLDER_DEPTH`]:
//! `tests/fixtures/dump/folders.json` records the oracle's runs on deep
//! folders (`tools/fixtures/folders.py`).
//!
//! An index is read as Python's `json.loads` reads it ([`index`]), and
//! `--kind`, `--build` and an entry's `path` are compared and named as the
//! Python `str`s they are ([`PyStr`]): `tests/fixtures/dump/index.json`
//! records the oracle's run on every kind of index text, and Python's
//! `str()` of every kind of value `--build` compares
//! (`tools/fixtures/index.py`).

mod glob;
pub mod index;
mod printable;
mod py;

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

pub use glob::{Glob, fnmatchcase, py_units};
pub use index::Value;
pub use printable::is_printable;
pub use py::{PyStr, py_float_repr, py_str};

/// The index entry kinds `--kind` defaults to (`unitypy_dump.py` `INDEX_KINDS`).
pub const INDEX_KINDS: [&str; 2] = ["bundle", "unitypy-sample"];

/// What to dump: the selection flags of `unitypy_dump.py`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    /// The inputs as given: files or folders, or with `index` one corpus
    /// `index.json`. `unitypy_dump.py` takes one; several are collected one
    /// by one and must not repeat a name.
    pub inputs: Vec<PathBuf>,
    /// Each input is a corpus index (`--index`).
    pub index: bool,
    /// Index entry kinds to dump (`--kind`): an entry is dumped when its
    /// `kind` is a `str` equal to one of them.
    pub kinds: Vec<PyStr>,
    /// Index builds to dump (`--build`): an entry is dumped when Python's
    /// `str()` of its `build` ([`py_str`]; `None` when it has none) is
    /// equal to one of them. `None` keeps every build.
    pub builds: Option<Vec<PyStr>>,
    /// Globs on the dump name, any of which keeps an input (`--name`, as
    /// bytes, matched as `fnmatch.fnmatchcase` matches the surrogate-escaped
    /// `str`s).
    pub names: Vec<Vec<u8>>,
    /// Keep this many inputs spread evenly over the sorted list (`--sample`;
    /// 0 keeps all, a negative count none, as Python's truthiness and
    /// `range` have it).
    pub sample: Option<i64>,
    /// Keep the first this many (`--limit`; 0 keeps all, a negative count
    /// drops that many from the end, as Python's `items[:limit]`).
    pub limit: Option<i64>,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            inputs: Vec::new(),
            index: false,
            kinds: INDEX_KINDS.iter().map(|k| PyStr::from(*k)).collect(),
            builds: None,
            names: Vec::new(),
            sample: None,
            limit: None,
        }
    }
}

const _: () = crate::send_sync::<Selection>();

/// One input to dump.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    /// The path to read.
    pub path: PathBuf,
    /// Its dump name: the path below the folder, the file's name, or the
    /// index entry's `path`, as bytes.
    pub name: Vec<u8>,
    /// What its index entry's `sha256` pins ([`Pin`]): `None` for an input
    /// that is no index entry's, or whose entry pins nothing.
    pub sha256: Option<Pin>,
}

const _: () = crate::send_sync::<Input>();

/// What an index entry's `sha256` pins, as the oracle's run tests it
/// (`unitypy_dump.py` `main`: `if expected[rel] and expected[rel] != sha`).
/// A value Python reads as false (`null`, `false`, a zero, `""`, `[]`,
/// `{}`), or no `sha256` at all, pins nothing ([`pin_of`]); any other
/// value pins the input, and a value that is no string matches no digest.
/// Each displays as the oracle's message prints it: Python's `str()` of
/// the value, through `stderr`'s `backslashreplace` ([`PyStr`]'s
/// `Display`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pin {
    /// A string: the input is the file it pins when its sha256, as
    /// lowercase hex, is this string.
    Sha256(PyStr),
    /// Any other value Python reads as true (`true`, a number that is not
    /// zero, `nan`, a list or an object that is not empty): no digest is
    /// equal to it, so no input is the file it pins. Held as Python's
    /// `str()` of it ([`py_str`]: a list's or a dict's `repr`).
    Other(PyStr),
}

const _: () = crate::send_sync::<Pin>();

impl Pin {
    /// Whether an input whose sha256 is `sha` (lowercase hex) is the file
    /// this pins.
    #[must_use]
    pub fn matches(&self, sha: &str) -> bool {
        matches!(self, Self::Sha256(s) if s.as_bytes() == sha.as_bytes())
    }
}

impl std::fmt::Display for Pin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sha256(s) | Self::Other(s) => s.fmt(f),
        }
    }
}

/// The pin an index entry's `sha256` member `v` makes ([`Pin`]): `None`
/// when it is missing or Python reads it as false.
#[must_use]
pub fn pin_of(v: Option<&Value>) -> Option<Pin> {
    match v? {
        v if !v.truthy() => None,
        Value::Str(s) => Some(Pin::Sha256(s.clone())),
        other => Some(Pin::Other(py_str(other))),
    }
}

/// `collect_inputs`: the inputs in canonical order, filtered, sampled and
/// limited (`unitypy_dump.py` `collect_inputs`).
pub fn collect(sel: &Selection) -> Result<Vec<Input>, String> {
    let mut items = Vec::new();
    for given in &sel.inputs {
        let before = items.len();
        collect_one(sel, given, &mut items)?;
        if sel.inputs.len() > 1 {
            // Several inputs: a name two of them give would be two records
            // under one key, which no single run of the oracle writes.
            let (old, new) = items.split_at(before);
            if let Some(dup) = new.iter().find(|n| old.iter().any(|o| o.name == n.name)) {
                return Err(format!(
                    "{} is named by two inputs",
                    String::from_utf8_lossy(&dup.name)
                ));
            }
        }
    }
    if !sel.names.is_empty() {
        let pats: Vec<Glob> = sel
            .names
            .iter()
            .map(|p| Glob::new(p))
            .collect::<Result<_, _>>()?;
        items.retain(|i| pats.iter().any(|p| p.matches(&i.name)));
    }
    // Names order by their bytes, as Rust's OsStr does on Unix (a stable sort,
    // as Python's `list.sort`).
    items.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(s) = sel.sample {
        let n = items.len();
        // `if args.sample and len(items) > args.sample`
        if s != 0 && i64::try_from(n).map_or(true, |n| n > s) {
            let keep = usize::try_from(s).unwrap_or(0);
            let mut out = Vec::with_capacity(keep);
            for k in 0..keep {
                // `items[(k * n) // args.sample]`; k < keep < n, so in range.
                let at = u128::try_from(k).unwrap_or(0) * u128::try_from(n).unwrap_or(0)
                    / u128::try_from(keep).unwrap_or(1);
                if let Some(item) = usize::try_from(at).ok().and_then(|at| items.get(at)) {
                    out.push(item.clone());
                }
            }
            items = out;
        }
    }
    if let Some(l) = sel.limit {
        // `if args.limit: items = items[: args.limit]`
        if l != 0 {
            let n = i64::try_from(items.len()).unwrap_or(i64::MAX);
            let end = if l < 0 { (n + l).max(0) } else { l.min(n) };
            items.truncate(usize::try_from(end).unwrap_or(0));
        }
    }
    Ok(items)
}

fn collect_one(sel: &Selection, given: &Path, items: &mut Vec<Input>) -> Result<(), String> {
    if sel.index {
        let text = std::fs::read(given).map_err(|e| format!("{}: {e}", given.display()))?;
        let index = index::parse(&text).map_err(|e| format!("{}: {e}", given.display()))?;
        // Entries are relative to the index file itself, symlinks resolved.
        let resolved =
            std::fs::canonicalize(given).map_err(|e| format!("{}: {e}", given.display()))?;
        let root = resolved.parent().unwrap_or(Path::new("/"));
        // `for e in index["files"]`: a list's items; an empty dict or `str`
        // is iterable and gives none, and anything else fails, as Python's
        // subscript, its iteration and then `e.get` do on it.
        let files: &[Value] = match index.get("files") {
            Some(Value::List(a)) => a,
            Some(Value::Dict(o)) if o.is_empty() => &[],
            Some(Value::Str(s)) if s.is_empty() => &[],
            _ => return Err(format!("{}: the index has no files list", given.display())),
        };
        for e in files {
            // `e.get(...)`: an entry that is no object fails the run.
            if !matches!(e, Value::Dict(_)) {
                return Err(format!(
                    "{}: an index entry is not an object",
                    given.display()
                ));
            }
            // `e.get("kind") not in args.kind`: only a `str` equals one.
            if !matches!(e.get("kind"), Some(Value::Str(k)) if sel.kinds.contains(k)) {
                continue;
            }
            // `str(e.get("build")) not in args.build`.
            if let Some(builds) = &sel.builds {
                let build = py_str(e.get("build").unwrap_or(&Value::Null));
                if !builds.contains(&build) {
                    continue;
                }
            }
            // `name_bytes(e["path"])`: a `str`, encoded as `os.fsencode`
            // encodes it; a surrogate it cannot encode fails the run.
            let Some(Value::Str(rel)) = e.get("path") else {
                return Err(format!("{}: an index entry has no path", given.display()));
            };
            let name = rel.to_os_bytes().ok_or_else(|| {
                format!(
                    "{}: an index entry's path ({rel}) holds a surrogate that names no byte",
                    given.display()
                )
            })?;
            items.push(Input {
                path: root.join(os_str(&name)),
                name,
                sha256: pin_of(e.get("sha256")),
            });
        }
    } else if std::fs::metadata(given).is_ok_and(|m| m.is_dir()) {
        walk(given, items)?;
    } else if std::fs::metadata(given).is_ok_and(|m| m.is_file()) {
        // `os.path.basename`: everything after the last `/`.
        let bytes = given.as_os_str().as_bytes();
        let base = bytes
            .iter()
            .rposition(|b| *b == b'/')
            .map_or(bytes, |i| bytes.get(i + 1..).unwrap_or_default());
        items.push(Input {
            path: given.to_path_buf(),
            name: base.to_vec(),
            sha256: None,
        });
    } else {
        return Err(format!("no such input: {}", given.display()));
    }
    Ok(())
}

/// The deepest folder below an input folder that the oracle's walk lists:
/// `os.walk` (Python 3.11) recurses one generator frame a level, so
/// listing a folder one level deeper raises `RecursionError` (Python's
/// limit of 1,000 frames, less those below the call) and the run exits 1
/// with no dump. A folder counts wherever `os.walk` lists one: a symlink to
/// a folder, which it never enters, and a folder it cannot read included.
/// Recorded from the oracle's own runs (`tests/fixtures/dump/folders.json`
/// "depth", `tools/fixtures/folders.py`), which `unityrs-cli`'s
/// `tests/folder_runs.rs` replays through [`collect`] and the binary.
pub const MAX_FOLDER_DEPTH: usize = 995;

/// `os.walk(given)` with `followlinks=False`, keeping each entry for which
/// `os.path.isfile` holds: a symlink to a file is kept under its own name, a
/// symlink to a folder is neither walked nor kept, a dangling symlink and
/// anything else that is not a regular file are left out, and a folder that
/// cannot be listed, wholly or part way, is skipped with all it holds, as
/// `os.walk` skips it. A folder listed deeper than [`MAX_FOLDER_DEPTH`]
/// fails the run, as `os.walk`'s `RecursionError` fails the oracle's.
///
/// Like `os.walk`, each folder is listed whole and closed before any folder
/// below it is opened, so the walk holds one open folder at a time at any
/// depth (review round 8: it held one a level, so under a low limit on
/// open files the folders past it were silently skipped).
fn walk(top: &Path, items: &mut Vec<Input>) -> Result<(), String> {
    // The folders still to list: each one's path, its name below `top`
    // and its depth.
    let mut folders = vec![(top.to_path_buf(), Vec::new(), 0_usize)];
    while let Some((dir, rel, depth)) = folders.pop() {
        // `scandir`: a folder that cannot be listed, or whose listing
        // fails part way, yields nothing (`onerror` is `None`). Each entry
        // keeps the folder open while it lives, so only its name and type
        // are kept: the folder is closed here.
        let Ok(listing) = std::fs::read_dir(&dir).and_then(|entries| {
            entries
                .map(|e| e.map(|e| (e.file_name(), e.file_type().ok())))
                .collect::<Result<Vec<_>, _>>()
        }) else {
            continue;
        };
        for (name, kind) in listing {
            let path = dir.join(&name);
            let mut child = rel.clone();
            if !child.is_empty() {
                child.push(b'/');
            }
            child.extend_from_slice(name.as_bytes());
            // `entry.is_dir()`: the entry's own type, a symlink's target's
            // (a type that cannot be read is no folder).
            let is_link = kind.is_some_and(|t| t.is_symlink());
            let is_dir = kind.is_some_and(|t| t.is_dir())
                || (is_link && std::fs::metadata(&path).is_ok_and(|m| m.is_dir()));
            if is_dir {
                if depth + 1 > MAX_FOLDER_DEPTH {
                    return Err(format!(
                        "{}: maximum recursion depth exceeded while walking the folder \
                         (Python lists folders {MAX_FOLDER_DEPTH} levels deep here)",
                        top.display()
                    ));
                }
                if !is_link {
                    folders.push((path, child, depth + 1));
                }
            } else if std::fs::metadata(&path).is_ok_and(|m| m.is_file()) {
                // `os.path.isfile(p)`.
                items.push(Input {
                    path,
                    name: child,
                    sha256: None,
                });
            }
        }
    }
    Ok(())
}

/// A path's bytes as an `OsStr`.
#[must_use]
pub fn os_str(bytes: &[u8]) -> &OsStr {
    OsStr::from_bytes(bytes)
}
