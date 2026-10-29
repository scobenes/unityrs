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
//! Owner: package E.

// STUB(E): delete this allow when the dump is implemented.
#![allow(
    clippy::todo,
    clippy::needless_pass_by_value,
    clippy::must_use_candidate,
    clippy::missing_panics_doc,
    unused_variables
)]

use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use unityrs_core::FileContext;

/// The dump's public types are `Send + Sync`, as the core's are (plan
/// section 3.16): `-j N` shares the options across rayon workers.
const _: () = {
    const fn send_sync<T: Send + Sync>() {}
    send_sync::<Levels>();
    send_sync::<DumpOptions>();
    send_sync::<InputDump>();
};

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
    /// The value-case harness's typetree source, when one is given.
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

/// One input's records, serialised, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InputDump {
    /// JSON lines, each without its newline.
    pub lines: Vec<String>,
    /// `file` records among them.
    pub files: u64,
    /// `object` records among them.
    pub objects: u64,
}

/// The `dump` header line.
pub fn header_line(levels: Levels, producer: &str) -> String {
    todo!()
}

/// The `end` line.
pub fn end_line(files: u64, objects: u64) -> String {
    todo!()
}

/// One input's records. `rel` is its path relative to the corpus root, as
/// bytes, which is how the dump names it.
pub fn dump_input(rel: &[u8], data: unityrs_core::Bytes, opts: &DumpOptions) -> InputDump {
    todo!()
}

/// Dumps every file under `root`, in bytewise path order, to `out` (gzip,
/// level 9, mtime 0, as the oracle writes).
pub fn dump_tree(
    root: &Path,
    opts: &DumpOptions,
    out: &mut dyn Write,
) -> std::io::Result<(u64, u64)> {
    todo!()
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
}
