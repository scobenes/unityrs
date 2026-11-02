//! The whole dump: `tools/parity/unitypy_dump.py`'s `main` and `run_jobs`,
//! ported.
//!
//! - **Order and `-j N`.** Inputs are dumped by up to `N` workers at once
//!   and written in input order ([`ordered`]), with at most `2N` finished or
//!   running at a time, as `run_jobs` keeps them, so the output does not
//!   depend on `N`.
//! - **Memory.** The window bounds how many inputs are held, and each
//!   input's meter what one holds: its parse and its records together, the
//!   records charged as they are made and returned once they are written
//!   ([`crate::input`]), so at most `2N` times `max_input_memory` (plus each
//!   worker's stack, [`worker_stack`]). The run has no bound over its
//!   inputs together (the batch engine's budget, plan section 4.1): a
//!   shared budget would make an input's record depend on what the others
//!   held at the time, and so on `N`.
//! - **An input's records are written as they are**, line by line, never
//!   copied into one buffer first.
//! - **Output.** The dump goes to `OUT.partial` and is renamed to `OUT` when
//!   complete, so a dump at the requested path is always whole; a `.partial`
//!   left by a killed run is removed first. A path ending in `.gz` is gzip
//!   at level 9 with mtime 0 (the header `1f8b0800 00000000 02ff`, as the
//!   oracle's `GzipFile` writes it). Like the oracle, it refuses to write
//!   into the repository (a dump names corpus inputs, and this repository is
//!   public), through a symlink, or over an input or the index.
//! - **Failures.** An input that cannot be opened or read (an I/O error)
//!   and an input whose sha256 differs from what its index entry pins, or
//!   was not taken (unless allowed; [`crate::read`]), stop the run with no
//!   dump written, as the oracle's `InputUnreadable` and `IndexMismatch` do.
//!   So does a panic while dumping one (a bug: the core does not panic on
//!   input), where the oracle's own bug costs only its input (a `dumper`
//!   record, `CANONICAL_DUMP.md` section 4.2). An input refused at a limit,
//!   or too large for its buffer to be allocated, is its own record and the
//!   run goes on ([`crate::read`]).

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::os::unix::fs::MetadataExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use flate2::Compression;
use flate2::GzBuilder;
use flate2::write::GzEncoder;
use unityrs_core::Bytes;
use unityrs_core::limits::{Limits, Meter};

use crate::json::hex;
use crate::read::{ReadInput, read_input};
use crate::select::{Input, Pin, Selection, collect};
use crate::{DumpOptions, InputDump, end_line, header_line_unbounded};

/// The producer name the header carries.
pub const PRODUCER: &str = "unityrs";

/// Why a run stopped, with the exit status the oracle gives the same stop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunError {
    /// Bad arguments or paths (argparse's `ap.error`: status 2).
    Usage(String),
    /// The dump itself failed (`InputUnreadable`, `IndexMismatch`, an input
    /// that does not exist: status 1).
    Failed(String),
}

impl RunError {
    /// The process exit status.
    #[must_use]
    pub fn status(&self) -> i32 {
        match self {
            Self::Usage(_) => 2,
            Self::Failed(_) => 1,
        }
    }
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Usage(m) | Self::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for RunError {}

const _: () = crate::send_sync::<RunError>();

/// What `unityrs dump` was asked for.
#[derive(Clone, Debug)]
pub struct RunOptions {
    /// The inputs and the selection flags.
    pub selection: Selection,
    /// The dump's path (`-o`), outside the repository.
    pub output: PathBuf,
    /// Levels, jobs, context.
    pub dump: DumpOptions,
    /// `--stats`: per-input timings and totals as JSON.
    pub stats: Option<PathBuf>,
    /// `-q`: no per-input progress on stderr.
    pub quiet: bool,
    /// `--allow-index-mismatch`: dump an input whose sha256 differs from its
    /// index entry (a warning) instead of stopping.
    pub allow_index_mismatch: bool,
}

const _: () = crate::send_sync::<RunOptions>();

/// What a run wrote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    /// Inputs dumped.
    pub inputs: u64,
    /// `file` records.
    pub files: u64,
    /// `object` records.
    pub objects: u64,
    /// Inputs whose sha256 differed from their index entry (allowed).
    pub index_sha256_mismatch: u64,
    /// Inputs whose one record is a `crash`: they ran their dumper out of
    /// memory ([`InputDump::crashes`]).
    pub crashes: u64,
}

const _: () = crate::send_sync::<Summary>();

impl Summary {
    /// Every input crashed: the dump is written, but the oracle exits 3
    /// after it (`unitypy_dump.py` `main`: "One crash is an input's; every
    /// input crashing is the machine's"), and `unityrs dump` does too.
    #[must_use]
    pub fn every_input_crashed(&self) -> bool {
        self.inputs > 0 && self.crashes == self.inputs
    }
}

// --------------------------------------------------------------- the paths

/// The path `path` names once `..` and symlinks are resolved: each existing
/// prefix is canonicalised (a dangling symlink followed to where it points),
/// and components that do not exist yet are applied as written, which is
/// how `create_dir_all` and `File::create` will create them (the P0 spike's
/// `resolve`, which Python's `Path.resolve()` matches for these checks).
pub fn resolve(path: &Path) -> Result<PathBuf, String> {
    resolve_at(path, 0)
}

fn resolve_at(path: &Path, depth: u32) -> Result<PathBuf, String> {
    if depth > 40 {
        return Err(format!("{}: too many levels of symlinks", path.display()));
    }
    let abs = std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut p = PathBuf::new();
    let mut rest = abs.components();
    while let Some(c) = rest.next() {
        match c {
            Component::Prefix(_) | Component::RootDir => p.push(c),
            Component::CurDir => {}
            Component::ParentDir => {
                p.pop();
            }
            Component::Normal(name) => {
                p.push(name);
                if let Ok(real) = std::fs::canonicalize(&p) {
                    p = real;
                } else if let Ok(target) = std::fs::read_link(&p) {
                    // A dangling symlink: carry on from where it points.
                    p.pop();
                    let mut next = p.join(target);
                    next.extend(rest);
                    return resolve_at(&next, depth + 1);
                }
            }
        }
    }
    Ok(p)
}

/// The git repository the resolved path `resolved` lies in, if any: its
/// nearest ancestor holding a `.git`, a clone's folder or a worktree's file
/// (`canonical.py`'s `repo_roots` lists the parity tools' own; this is
/// stricter, any repository). This crate's own checkout is one of them
/// wherever it is, and nothing about where the dump was built is kept.
fn repo_root_of(resolved: &Path) -> Option<PathBuf> {
    resolved
        .ancestors()
        .find(|a| a.join(".git").exists())
        .map(Path::to_path_buf)
}

/// Refuses a path inside a repository (`canonical.py` `inside_repo`, made
/// stricter as the spike's: any repository, not only this one).
pub fn outside_repo(path: &Path) -> Result<(), String> {
    let real = resolve(path)?;
    match repo_root_of(&real) {
        Some(root) => Err(format!(
            "refusing to write into a repository ({}); pick a path outside it",
            root.display()
        )),
        None => Ok(()),
    }
}

/// `output_path`: the folder resolved, the name kept, so a symlink at the
/// name is seen rather than followed.
pub fn output_path(given: &Path) -> Result<PathBuf, String> {
    let name = given
        .file_name()
        .ok_or_else(|| format!("{}: not a file name", given.display()))?;
    let parent = match given.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    Ok(resolve(&parent)?.join(name))
}

/// `(st_dev, st_ino)` of the file at `path`, which every hard link to it
/// shares, or `None` when there is none.
fn identity(path: &Path, follow: bool) -> Option<(u64, u64)> {
    let m = if follow {
        std::fs::metadata(path)
    } else {
        std::fs::symlink_metadata(path)
    };
    m.ok().map(|m| (m.dev(), m.ino()))
}

/// `path + ".partial"`.
fn partial_of(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    path.with_file_name(name)
}

// ---------------------------------------------------------------- the writer

/// A gzip stream at level 9 with mtime 0 and no file name: the header the
/// oracle's `GzipFile(filename="", mtime=0)` writes.
pub fn gzip<W: Write>(w: W) -> GzEncoder<W> {
    GzBuilder::new().mtime(0).write(w, Compression::best())
}

/// The output stream. The encoder is boxed (`clippy::large_enum_variant`):
/// zlib-rs's deflate stream, which flate2 holds inline, makes it many
/// times a `BufWriter`'s size.
enum Out {
    Plain(BufWriter<File>),
    Gz(Box<GzEncoder<BufWriter<File>>>),
}

impl Write for Out {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Plain(w) => w.write(buf),
            Self::Gz(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Plain(w) => w.flush(),
            Self::Gz(w) => w.flush(),
        }
    }
}

impl Out {
    fn finish(self) -> std::io::Result<()> {
        let mut file = match self {
            Self::Plain(w) => w,
            Self::Gz(w) => (*w).finish()?,
        };
        file.flush()?;
        file.into_inner()
            .map_err(std::io::IntoInnerError::into_error)?
            .sync_all()
    }
}

// ------------------------------------------------------------- the ordering

fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic".to_owned())
}

/// The stack one level of container nesting takes, in the core's eager load
/// and the dump's own walks of the nest (load, pre-pass, records), up to the
/// deepest nest [`crate::LIFTED_CONTAINER_DEPTH`] lets parse. It was set at
/// about twice the 5,488 bytes a level cost in a debug build and 1,520 in a
/// release one; since the records of each file are made outside the
/// recursion's frame (`input/`, `Writer::emit`), `tests/limits.rs`'s
/// `container_stack_measured` finds 1,328 and 320 (toolchain 1.99.1;
/// `docs/plans/core.md` section 6 records each integrated head's), eagerly
/// and through the reverse pre-pass alike, and
/// `the_container_term_pays_for_every_level` checks that this term alone,
/// on the least stack of the nest's leaf, holds the deepest nest.
pub const CONTAINER_LEVEL_STACK: usize = if cfg!(debug_assertions) {
    12 << 10
} else {
    3 << 10
};

/// The stack a worker dumps an input on, under `limits`: what reading and
/// writing one object needs ([`Limits::stack_needed`]), and as much again
/// for the dump's own frames around it (`tests/deep_values.rs` dumps the
/// deepest inputs at `-j 1` and `-j 8`; the value encoder adds no frame a
/// level, since it walks a value with a heap stack of its own: review round
/// 11, `encode`'s tests), on top of
/// [`CONTAINER_LEVEL_STACK`] for each level of nesting
/// `limits.max_container_depth` lets an input reach, inside which that
/// object is read (`tests/limits.rs`).
#[must_use]
pub fn worker_stack(limits: &Limits) -> usize {
    let levels = usize::try_from(limits.max_container_depth)
        .unwrap_or(usize::MAX)
        .saturating_add(1);
    limits
        .stack_needed()
        .saturating_mul(2)
        .saturating_add(levels.saturating_mul(CONTAINER_LEVEL_STACK))
}

/// [`ordered_on`] workers of [`worker_stack`] at the default limits.
pub fn ordered<T: Sync, R: Send>(
    items: &[T],
    jobs: usize,
    f: &(dyn Fn(&T) -> R + Sync),
    sink: &mut dyn FnMut(usize, R) -> Result<(), RunError>,
) -> Result<(), RunError> {
    ordered_on(items, jobs, worker_stack(&Limits::default()), f, sink)
}

/// `f` over `items` on `jobs` workers of `stack` bytes each, each result
/// handed to `sink` in item order (`run_jobs`): at most `2 * jobs` items are
/// running or waiting at once. A panic in `f` stops the run with an error
/// naming the item; so does an error from `sink`. `jobs` 1 runs on one
/// worker too, so the stack an input is read on never depends on the
/// caller's. No more workers start than there are items (`run_jobs` forks no
/// more processes than inputs), so a large `-j` costs nothing.
pub fn ordered_on<T: Sync, R: Send>(
    items: &[T],
    jobs: usize,
    stack: usize,
    f: &(dyn Fn(&T) -> R + Sync),
    sink: &mut dyn FnMut(usize, R) -> Result<(), RunError>,
) -> Result<(), RunError> {
    let jobs = jobs.clamp(1, items.len().max(1));
    let died = |i: usize, p: &(dyn std::any::Any + Send)| {
        RunError::Failed(format!(
            "input {} of {}: the dumper panicked: {}",
            i + 1,
            items.len(),
            panic_message(p)
        ))
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .stack_size(stack)
        .thread_name(|i| format!("unityrs-dump-{i}"))
        .build()
        .map_err(|e| RunError::Failed(format!("cannot start {jobs} workers: {e}")))?;
    let window = jobs.saturating_mul(2);
    let (tx, rx) = std::sync::mpsc::channel::<(usize, std::thread::Result<R>)>();
    pool.in_place_scope(|s| {
        let mut started = 0usize;
        let mut finished: BTreeMap<usize, R> = BTreeMap::new();
        for want in 0..items.len() {
            loop {
                while started < items.len() && started < want + window {
                    let (i, tx) = (started, tx.clone());
                    if let Some(item) = items.get(i) {
                        s.spawn(move |_| {
                            let r = catch_unwind(AssertUnwindSafe(|| f(item)));
                            // The receiver outlives the scope.
                            let _ = tx.send((i, r));
                        });
                    }
                    started += 1;
                }
                if let Some(r) = finished.remove(&want) {
                    sink(want, r)?;
                    break;
                }
                let (i, r) = rx
                    .recv()
                    .map_err(|e| RunError::Failed(format!("a worker vanished: {e}")))?;
                match r {
                    Ok(r) => {
                        finished.insert(i, r);
                    }
                    Err(p) => return Err(died(i, &*p)),
                }
            }
        }
        Ok(())
    })
}

// ------------------------------------------------------------------ the dump

/// One input's outcome, as a worker hands it back.
type Dumped = Result<InputDump, String>;

/// What makes one input's records: from its name, its bytes and its meter,
/// which the bytes are charged to ([`crate::input::dump_on`], or a test's).
type DumpOne<'a> = dyn Fn(&[u8], Bytes, &Arc<Meter>) -> InputDump + Sync + 'a;

/// Writes the header, every input's records in order, and the `end` record
/// to `w`. Each input is read by [`read_input`] (refused at its limits, its
/// bytes charged to its meter, a refusal its own record), and `dump_one`
/// makes its records from its name, its bytes and that meter; `on_input`
/// sees each input's records after they are written and may stop the run.
/// Returns the `file` and `object` counts.
pub fn write_dump(
    inputs: &[Input],
    opts: &DumpOptions,
    w: &mut dyn Write,
    on_input: &mut dyn FnMut(&Input, &InputDump) -> Result<(), RunError>,
    dump_one: &DumpOne<'_>,
) -> Result<(u64, u64), RunError> {
    let io = |e: std::io::Error| RunError::Failed(format!("writing the dump: {e}"));
    w.write_all(header_line_unbounded(opts.levels, PRODUCER, &opts.unbounded).as_bytes())
        .and_then(|()| w.write_all(b"\n"))
        .map_err(io)?;
    let (mut files, mut objects) = (0u64, 0u64);
    let work = |input: &Input| -> Dumped {
        match read_input(&input.name, &input.path, &opts.context) {
            Ok(ReadInput::Bytes { data, meter }) => Ok(dump_one(&input.name, data, &meter)),
            Ok(ReadInput::Refused(d)) => Ok(d),
            Err(e) => Err(format!("{}: {e}", String::from_utf8_lossy(&input.name))),
        }
    };
    let stack = worker_stack(&opts.context.limits);
    ordered_on(inputs, opts.jobs, stack, &work, &mut |i, r: Dumped| {
        let d = r.map_err(|e| {
            RunError::Failed(format!("{e}: the dumper itself failed; no dump written"))
        })?;
        for line in &d.lines {
            w.write_all(line.as_bytes())
                .and_then(|()| w.write_all(b"\n"))
                .map_err(io)?;
        }
        files += d.files;
        objects += d.objects;
        match inputs.get(i) {
            Some(input) => on_input(input, &d),
            None => Ok(()),
        }
    })?;
    w.write_all(end_line(files, objects).as_bytes())
        .and_then(|()| w.write_all(b"\n"))
        .map_err(io)?;
    Ok((files, objects))
}

/// `unitypy_dump.py`'s `main`, after argument parsing: checks the paths,
/// selects the inputs, writes the dump to its `.partial` file and renames
/// it, then writes the stats.
pub fn run(opts: &RunOptions) -> Result<Summary, RunError> {
    run_with(opts, &|rel, data, meter| {
        crate::input::dump_on(rel, data, meter, &opts.dump)
    })
}

/// [`run`] with another per-input dumper (a test's).
#[allow(clippy::too_many_lines)] // one linear pass, as the oracle's main
pub fn run_with(opts: &RunOptions, dump_one: &DumpOne<'_>) -> Result<Summary, RunError> {
    let usage = RunError::Usage;
    // The dump and the stats both name corpus inputs; neither may land in a repository.
    for (label, path) in [
        ("the dump", Some(&opts.output)),
        ("--stats", opts.stats.as_ref()),
    ] {
        if let Some(path) = path {
            outside_repo(path).map_err(|e| usage(format!("{label}: {e}")))?;
        }
    }
    let out_path = output_path(&opts.output).map_err(usage)?;
    let tmp = partial_of(&out_path);
    let stats_path = opts
        .stats
        .as_deref()
        .map(output_path)
        .transpose()
        .map_err(usage)?;
    let mut writes: Vec<(PathBuf, &str)> = vec![
        (out_path.clone(), "the dump"),
        (tmp.clone(), "the dump's .partial file"),
    ];
    if let Some(sp) = &stats_path {
        if let Some((_, what)) = writes.iter().find(|(p, _)| p == sp) {
            return Err(usage(format!(
                "--stats names {what} ({}); pick another path for the stats",
                sp.display()
            )));
        }
        writes.push((sp.clone(), "--stats"));
    }
    let mut written: HashMap<(u64, u64), &str> = HashMap::new();
    for (path, label) in &writes {
        if std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(usage(format!(
                "{label} ({}) is a symlink; pick a path that is not one",
                path.display()
            )));
        }
        if let Some(id) = identity(path, false) {
            if let Some(other) = written.get(&id) {
                return Err(usage(format!(
                    "{label} ({}) is the same file as {other}; pick another path",
                    path.display()
                )));
            }
            written.insert(id, label);
        }
    }

    let inputs = collect(&opts.selection).map_err(RunError::Failed)?;
    if inputs.is_empty() {
        return Err(usage("no inputs matched".into()));
    }
    let index_files: Vec<&PathBuf> = if opts.selection.index {
        opts.selection.inputs.iter().collect()
    } else {
        Vec::new()
    };
    for path in index_files
        .iter()
        .copied()
        .chain(inputs.iter().map(|i| &i.path))
    {
        if let Some(clash) = identity(path, true).and_then(|id| written.get(&id)) {
            return Err(usage(format!(
                "{clash} would overwrite {}; pick a path that is not an input",
                path.display()
            )));
        }
    }

    // What each name's entry pins: a repeated name takes its last entry's,
    // one that pins nothing included, as the oracle's dict does.
    let expected: HashMap<&[u8], Option<&Pin>> = inputs
        .iter()
        .map(|i| (i.name.as_slice(), i.sha256.as_ref()))
        .collect();

    if let Some(dir) = out_path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| RunError::Failed(format!("{}: {e}", dir.display())))?;
    }
    match std::fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(RunError::Failed(format!("{}: {e}", tmp.display()))),
    }
    // O_CREAT | O_EXCL: nothing already at the path, a symlink included, is
    // written through.
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| {
            usage(format!(
                "the dump's .partial file ({}) could not be created ({e}); try again",
                tmp.display()
            ))
        })?;
    let buf = BufWriter::with_capacity(1 << 20, file);
    let mut out = if out_path.extension().is_some_and(|e| e == "gz") {
        Out::Gz(Box::new(gzip(buf)))
    } else {
        Out::Plain(buf)
    };

    let t0 = Instant::now();
    let total = inputs.len();
    let mut summary = Summary {
        inputs: u64::try_from(total).unwrap_or(u64::MAX),
        ..Summary::default()
    };
    let mut per_input = Vec::new();
    let mut done = 0usize;
    let mut on_input = |input: &Input, d: &InputDump| -> Result<(), RunError> {
        done += 1;
        summary.crashes += d.crashes;
        let name = String::from_utf8_lossy(&input.name).into_owned();
        let differs = |want: &Pin| match d.sha256.map(|s| hex(&s)) {
            Some(sha) if want.matches(&sha) => None,
            Some(sha) => Some(format!(
                "{name}: sha256 {sha} differs from the index's {want}"
            )),
            // Not taken (`crate::read`): it cannot be the file the pin names.
            None => Some(format!(
                "{name}: its sha256 was not taken (hashing it would pass its max_decode_work), \
                 so it cannot be the index's {want}"
            )),
        };
        if let Some(said) = expected
            .get(input.name.as_slice())
            .copied()
            .flatten()
            .and_then(differs)
        {
            if !opts.allow_index_mismatch {
                return Err(RunError::Failed(format!(
                    "{said}\nunityrs dump: the input is not the file the index pins; no dump written \
                     (--allow-index-mismatch dumps it anyway)"
                )));
            }
            summary.index_sha256_mismatch += 1;
            eprintln!("warning: {said}");
        }
        let bytes_out: usize = d.lines.iter().map(|l| l.len() + 1).sum();
        if !opts.quiet {
            eprintln!(
                "[{done}/{total}] {name}: {} objects, {} B in, {bytes_out} B out",
                d.objects, d.bytes_in
            );
        }
        if opts.stats.is_some() {
            per_input.push(serde_json::json!({
                "path": name,
                "files": d.files,
                "objects": d.objects,
                "bytes_in": d.bytes_in,
                "bytes_out": bytes_out,
            }));
        }
        Ok(())
    };
    let result =
        write_dump(&inputs, &opts.dump, &mut out, &mut on_input, dump_one).and_then(|counts| {
            out.finish()
                .map_err(|e| RunError::Failed(format!("writing the dump: {e}")))?;
            Ok(counts)
        });
    let (files, objects) = match result {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };
    std::fs::rename(&tmp, &out_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        RunError::Failed(format!("{}: {e}", out_path.display()))
    })?;
    summary.files = files;
    summary.objects = objects;
    let report = serde_json::json!({
        "inputs": summary.inputs,
        "files": files,
        "objects": objects,
        "index_sha256_mismatch": summary.index_sha256_mismatch,
        "crashes": summary.crashes,
        "wall_s": (t0.elapsed().as_secs_f64() * 1000.0).round() / 1000.0,
        "output_bytes": std::fs::metadata(&out_path).map_or(0, |m| m.len()),
    });
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&report).unwrap_or_default()
    );
    if let Some(sp) = &stats_path {
        let body = serde_json::json!({"summary": report, "inputs": per_input});
        write_new(
            sp,
            serde_json::to_string_pretty(&body)
                .unwrap_or_default()
                .as_bytes(),
        )
        .map_err(|e| RunError::Failed(format!("{}: {e}", sp.display())))?;
    }
    Ok(summary)
}

/// `path := data`, through a new file beside it renamed over it, so no file
/// or symlink at `path` is written through (`unitypy_dump.py` `write_new`).
fn write_new(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().unwrap_or_default());
    name.push(format!(".{}.partial", std::process::id()));
    let tmp = path.with_file_name(name);
    let r = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

#[cfg(test)]
mod tests;
