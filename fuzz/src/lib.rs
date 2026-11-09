//! What the fuzz targets share: the limits every run is bounded by, the
//! context and meter of one input, a thread with the stack a read needs,
//! and a [`TreeCase`]'s tree and ref types as the core's.
//!
//! Every target uses the core's public API, as a caller would: no target
//! needs an item the crate does not export.

use std::sync::Arc;

use unityrs_core::budget::Reservation;
use unityrs_core::files::SerializedType;
use unityrs_core::limits::{Limits, Meter, MeteredVec};
use unityrs_core::stream::Endian;
use unityrs_core::typetree::{NodeId, NodeList, NodeRow, Tree};
use unityrs_core::walk::{FloatMode, ReadOptions, RefTypes};
use unityrs_core::{FileContext, RawStr};
use unityrs_testkit::fuzz::{Row, TreeCase};

/// The bound of an input's memory and of its work: far below libFuzzer's
/// allocation limit (2 GiB in CI), so an input that reaches a bound ends
/// with the core's named limit error, and an allocation the meter did not
/// see is what reaches libFuzzer's. Work (decoding, reading, writing) is
/// bounded with it, so no input runs near the 10 s timeout by doing work
/// the limits allow.
pub const BOUND: u64 = 128 << 20;

/// The limits of every run: the defaults, with every memory, size and work
/// bound at [`BOUND`].
#[must_use]
pub fn limits() -> Limits {
    let mut l = Limits::default();
    l.max_input_memory = BOUND;
    l.max_input_bytes = BOUND;
    l.max_block_output = BOUND;
    l.max_decode_work = BOUND;
    l.max_inflate = BOUND;
    l
}

/// UnityPy's context, under [`limits`].
#[must_use]
pub fn context() -> Arc<FileContext> {
    let mut c = FileContext::unitypy();
    c.limits = limits();
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
            // A panic aborted before this; a thread the system refused is
            // no finding of the core's.
            Ok(Err(p)) => std::panic::resume_unwind(p),
            Err(e) => panic!("no thread for the read: {e}"),
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
    /// The reader's options.
    pub options: ReadOptions,
}

impl Case {
    /// The case `input` holds, its tree compiled on `meter`; `None` when the
    /// tree does not compile (the `typetree` target's concern).
    #[must_use]
    pub fn new(input: &[u8], meter: &Arc<Meter>) -> Option<Self> {
        let case = TreeCase::decode(input);
        let limits = limits();
        let tree = Tree::compile(node_list(&case.rows, meter)?, &limits).ok()?;
        let refs = case
            .refs
            .iter()
            .map(|r| {
                SerializedType::ref_type(
                    114,
                    RawStr::from(r.class.clone()),
                    RawStr::from(r.namespace.clone()),
                    RawStr::from(r.assembly.clone()),
                    r.rows.as_ref().and_then(|rows| node_list(rows, meter)),
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
        })
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
