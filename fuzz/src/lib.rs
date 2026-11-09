//! What the fuzz targets share: the limits every run is bounded by, the
//! context and meter of one input, a thread with the stack a read needs,
//! and a [`TreeCase`]'s tree and ref types as the core's.
//!
//! Every target uses the core's public API, as a caller would: no target
//! needs an item the crate does not export.

#[cfg(test)]
mod tests;

use std::sync::Arc;

use unityrs_core::budget::Reservation;
use unityrs_core::files::SerializedType;
use unityrs_core::limits::{Limits, Meter, MeteredVec};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeId, NodeList, NodeRow, Tree};
use unityrs_core::walk::{FloatMode, ReadOptions, RefTypes};
use unityrs_core::{FileContext, RawStr};
use unityrs_testkit::fuzz::{Row, TreeCase};

/// The bound of an input's memory (and of its size and of one block's
/// output): 128 MiB. Every seed is read through to its deep paths rather
/// than stopped at it, and it is far below libFuzzer's limits
/// (`-malloc_limit_mb=2048`, the plan's 2 GiB, and `-rss_limit_mb`), so an
/// input that reaches it ends with the core's named limit error, and an
/// allocation the meter did not see is what reaches libFuzzer's.
///
/// A file target can reach it (`serialized` does, by its stream reader's
/// rules). A value read cannot: within the work bound ([`WORK`]) it holds
/// under 20 MB, about twice the work it has done. So a value case picks its
/// memory bound ([`Case::new`]), this one or [`SMALL_MEMORY`].
pub const MEMORY: u64 = 128 << 20;

/// The memory bound of a value case with `TreeCase::SMALL_MEMORY`: 8 MiB.
/// A read of many small values reaches it at 4 to 6 MiB of work, so the
/// code that grows a charged buffer at the bound runs: a doubling the meter
/// refuses, retried with half the extra room until it fits (fix pass 17).
///
/// The first runs found the defect fix pass 17 fixed there: a refused
/// doubling asked for exactly what one push needed, so near the bound
/// every push reallocated, and AddressSanitizer's `realloc` copies the
/// whole buffer each time. It showed at memory bounds of 8 and 128 MiB
/// (with 64 MiB of work), never at 1 MiB, where the copies stay short. Put
/// back into the core, it times out two of the six inputs those runs kept
/// (one of `python_read`'s, one of `write_after_read`'s) at this bound, and
/// none at [`MEMORY`].
pub const SMALL_MEMORY: u64 = 8 << 20;

/// The bound of an input's work (every byte decoded, every value read or
/// written, every byte detection inflates): 8 MiB, so no input runs near the
/// 10 s timeout by doing work the limits allow. A target runs a read at
/// about 6 to 7 MiB of work a second at its slowest, under
/// AddressSanitizer, libFuzzer's coverage and debug assertions: a list of
/// small values reading to the bound took 4 to 9 s at 64 MiB and 0.6 to
/// 1.4 s at 8 MiB. A CI runner may be slower.
pub const WORK: u64 = 8 << 20;

/// The limits of every run: the defaults, with the memory, size and output
/// bounds at [`MEMORY`] and the work bounds at [`WORK`].
#[must_use]
pub fn limits() -> Limits {
    let mut l = Limits::default();
    l.max_input_memory = MEMORY;
    l.max_input_bytes = MEMORY;
    l.max_block_output = MEMORY;
    l.max_decode_work = WORK;
    l.max_inflate = WORK;
    l
}

/// UnityPy's context, under [`limits`].
#[must_use]
pub fn context() -> Arc<FileContext> {
    context_with(limits())
}

/// UnityPy's context, under `limits`.
fn context_with(limits: Limits) -> Arc<FileContext> {
    let mut c = FileContext::unitypy();
    c.limits = limits;
    Arc::new(c)
}

/// A new input's meter under `ctx`.
#[must_use]
pub fn meter(ctx: &FileContext) -> Arc<Meter> {
    Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input)
}

/// Runs `f` on a thread with the stack a read or a write needs under
/// [`limits`] (`Limits::stack_needed`), four times over for the
/// sanitizer's larger frames. A panic there aborts the process, as
/// libFuzzer's hook makes every panic do.
pub fn with_stack<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    let stack = limits().stack_needed().saturating_mul(4);
    std::thread::scope(|s| {
        let worker = std::thread::Builder::new()
            .stack_size(stack)
            .spawn_scoped(s, f);
        match worker.map(std::thread::ScopedJoinHandle::join) {
            Ok(Ok(r)) => r,
            // A panic aborted before this. A thread the system refused
            // stops the run too, saying so: the harness's failure, not
            // the core's.
            Ok(Err(p)) => std::panic::resume_unwind(p),
            Err(e) => panic!("the harness got no thread for the read: {e}"),
        }
    })
}

/// A case's rows as a node list on `meter`, or `None` when the meter
/// refuses them.
#[must_use]
pub fn node_list(rows: &[Row], meter: &Arc<Meter>) -> Option<Arc<NodeList>> {
    let mut v: MeteredVec<NodeRow> = MeteredVec::new(meter);
    for r in rows {
        let row = NodeRow::new(
            i32::from(r.level),
            RawStr::from(r.ty.clone()),
            RawStr::from(r.name.clone()),
        )
        .with_meta_flag(r.meta_flag);
        v.push(row).ok()?;
    }
    NodeList::from_rows(v).ok().map(Arc::new)
}

/// What a [`TreeCase`] describes, as the core's values.
pub struct Case {
    /// The value's tree, compiled.
    pub tree: Arc<Tree>,
    /// The ref types; [`Case::ref_types`] says how the reader sees them.
    pub refs: Vec<SerializedType>,
    /// The value's bytes.
    pub data: unityrs_core::Bytes,
    /// The case, as decoded.
    pub case: TreeCase,
    /// The reader's options, whose limits are the case's.
    pub options: ReadOptions,
    /// The input's meter, under the case's limits: the tree and the ref
    /// types are charged to it, and the read is.
    pub meter: Arc<Meter>,
    ctx: Arc<FileContext>,
}

impl Case {
    /// The case `input` holds, under [`limits`] with the memory bound at
    /// [`SMALL_MEMORY`] when it has `TreeCase::SMALL_MEMORY`, its tree
    /// compiled on a new meter of those limits; `None` when the tree does not
    /// compile (the `typetree` target's concern).
    #[must_use]
    pub fn new(input: &[u8]) -> Option<Self> {
        let case = TreeCase::decode(input);
        let mut limits = limits();
        if case.has(TreeCase::SMALL_MEMORY) {
            limits.max_input_memory = SMALL_MEMORY;
        }
        let ctx = context_with(limits);
        let meter = meter(&ctx);
        let tree = Tree::compile(node_list(&case.rows, &meter)?, &limits).ok()?;
        let refs = case
            .refs
            .iter()
            .map(|r| {
                SerializedType::ref_type(
                    114,
                    RawStr::from(r.class.clone()),
                    RawStr::from(r.namespace.clone()),
                    RawStr::from(r.assembly.clone()),
                    r.rows.as_ref().and_then(|rows| node_list(rows, &meter)),
                )
            })
            .collect();
        let options = ReadOptions {
            floats: if case.has(TreeCase::LOSSLESS) {
                FloatMode::Lossless
            } else {
                FloatMode::UnityPy
            },
            limits,
        };
        Some(Self {
            tree,
            refs,
            data: unityrs_core::Bytes::from(case.data.clone()),
            case,
            options,
            meter,
            ctx,
        })
    }

    /// A new meter under the case's limits, for a read that must not use
    /// what an earlier one used.
    #[must_use]
    pub fn new_meter(&self) -> Arc<Meter> {
        meter(&self.ctx)
    }

    /// The data's byte order.
    #[must_use]
    pub fn endian(&self) -> Endian {
        if self.case.has(TreeCase::BIG_ENDIAN) {
            Endian::Big
        } else {
            Endian::Little
        }
    }

    /// The ref types as the reader sees them.
    #[must_use]
    pub fn ref_types(&self) -> RefTypes<'_> {
        if self.case.has(TreeCase::REFS_NO_FILE) {
            RefTypes::NoFile
        } else if self.case.has(TreeCase::REFS_ABSENT) {
            RefTypes::Absent
        } else {
            RefTypes::List(&self.refs)
        }
    }

    /// Whether no two children of a node share a name, in the tree and in
    /// each ref type's: a repeated name keeps its last value only, so no
    /// writer gives back the others' bytes (`WriteRules::MirrorReader`).
    #[must_use]
    pub fn names_unique(&self) -> bool {
        let unique = |t: &Tree| {
            (0..t.list().len()).all(|i| {
                let id = NodeId(u32::try_from(i).unwrap_or(u32::MAX));
                t.info(id).is_none_or(|x| x.key == id)
            })
        };
        unique(&self.tree)
            && self
                .refs
                .iter()
                .all(|r| match r.tree(&self.options.limits) {
                    Some(Ok(t)) => unique(&t),
                    _ => true,
                })
    }
}
