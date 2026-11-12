//! `docs/plans/batch.md` 5.1's memory bound, as `unityrs batch --check`
//! prints it (`docs/BATCH_SPEC.md` section 10's `memory_bound <bytes>`):
//!
//! ```text
//! heap <= I + C_thr + C_env + C_net + C_spec + C_parse + C_hdr + C_fin + C_base
//! ```
//!
//! each term as 5.3 states it, in the charge unit (`heap_cost`).
//!
//! **Provisional.** The constants of [`PROVISIONAL`] (`c_base`, `m_thread`,
//! `m_pool`, `m_dispatch`, `m_end`, `c_env`) are estimates until
//! `docs/plans/extract.md` X12 measures them and the binary compiles the
//! measured figures in; `c_net` is A1's stated bound. So the bound is
//! provisional too: a caller sizes by it only as an estimate until then.

use std::collections::HashSet;

use unityrs_core::limits::{Limits, heap_cost};

use crate::alloc::Measured;
use crate::spec::fields::MAX_HEADER_VALUE;
use crate::spec::model::{HeaderValue, Sink, Spec};

const KIB: u64 = 1 << 10;
const MIB: u64 = 1 << 20;

/// The constants 5.3 compiles into the binary, and the sizes it takes as
/// given.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Constants {
    /// `c_base`: the heap no other term holds (X12 measures it).
    pub c_base: u64,
    /// `m_thread`: the heap one more worker adds.
    pub m_thread: u64,
    /// `m_pool`: the heap one more blocking-pool thread adds.
    pub m_pool: u64,
    /// `m_dispatch`: the largest dispatch message, its heap included.
    pub m_dispatch: u64,
    /// `m_end`: one end-channel slot, its 1 KiB path field included.
    pub m_end: u64,
    /// `c_env`: a private Environment's fixed parts.
    pub c_env: u64,
    /// `c_net`: one open connection's state (A1: at most 4 MiB).
    pub c_net: u64,
}

/// Today's constants: **provisional** estimates (module docs). `c_env`
/// and `c_base` are those 5.6's example comes to (9 MiB of Environments
/// at 8 threads and `max_view_bundles` 1,024; about 20 MiB of base).
pub const PROVISIONAL: Constants = Constants {
    c_base: 18 * MIB,
    m_thread: 128 * KIB,
    m_pool: 64 * KIB,
    m_dispatch: 2 * KIB,
    m_end: 2 * KIB,
    c_env: 128 * KIB,
    c_net: 4 * MIB,
};

/// The main thread's environment block as `--check` counts it: 6 MiB,
/// Linux's cap on `execve`'s arguments and environment (three quarters of
/// `_STK_LIM`, `fs/exec.c`), since `--check` reads no environment
/// (`docs/BATCH_SPEC.md` section 10).
pub const CHECK_ENV_BLOCK: u64 = 6 * MIB;

/// `C_fin` (5.3).
pub const C_FIN: u64 = 24 * MIB;

/// A worker's detection scratch, inflater state and window, and tally
/// (5.3's 0.11 MiB, rounded up).
pub const WORKER_STATE: u64 = 115_344;

/// A worker's panic slot (5.3's 2 KiB, `docs/plans/batch.md` 2.11's
/// *Panics*): the one place the bound counts it.
pub const PANIC_SLOT: u64 = 2 * KIB;

/// The stack of each named thread (main, controller, lifecycle, progress,
/// finisher) and of each blocking-pool thread.
pub const THREAD_STACK: u64 = 2 * MIB;

/// The stack each level of container nesting adds to a worker's
/// (`unityrs-dump`'s `run::CONTAINER_LEVEL_STACK`, which the CLI's tests
/// hold this to).
pub const CONTAINER_LEVEL_STACK: u64 = if cfg!(debug_assertions) {
    12 * KIB
} else {
    3 * KIB
};

/// The controller's per-bundle state and the id permutation (5.3's
/// `C_spec`, per declared bundle).
pub const PER_BUNDLE: u64 = 256;

/// The credentials at their caps (`docs/BATCH_SPEC.md` 1.6): the access
/// key id and the secret at 128 bytes, the session token at 16 KiB.
const CREDENTIALS: [u64; 3] = [128, 128, 16 * KIB];

/// One map entry of the header variables' name-to-value table, and of the
/// masker's list, beside the strings they point at.
const TABLE_ENTRY: u64 = 48;
const MASK_ENTRY: u64 = 32;

/// The bound's terms (5.1), in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[allow(clippy::struct_field_names)]
pub struct Bound {
    /// `I`.
    pub i: u64,
    /// `C_thr`.
    pub c_thr: u64,
    /// `C_env`.
    pub c_env: u64,
    /// `C_net`.
    pub c_net: u64,
    /// `C_spec`.
    pub c_spec: u64,
    /// `C_parse`.
    pub c_parse: u64,
    /// `C_hdr`.
    pub c_hdr: u64,
    /// `C_fin`.
    pub c_fin: u64,
    /// `C_base`.
    pub c_base: u64,
}

impl Bound {
    /// The bound: the terms' sum.
    #[must_use]
    pub fn total(&self) -> u64 {
        [
            self.i,
            self.c_thr,
            self.c_env,
            self.c_net,
            self.c_spec,
            self.c_parse,
            self.c_hdr,
            self.c_fin,
            self.c_base,
        ]
        .into_iter()
        .fold(0, u64::saturating_add)
    }
}

fn mul(a: u64, b: u64) -> u64 {
    a.saturating_mul(b)
}

/// 5.1's bound for `spec` under `k`: `phase1` is the counting allocator's
/// measure of phase 1 on the main thread (`C_spec`'s measured part and
/// `p`), and `env_block` the main thread's environment block.
#[must_use]
pub fn memory_bound(spec: &Spec, phase1: Measured, env_block: u64, k: &Constants) -> Bound {
    let threads = u64::from(spec.threads);
    let dc = u64::from(spec.download_concurrency);
    let pool = mul(2, dc + 1);
    let limits = Limits::default();
    let stack = u64::try_from(limits.stack_needed()).unwrap_or(u64::MAX);
    let levels = u64::from(limits.max_container_depth) + 1;
    let worker = mul(2, stack)
        .saturating_add(mul(CONTAINER_LEVEL_STACK, levels))
        .saturating_add(WORKER_STATE)
        .saturating_add(PANIC_SLOT);
    let c_thr = mul(threads, worker)
        .saturating_add(5 * THREAD_STACK)
        .saturating_add(env_block)
        .saturating_add(mul(pool, THREAD_STACK));
    let c_env = mul(
        threads,
        k.c_env
            .saturating_add(mul(u64::from(spec.max_view_bundles), KIB)),
    );
    let sockets = mul(2, dc) + 1;
    let c_net = mul(sockets, k.c_net)
        .saturating_add(mul(dc, 2 * MIB))
        .saturating_add(mul(sockets, 64 * KIB));
    let bundles = spec.bundle_count() as u64;
    let c_spec = phase1.kept.saturating_add(mul(PER_BUNDLE, bundles));
    let i = spec.max_inflight_uncompressed_bytes;
    let c_parse = phase1
        .peak
        .saturating_sub(c_spec)
        .saturating_sub(i)
        .saturating_sub(C_FIN);
    let c_base = k
        .c_base
        .saturating_add(mul(threads, k.m_thread))
        .saturating_add(mul(pool, k.m_pool))
        .saturating_add(mul(dc, 2080))
        .saturating_add(mul(threads, k.m_dispatch))
        .saturating_add(mul(threads, k.m_end))
        .saturating_add(16 * KIB)
        .saturating_add(mul(spec.archives.len() as u64, 160));
    Bound {
        i,
        c_thr,
        c_env,
        c_net,
        c_spec,
        c_parse,
        c_hdr: c_hdr(spec),
        c_fin: C_FIN,
        c_base,
    }
}

/// `C_hdr` (5.3) with every `{"env": NAME}` value at its 16 KiB cap: the
/// distinct header values, the name-to-value table and the masker's
/// entries, `4 x download_concurrency` copies of the largest bundle's
/// header block (`name: value\r\n` each), and, with an object store sink,
/// whose credentials the engine reads, six copies of the credentials at
/// their caps.
#[must_use]
pub fn c_hdr(spec: &Spec) -> u64 {
    let mut literals: HashSet<&str> = HashSet::new();
    let mut vars: HashSet<&str> = HashSet::new();
    let mut largest = 0_u64;
    for b in spec.bundles() {
        let mut block = 0_u64;
        for (name, value) in b.headers() {
            let len = match value {
                HeaderValue::Literal(v) => {
                    literals.insert(v);
                    v.len() as u64
                }
                HeaderValue::Env(n) => {
                    vars.insert(n);
                    MAX_HEADER_VALUE
                }
            };
            block = block.saturating_add(name.len() as u64 + 4 + len);
        }
        largest = largest.max(block);
    }
    let n_vars = vars.len() as u64;
    let values = literals
        .iter()
        .map(|v| heap_cost(v.len() as u64))
        .fold(0, u64::saturating_add)
        .saturating_add(mul(n_vars, heap_cost(MAX_HEADER_VALUE)));
    let table = heap_cost(mul(n_vars, TABLE_ENTRY)).saturating_add(
        vars.iter()
            .map(|n| heap_cost(n.len() as u64))
            .fold(0, u64::saturating_add),
    );
    // A note for the PR that builds the masker (2.11 step 2), which does
    // not exist yet: this counts only its header part, an entry per
    // header variable pointing at the value `values` counts. Its spec
    // part (a signed URL's query, a literal header value) is built in
    // phase 1, so `C_spec`'s measure holds it only once phase 1 builds
    // it; its credentials part is one of the six credential copies
    // below. That PR checks each part lands in one of those terms, and
    // counts here, and in 5.3, any copy of a secret that lands in none.
    let masker = heap_cost(mul(n_vars, MASK_ENTRY));
    let blocks = if largest == 0 {
        0
    } else {
        mul(
            mul(4, u64::from(spec.download_concurrency)),
            heap_cost(largest),
        )
    };
    let credentials = if matches!(spec.sink, Sink::ObjectStore(_)) {
        mul(6, CREDENTIALS.iter().map(|&c| heap_cost(c)).sum())
    } else {
        0
    };
    values
        .saturating_add(table)
        .saturating_add(masker)
        .saturating_add(blocks)
        .saturating_add(credentials)
}

#[cfg(test)]
mod tests;
