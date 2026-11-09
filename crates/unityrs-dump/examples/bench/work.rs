//! One input's work: read it, load it (eagerly, as UnityPy's `load_file`
//! does, or lazily, every directory row in order), read every object's
//! value, and take its counts.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use unityrs_core::budget::Reservation;
use unityrs_core::detect::check_entry_type;
use unityrs_core::env::read_path;
use unityrs_core::files::{BundleCounts, parse_file};
use unityrs_core::limits::Meter;
use unityrs_core::version::{ResolvedVersion, VersionError};
use unityrs_core::{Error, File, FileContext, Parsed, PrimArray, RawStr, SerializedFile, Value};
use unityrs_dump::input::load_deep;

use crate::clock::Clock;

/// How an input is loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Load {
    /// `load_deep`: every block decoded, every entry parsed (UnityPy's
    /// `Environment.load_file`).
    Eager,
    /// Opened lazily, then every directory row's entry read in directory
    /// order, a nested bundle's rows as soon as it is parsed (plan section
    /// 8, R2: "read in directory order").
    Lazy,
}

/// The bundles' counts of an input, summed over its bundles (nested ones
/// included), with the largest bundle's run peak.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sums {
    pub bundles: u64,
    /// Data blocks, and the most in one bundle.
    pub blocks: u64,
    pub blocks_max: u64,
    pub decoded: u64,
    pub discarded: u64,
    pub redecoded: u64,
    pub runs_peak_max: u64,
    pub runs_peak_sum: u64,
    /// The bundles' declared decoded sizes (`uncompressed_size`).
    pub declared: u64,
}

impl Sums {
    pub fn add(&mut self, c: &BundleCounts, declared: u64, blocks: u64) {
        self.bundles += 1;
        self.blocks += blocks;
        self.blocks_max = self.blocks_max.max(blocks);
        self.decoded += c.decoded;
        self.discarded += c.discarded;
        self.redecoded += c.redecoded;
        self.runs_peak_max = self.runs_peak_max.max(c.runs_peak);
        self.runs_peak_sum += c.runs_peak;
        self.declared += declared;
    }

    pub fn absorb(&mut self, o: &Self) {
        self.bundles += o.bundles;
        self.blocks += o.blocks;
        self.blocks_max = self.blocks_max.max(o.blocks_max);
        self.decoded += o.decoded;
        self.discarded += o.discarded;
        self.redecoded += o.redecoded;
        self.runs_peak_max = self.runs_peak_max.max(o.runs_peak_max);
        self.runs_peak_sum += o.runs_peak_sum;
        self.declared += o.declared;
    }
}

/// What one value kept past its input's handles holds (plan section 8,
/// R3): the input's live charges once the handles are dropped, against the
/// values' own charges.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pin {
    /// Live once the handles drop, every value kept, beyond the values' own
    /// charges and their node lists'.
    pub all: u64,
    /// The same with one value kept: the first in object order that holds
    /// a slice of the file (a string, `TypelessData` or a `vector<UInt8>`).
    pub one: u64,
    /// That value's own charge and its node lists'.
    pub one_own: u64,
    /// Whether that value holds a slice (no value did, when false).
    pub one_slices: bool,
}

/// One input's results.
#[derive(Clone, Debug, Default)]
pub struct Done {
    pub bytes: u64,
    pub objects: u64,
    pub value_errors: u64,
    pub file_error: bool,
    pub load_ns: u64,
    pub values_ns: u64,
    pub sums: Sums,
    /// What the input decodes to: its bundles' declared sizes, or its own
    /// size when it is no bundle.
    pub decoded_size: u64,
    /// `(chain, source)` → count: each bundle's engine version (`bundle`)
    /// and each serialized file's (`serialized`), by `VersionSource`, or
    /// `error`/`none`; `big` counts versions whose packing passes `u64`.
    pub versions: BTreeMap<(&'static str, String), u64>,
    /// Charges still live after every handle and value dropped.
    pub leaked: u64,
    pub pin: Option<Pin>,
}

/// Reads, loads and walks the input at `path` on `ctx` (its budget books
/// the input's meter), keeping values for [`Pin`] when `pin`.
pub fn work(path: &Path, ctx: &Arc<FileContext>, load: Load, pin: bool, clock: Clock) -> Done {
    let mut d = Done::default();
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let t0 = clock.now();
    let file = open(path, ctx, &meter, load, &mut d);
    let t1 = clock.now();
    d.load_ns = t1.saturating_sub(t0);
    let mut kept = Vec::new();
    if let Some(file) = file {
        let mut sfs = Vec::new();
        tree(&file, &mut |f| {
            if let File::Serialized(sf) = f {
                sfs.push(sf.clone());
            }
        });
        for sf in &sfs {
            for obj in sf.objects() {
                d.objects += 1;
                match obj.parse_as_dict(true) {
                    Ok(v) if pin => kept.push(v),
                    Ok(_) => {}
                    Err(_) => d.value_errors += 1,
                }
            }
        }
        d.values_ns = clock.now().saturating_sub(t1);
        // Counted after the timed region; reading them decodes nothing.
        tree(&file, &mut |f| match f {
            File::Bundle(b) => {
                let blocks = u64::try_from(b.blocks().len()).unwrap_or(u64::MAX);
                d.sums.add(&b.counts(), b.uncompressed_size(), blocks);
                version(&mut d, "bundle", b.header().engine_version.as_ref());
            }
            File::Serialized(sf) => serialized_version(&mut d, sf),
            _ => {}
        });
        d.decoded_size = if matches!(file, File::Bundle(_)) {
            d.sums.declared
        } else {
            d.bytes
        };
        drop(sfs);
        drop(file);
        if pin {
            d.pin = Some(pinned(&meter, kept));
        }
    }
    d.leaked = meter.used();
    d
}

fn open(
    path: &Path,
    ctx: &Arc<FileContext>,
    meter: &Arc<Meter>,
    load: Load,
    d: &mut Done,
) -> Option<File> {
    let Ok(data) = read_path(path, &ctx.limits, meter) else {
        d.file_error = true;
        return None;
    };
    d.bytes = u64::try_from(data.len()).unwrap_or(u64::MAX);
    let name = RawStr::from(path.as_os_str().as_encoded_bytes());
    let loaded = check_entry_type(&data, &ctx.limits, meter)
        .map_err(Error::Detect)
        .and_then(|ty| parse_file(name, data, ty, None, meter, ctx))
        .and_then(|file| match load {
            Load::Eager => load_deep(&file).map(|()| file),
            Load::Lazy => {
                d.file_error |= !rows_in_order(&file);
                Ok(file)
            }
        });
    let file = loaded.ok();
    d.file_error |= file.is_none();
    file
}

/// Every directory row's entry, in directory order, a nested bundle's rows
/// as soon as it is parsed: whether every one parsed.
fn rows_in_order(file: &File) -> bool {
    let mut ok = true;
    if let File::Bundle(b) = file {
        for i in 0..b.directory().len() {
            match b.entry(i) {
                Ok(entry) => ok &= rows_in_order(&entry),
                Err(_) => ok = false,
            }
        }
    }
    ok
}

/// `file` and every file under it, as UnityPy's `files` dicts hold them
/// (a bundle's `files` slots, in order).
fn tree(file: &File, visit: &mut impl FnMut(&File)) {
    visit(file);
    if let File::Bundle(b) = file {
        for slot in b.files() {
            if let Ok(entry) = b.entry(slot.dir_index) {
                tree(&entry, visit);
            }
        }
    }
}

fn version(d: &mut Done, chain: &'static str, v: Option<&Result<ResolvedVersion, VersionError>>) {
    let source = match v {
        None => "none".to_owned(),
        Some(Err(_)) => "error".to_owned(),
        Some(Ok(r)) => {
            if r.version.packed_u64().is_none() {
                *d.versions.entry((chain, "big".to_owned())).or_default() += 1;
            }
            format!("{:?}", r.source)
        }
    };
    *d.versions.entry((chain, source)).or_default() += 1;
}

fn serialized_version(d: &mut Done, sf: &SerializedFile) {
    match sf.version() {
        Some(v) => version(d, "serialized", Some(&Ok(v.clone()))),
        None => version(d, "serialized", None),
    }
}

/// A value's own charge and its node lists' (each list counted once in
/// `seen`).
fn own(p: &Parsed, seen: &mut HashSet<usize>) -> u64 {
    let lists: u64 = p
        .lists()
        .iter()
        .filter(|l| seen.insert(Arc::as_ptr(l) as usize))
        .map(|l| l.charge().bytes())
        .sum();
    p.charge().bytes() + lists
}

/// Whether `v` holds a slice of the file: a non-empty string,
/// `TypelessData` or `vector<UInt8>`.
fn slices(v: &Value) -> bool {
    match v {
        Value::Str(s) => !s.as_bytes().is_empty(),
        Value::Bytes(b) | Value::Prims(PrimArray::U8(b)) => !b.is_empty(),
        Value::List(items) => items.iter().any(slices),
        Value::Tuple(items) => items.iter().any(slices),
        Value::Dict(m) => m.entries().iter().any(|(_, v)| slices(v)),
        _ => false,
    }
}

/// [`Pin`]: the handles are already dropped; `values` are every value read.
fn pinned(meter: &Meter, mut values: Vec<Parsed>) -> Pin {
    let mut seen = HashSet::new();
    let own_all: u64 = values.iter().map(|p| own(p, &mut seen)).sum();
    let all = meter.used().saturating_sub(own_all);
    let pick = values.iter().position(|p| slices(p.value()));
    let one = pick.map(|i| values.swap_remove(i)).or_else(|| values.pop());
    drop(values);
    let Some(one) = one else {
        return Pin {
            all,
            ..Pin::default()
        };
    };
    let one_own = own(&one, &mut HashSet::new());
    Pin {
        all,
        one: meter.used().saturating_sub(one_own),
        one_own,
        one_slices: pick.is_some(),
    }
}
