//! `unityrs dump`'s command line, parsed as `tools/parity/unitypy_dump.py`'s
//! `argparse` parser parses its own (Python 3.11): the same options, the same
//! defaults, the same values from the same arguments, and an error (status
//! 2) wherever it errors, with its message.
//!
//! The parser is a port of `argparse.ArgumentParser._parse_known_args` for
//! this option set: `_parse_optional` classifies every argument before any is
//! consumed (so an ambiguous abbreviation is reported first), then
//! `consume_optional` takes each option's value (`--opt V`, `--opt=V`,
//! `-oV`, `-o=V`, a unique prefix of a long option, single-dash flags run
//! together), and `int` options convert as they are taken. Arguments are
//! bytes, as Python's surrogate-escaped `argv` is, so a path or a `--name`
//! glob that is not UTF-8 reaches the run unchanged.
//!
//! Three extensions: the oracle takes one INPUT, `unityrs dump` takes one or
//! more (every positional, wherever it stands; their dump names must not
//! repeat, and `--index` takes one); `--reverse-prepass` (G1c); and
//! `--unbounded` (G3b's limit check, never a gate dump). No option the
//! oracle has is a prefix of either flag's name, nor either of one of its
//! own, so every abbreviation the oracle takes still means what it does
//! there.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use unityrs_core::LimitKind;
use unityrs_dump::select::INDEX_KINDS;
use unityrs_dump::{LIFTABLE, Levels};

pub const USAGE: &str = "usage: unityrs dump [-h] -o OUTPUT [--index] [--kind KIND] [--build BUILD] \
[--allow-index-mismatch] [--name NAME] [--sample SAMPLE] [--limit LIMIT] [--levels LEVELS] \
[-j JOBS] [--stats STATS] [-q] [--tpk TPK] [--reverse-prepass] [--unbounded UNBOUNDED] \
input [input ...]";

pub const HELP: &str = "\
Writes the canonical dump (docs/CANONICAL_DUMP.md, format 1) of files,
folders, or the inputs of a corpus index, as tools/parity/unitypy_dump.py
writes it from UnityPy.

positional arguments:
  input                 files, folders, or (with --index) one corpus index.json

options:
  -h, --help            show this help message and exit
  -o OUTPUT, --output OUTPUT
                        output .jsonl or .jsonl.gz, outside the repository
  --index               INPUT is a corpus index.json
  --kind KIND           index entry kinds to dump (comma list)
  --build BUILD         index builds to dump (comma list)
  --allow-index-mismatch
                        with --index, dump an input whose sha256 differs from
                        its entry (a warning) instead of stopping
  --name NAME           glob on the dump-relative path (repeatable)
  --sample SAMPLE       keep N inputs spread evenly over the sorted list
  --limit LIMIT         keep the first N inputs
  --levels LEVELS       levels to dump (default: L0,L1,L2,L3)
  -j JOBS, --jobs JOBS  inputs dumped at once (default 1)
  --stats STATS         write per-input timings and totals as JSON to this path
  -q, --quiet           no per-input progress on stderr
  --tpk TPK             the UnityPy TPK for stripped objects' typetrees
  --reverse-prepass     before dumping a bundle, parse its entries in reverse
                        directory order, nested bundles' too (the gate's G1c:
                        the output must not change)
  --unbounded UNBOUNDED
                        lift these limits (comma list of InputMemory,
                        InputBytes, BlockOutput, DecodeWork, Inflate,
                        ContainerDepth, TreeDepth, ValueDepth, ValuesPerObject,
                        ZeroSizeElements): G3b's check that a limit refusal is
                        the only difference from UnityPy. Bound the process
                        instead; the header names them, and no gate takes
                        such a dump";

/// What an option stores into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dest {
    Help,
    Output,
    Index,
    Kind,
    Build,
    Allow,
    Name,
    Sample,
    Limit,
    Levels,
    Jobs,
    Stats,
    Quiet,
    Tpk,
    LazyReverse,
    Unbounded,
}

/// `_option_string_actions`, in the order `add_argument` fills it: an
/// ambiguous prefix lists its matches in this order.
const STRINGS: [(&str, Dest); 20] = [
    ("-h", Dest::Help),
    ("--help", Dest::Help),
    ("-o", Dest::Output),
    ("--output", Dest::Output),
    ("--index", Dest::Index),
    ("--kind", Dest::Kind),
    ("--build", Dest::Build),
    ("--allow-index-mismatch", Dest::Allow),
    ("--name", Dest::Name),
    ("--sample", Dest::Sample),
    ("--limit", Dest::Limit),
    ("--levels", Dest::Levels),
    ("-j", Dest::Jobs),
    ("--jobs", Dest::Jobs),
    ("--stats", Dest::Stats),
    ("-q", Dest::Quiet),
    ("--quiet", Dest::Quiet),
    ("--tpk", Dest::Tpk),
    ("--reverse-prepass", Dest::LazyReverse),
    ("--unbounded", Dest::Unbounded),
];

impl Dest {
    /// `nargs` None (one value) against `store_true` / help (none).
    fn takes_value(self) -> bool {
        !matches!(
            self,
            Self::Help | Self::Index | Self::Allow | Self::Quiet | Self::LazyReverse
        )
    }

    /// The action's name in an error: `-o/--output`.
    fn display(self) -> String {
        STRINGS
            .iter()
            .filter(|(_, d)| *d == self)
            .map(|(s, _)| *s)
            .collect::<Vec<_>>()
            .join("/")
    }
}

fn exact(s: &[u8]) -> Option<Dest> {
    STRINGS
        .iter()
        .find(|(o, _)| o.as_bytes() == s)
        .map(|(_, d)| *d)
}

/// `unityrs dump`'s arguments, as `unitypy_dump.py`'s `args` after `main`
/// has split and checked them.
#[derive(Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // argparse's store_true flags
pub struct DumpArgs {
    pub inputs: Vec<PathBuf>,
    pub output: PathBuf,
    pub index: bool,
    pub kind: Vec<String>,
    pub build: Option<Vec<String>>,
    pub allow_index_mismatch: bool,
    pub name: Vec<Vec<u8>>,
    pub sample: Option<i64>,
    pub limit: Option<i64>,
    pub levels: Levels,
    pub jobs: i64,
    pub stats: Option<PathBuf>,
    pub quiet: bool,
    pub tpk: Option<PathBuf>,
    /// `--reverse-prepass`, an extension: G1c's pre-pass.
    pub lazy_reverse: bool,
    /// `--unbounded`, an extension: the limits G3b's check lifts, in
    /// [`LIFTABLE`] order.
    pub unbounded: Vec<LimitKind>,
}

/// The command line's outcome.
#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    /// `-h`: print [`HELP`] and exit 0.
    Help,
    Dump(Box<DumpArgs>),
}

/// An argument error (`ap.error`): status 2 after [`USAGE`].
#[derive(Debug, PartialEq, Eq)]
pub struct Usage(pub String);

/// Python's `repr` of a surrogate-escaped `str` made from these bytes: the
/// quote it picks, backslash escapes for the quote, `\\`, `\t`, `\n`, `\r`
/// and other ASCII and C1 controls, and `\udcXX` for a byte that is not
/// UTF-8. (Python also escapes the rest of what `str.isprintable` refuses,
/// which no option value here holds.)
pub fn py_repr(bytes: &[u8]) -> String {
    let mut chars = Vec::new();
    for chunk in bytes.utf8_chunks() {
        chars.extend(chunk.valid().chars().map(Ok));
        chars.extend(chunk.invalid().iter().map(|b| Err(*b)));
    }
    let has = |q: char| chars.contains(&Ok(q));
    let quote = if has('\'') && !has('"') { '"' } else { '\'' };
    let mut s = String::from(quote);
    for c in chars {
        match c {
            Err(b) => {
                let _ = write!(s, "\\udc{b:02x}");
            }
            Ok('\\') => s.push_str("\\\\"),
            Ok('\t') => s.push_str("\\t"),
            Ok('\n') => s.push_str("\\n"),
            Ok('\r') => s.push_str("\\r"),
            Ok(c) if c == quote => {
                s.push('\\');
                s.push(c);
            }
            Ok(c) if (c as u32) < 0x20 || (0x7f..0xa0).contains(&(c as u32)) => {
                let _ = write!(s, "\\x{:02x}", c as u32);
            }
            Ok(c) => s.push(c),
        }
    }
    s.push(quote);
    s
}

/// Python's `int(s)` for a command-line number, saturated to `i64`: every
/// count it feeds (`--sample`, `--limit`, `--jobs`) means the same past any
/// list's length. Surrounding whitespace, a sign, and single underscores
/// between digits, as `int` takes them; ASCII digits only (Python also
/// takes other decimal digits, which no corpus run passes).
pub fn py_int(bytes: &[u8]) -> Option<i64> {
    let s = std::str::from_utf8(bytes).ok()?;
    let t = s.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c));
    let (neg, digits) = match t.as_bytes().first()? {
        b'-' => (true, &t[1..]),
        b'+' => (false, &t[1..]),
        _ => (false, t),
    };
    let b = digits.as_bytes();
    if !b.first()?.is_ascii_digit() || !b.last()?.is_ascii_digit() {
        return None;
    }
    let mut v: i64 = 0;
    let mut after_underscore = false;
    for &c in b {
        if c == b'_' {
            if after_underscore {
                return None;
            }
            after_underscore = true;
            continue;
        }
        if !c.is_ascii_digit() {
            return None;
        }
        after_underscore = false;
        v = v.saturating_mul(10).saturating_add(i64::from(c - b'0'));
    }
    Some(if neg { v.saturating_neg() } else { v })
}

/// argparse's `_negative_number_matcher`, `^-\d+$|^-\d*\.\d+$` (`$` also
/// before a final newline; ASCII digits): with no option of that shape,
/// such an argument is a value, never an option.
fn negative_number(arg: &[u8]) -> bool {
    let Some(rest) = arg.strip_prefix(b"-") else {
        return false;
    };
    let rest = rest.strip_suffix(b"\n").unwrap_or(rest);
    let digits = |s: &[u8]| s.iter().all(u8::is_ascii_digit);
    match rest.iter().position(|b| *b == b'.') {
        None => !rest.is_empty() && digits(rest),
        Some(p) => digits(&rest[..p]) && rest.len() > p + 1 && digits(&rest[p + 1..]),
    }
}

/// One argument as `_parse_optional` reads it.
#[derive(Clone, Debug)]
enum Class {
    /// A value or an input.
    Positional,
    /// An option string: its action (none for an unknown option, which
    /// goes to the extras), the string matched, whether `=` separated the
    /// value, and the value attached to it.
    Optional {
        dest: Option<Dest>,
        option: Vec<u8>,
        eq: bool,
        explicit: Option<Vec<u8>>,
    },
}

fn optional(dest: Dest, option: &[u8], eq: bool, explicit: Option<&[u8]>) -> Class {
    Class::Optional {
        dest: Some(dest),
        option: option.to_vec(),
        eq,
        explicit: explicit.map(<[u8]>::to_vec),
    }
}

/// `_get_option_tuples`.
fn option_tuples(arg: &[u8]) -> Vec<Class> {
    let mut out = Vec::new();
    if arg.get(1) == Some(&b'-') {
        // Long: a prefix, split at the first `=` only.
        let (prefix, explicit) = match arg.iter().position(|b| *b == b'=') {
            Some(p) => (&arg[..p], Some(&arg[p + 1..])),
            None => (arg, None),
        };
        for (o, d) in STRINGS {
            if o.as_bytes().starts_with(prefix) {
                out.push(optional(d, o.as_bytes(), explicit.is_some(), explicit));
            }
        }
    } else {
        // Short: the first two characters, the rest attached.
        let short = &arg[..2.min(arg.len())];
        for (o, d) in STRINGS {
            if o.as_bytes() == short {
                out.push(optional(d, o.as_bytes(), false, Some(&arg[short.len()..])));
            } else if o.as_bytes().starts_with(arg) {
                out.push(optional(d, o.as_bytes(), false, None));
            }
        }
    }
    out
}

/// `_parse_optional`.
fn classify(arg: &[u8]) -> Result<Class, Usage> {
    if arg.first() != Some(&b'-') {
        return Ok(Class::Positional);
    }
    if let Some(d) = exact(arg) {
        return Ok(optional(d, arg, false, None));
    }
    if arg.len() == 1 {
        return Ok(Class::Positional);
    }
    if let Some(p) = arg.iter().position(|b| *b == b'=')
        && let Some(d) = exact(&arg[..p])
    {
        return Ok(optional(d, &arg[..p], true, Some(&arg[p + 1..])));
    }
    let mut tuples = option_tuples(arg);
    if tuples.len() > 1 {
        let names: Vec<String> = tuples
            .iter()
            .filter_map(|t| match t {
                Class::Optional { option, .. } => {
                    Some(String::from_utf8_lossy(option).into_owned())
                }
                Class::Positional => None,
            })
            .collect();
        return Err(Usage(format!(
            "ambiguous option: {} could match {}",
            String::from_utf8_lossy(arg),
            names.join(", ")
        )));
    }
    if let Some(t) = tuples.pop() {
        return Ok(t);
    }
    if negative_number(arg) || arg.contains(&b' ') {
        return Ok(Class::Positional);
    }
    Ok(Class::Optional {
        dest: None,
        option: arg.to_vec(),
        eq: false,
        explicit: None,
    })
}

/// What the options stored, as argparse's namespace holds it.
#[derive(Default)]
struct Namespace {
    inputs: Vec<Vec<u8>>,
    extras: Vec<Vec<u8>>,
    values: Vec<(Dest, Option<Vec<u8>>)>,
    ints: Vec<(Dest, i64)>,
}

impl Namespace {
    /// `take_action`: stores one option. `int` options convert here, in
    /// order, as argparse's `_get_values` does.
    fn take(&mut self, dest: Dest, value: Option<Vec<u8>>) -> Result<bool, Usage> {
        if dest == Dest::Help {
            return Ok(true);
        }
        if matches!(dest, Dest::Sample | Dest::Limit | Dest::Jobs) {
            let v = value.unwrap_or_default();
            let n = py_int(&v).ok_or_else(|| {
                Usage(format!(
                    "argument {}: invalid int value: {}",
                    dest.display(),
                    py_repr(&v)
                ))
            })?;
            self.ints.push((dest, n));
            return Ok(false);
        }
        self.values.push((dest, value));
        Ok(false)
    }

    fn last(&self, dest: Dest) -> Option<&[u8]> {
        self.values
            .iter()
            .rev()
            .find(|(d, _)| *d == dest)
            .and_then(|(_, v)| v.as_deref())
    }

    fn int(&self, dest: Dest) -> Option<i64> {
        self.ints
            .iter()
            .rev()
            .find(|(d, _)| *d == dest)
            .map(|(_, n)| *n)
    }

    fn flag(&self, dest: Dest) -> bool {
        self.values.iter().any(|(d, _)| *d == dest)
    }
}

/// `consume_optional` for the option at `args[i]`: returns the index after
/// it and its value, or that `-h` was taken.
fn consume_optional(
    args: &[Vec<u8>],
    classes: &[Class],
    i: usize,
    ns: &mut Namespace,
) -> Result<Option<usize>, Usage> {
    let Class::Optional {
        mut dest,
        mut option,
        mut eq,
        mut explicit,
    } = classes[i].clone()
    else {
        unreachable!("consume_optional on a positional");
    };
    let ignored = |d: Dest, ex: &[u8]| {
        Usage(format!(
            "argument {}: ignored explicit argument {}",
            d.display(),
            py_repr(ex)
        ))
    };
    let mut taken: Vec<(Dest, Option<Vec<u8>>)> = Vec::new();
    let mut stop = i + 1;
    loop {
        let Some(d) = dest else {
            ns.extras.push(args[i].clone());
            return Ok(Some(i + 1));
        };
        if let Some(ex) = explicit.take() {
            if !d.takes_value() && option.get(1) != Some(&b'-') && !ex.is_empty() {
                // `-qj4`: more single-dash options in the tail.
                if eq || ex[0] == b'-' {
                    return Err(ignored(d, &ex));
                }
                taken.push((d, None));
                option = vec![b'-', ex[0]];
                if let Some(next) = exact(&option) {
                    dest = Some(next);
                    let rest = &ex[1..];
                    (eq, explicit) = match rest.first() {
                        None => (false, None),
                        Some(b'=') => (true, Some(rest[1..].to_vec())),
                        Some(_) => (false, Some(rest.to_vec())),
                    };
                } else {
                    let mut extra = vec![b'-'];
                    extra.extend_from_slice(&ex);
                    ns.extras.push(extra);
                    break;
                }
            } else if d.takes_value() {
                taken.push((d, Some(ex)));
                break;
            } else {
                return Err(ignored(d, &ex));
            }
        } else {
            if d.takes_value() {
                // The next argument, if it is a value (`A`; `--` is not).
                match (args.get(i + 1), classes.get(i + 1)) {
                    (Some(v), Some(Class::Positional)) if v != b"--" => {
                        taken.push((d, Some(v.clone())));
                        stop = i + 2;
                    }
                    _ => {
                        return Err(Usage(format!(
                            "argument {}: expected one argument",
                            d.display()
                        )));
                    }
                }
            } else {
                taken.push((d, None));
            }
            break;
        }
    }
    for (d, v) in taken {
        if ns.take(d, v)? {
            return Ok(None);
        }
    }
    Ok(Some(stop))
}

fn utf8(dest: Dest, v: &[u8]) -> Result<String, Usage> {
    String::from_utf8(v.to_vec()).map_err(|_| {
        Usage(format!(
            "argument {}: {} is not UTF-8, so it names no index entry",
            dest.display(),
            py_repr(v)
        ))
    })
}

fn path(v: &[u8]) -> PathBuf {
    PathBuf::from(OsString::from_vec(v.to_vec()))
}

/// Parses `unityrs dump`'s arguments (those after `dump`), then checks them
/// as `unitypy_dump.py`'s `main` does before it touches a file.
pub fn parse(args: &[Vec<u8>]) -> Result<Parsed, Usage> {
    // `--` ends the options; the arguments before it are classified first.
    let dd = args.iter().position(|a| a == b"--").unwrap_or(args.len());
    let classes = args[..dd]
        .iter()
        .map(|a| classify(a))
        .collect::<Result<Vec<_>, _>>()?;
    let mut ns = Namespace::default();
    let mut i = 0;
    while i < dd {
        if matches!(classes[i], Class::Positional) {
            ns.inputs.push(args[i].clone());
            i += 1;
            continue;
        }
        match consume_optional(&args[..dd], &classes, i, &mut ns)? {
            Some(next) => i = next,
            None => return Ok(Parsed::Help),
        }
    }
    ns.inputs.extend(args.iter().skip(dd + 1).cloned());

    let mut missing = Vec::new();
    if ns.inputs.is_empty() {
        missing.push("input");
    }
    let output = ns.last(Dest::Output).map(path);
    if output.is_none() {
        missing.push("-o/--output");
    }
    if !missing.is_empty() {
        return Err(Usage(format!(
            "the following arguments are required: {}",
            missing.join(", ")
        )));
    }
    if !ns.extras.is_empty() {
        let extras: Vec<String> = ns
            .extras
            .iter()
            .map(|e| String::from_utf8_lossy(e).into_owned())
            .collect();
        return Err(Usage(format!(
            "unrecognized arguments: {}",
            extras.join(" ")
        )));
    }

    let index = ns.flag(Dest::Index);
    let allow_index_mismatch = ns.flag(Dest::Allow);
    if allow_index_mismatch && !index {
        return Err(Usage("--allow-index-mismatch needs --index".into()));
    }
    if index && ns.inputs.len() > 1 {
        return Err(Usage(
            "--index takes one INPUT, the corpus index.json".into(),
        ));
    }
    let kind = match ns.last(Dest::Kind) {
        Some(k) => utf8(Dest::Kind, k)?,
        None => INDEX_KINDS.join(","),
    };
    // `args.build.split(",") if args.build else None`: an empty one is none.
    let build = match ns.last(Dest::Build) {
        Some(b) if !b.is_empty() => Some(utf8(Dest::Build, b)?),
        _ => None,
    };
    let levels = match ns.last(Dest::Levels) {
        Some(l) => String::from_utf8_lossy(l).into_owned(),
        None => "L0,L1,L2,L3".to_owned(),
    };
    let levels = Levels::parse(&levels).map_err(Usage)?;
    let unbounded = match ns.last(Dest::Unbounded) {
        Some(u) => unbounded(u)?,
        None => Vec::new(),
    };
    Ok(Parsed::Dump(Box::new(DumpArgs {
        inputs: ns.inputs.iter().map(|a| path(a)).collect(),
        output: output.unwrap_or_default(),
        index,
        kind: kind.split(',').map(str::to_owned).collect(),
        build: build.map(|b| b.split(',').map(str::to_owned).collect()),
        allow_index_mismatch,
        name: ns
            .values
            .iter()
            .filter(|(d, _)| *d == Dest::Name)
            .filter_map(|(_, v)| v.clone())
            .collect(),
        sample: ns.int(Dest::Sample),
        limit: ns.int(Dest::Limit),
        levels,
        jobs: ns.int(Dest::Jobs).unwrap_or(1),
        stats: ns.last(Dest::Stats).map(path),
        quiet: ns.flag(Dest::Quiet),
        tpk: ns.last(Dest::Tpk).map(path),
        lazy_reverse: ns.flag(Dest::LazyReverse),
        unbounded,
    })))
}

/// `--unbounded`'s comma list: each a [`LIFTABLE`] kind's name, any order,
/// repeats allowed; returned in [`LIFTABLE`] order.
fn unbounded(v: &[u8]) -> Result<Vec<LimitKind>, Usage> {
    let names: Vec<String> = LIFTABLE.iter().map(|k| format!("{k:?}")).collect();
    let mut picked = Vec::new();
    for name in v.split(|b| *b == b',') {
        let Some(i) = names.iter().position(|n| n.as_bytes() == name) else {
            return Err(Usage(format!(
                "argument --unbounded: invalid limit: {} (choose from {})",
                py_repr(name),
                names.join(", ")
            )));
        };
        picked.push(LIFTABLE[i]);
    }
    Ok(LIFTABLE
        .into_iter()
        .filter(|k| picked.contains(k))
        .collect())
}

#[cfg(test)]
mod tests;
