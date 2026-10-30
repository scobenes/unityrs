//! A tree a provider was refused memory for is the input's `crash` record,
//! as the oracle's `MemoryError` is, never the object's `no_typetree` (or
//! any L2 or L3) outcome, and the refusal is never kept: the next input
//! that asks for the same class gets the tree (`unityrs_dump::input`,
//! "Memory"; `ProviderError::memory`). The refusal depends on what the
//! process held at that moment, not on the input.
//!
//! - A real TPK provider whose walk the allocator refuses, once: a global
//!   allocator refuses the first allocation the provider makes while it is
//!   armed, on the test's thread only.
//! - A provider that answers a refusal at a chosen ask, with the chain's
//!   cache off: the dump asks for an object's tree three times (its L2
//!   lookup, its read, its resave), and a refusal at any of them is the
//!   input's crash.
//!
//! The TPK is built here (a class 1 whose tree is one node, `s0 s1`), so the
//! tests need no oracle image.

use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use unityrs_core::budget::{BudgetExceeded, Reservation};
use unityrs_core::limits::Limits;
use unityrs_core::provider::{ProviderChain, ProviderError, TreeRequest, TypeTreeProvider};
use unityrs_core::typetree::Tree;
use unityrs_core::typetree::commonstring::CommonStrings;
use unityrs_core::{Bytes, FileContext};
use unityrs_dump::DumpOptions;
use unityrs_testkit::counted::{Allocator, Tally};
use unityrs_testkit::serialized::{SerializedSpec, build, one_object};
use unityrs_tpk::TpkProvider;

std::thread_local! {
    /// Whether the next allocation on this thread is refused.
    static REFUSE_NEXT: Cell<bool> = const { Cell::new(false) };
}

/// Takes the refusal armed on this thread, if any. It allocates nothing.
fn refused_now() -> bool {
    REFUSE_NEXT.try_with(|c| c.replace(false)).unwrap_or(false)
}

/// Refuses the next allocation of the thread that armed it, once (a
/// `realloc` too: the old block stays the caller's, as a failed `realloc`
/// leaves it). It counts nothing.
struct OnceRefusing;

impl Tally for OnceRefusing {
    fn refuses(&self, _size: usize) -> bool {
        refused_now()
    }

    fn grew(&self, _size: usize, _zeroed: bool) {}

    fn shrank(&self, _size: usize) {}
}

#[global_allocator]
static ALLOC: Allocator<OnceRefusing> = Allocator(OnceRefusing);

/// A stored TPK (`helpers/Tpk.py:146-174`) whose blob (`:232-240`) holds
/// one version, 2.0.0, and one class, 1, from that version on, whose release
/// root is a node of type `s0` named `s1` with no sub-nodes.
fn tpk() -> Vec<u8> {
    const V2: u64 = 2 << 48;
    let i32_le = |n: i32| n.to_le_bytes();
    let mut blob = 1_i64.to_le_bytes().to_vec();
    // The versions.
    blob.extend_from_slice(&i32_le(1));
    blob.extend_from_slice(&V2.to_le_bytes());
    // The classes: class 1, one entry at 2.0.0, both roots node 0.
    blob.extend_from_slice(&i32_le(1));
    blob.extend_from_slice(&i32_le(1));
    blob.extend_from_slice(&i32_le(1));
    blob.extend_from_slice(&V2.to_le_bytes());
    blob.extend_from_slice(&[1, 7, 0, 0, 0, 192, 0, 0, 0, 0]);
    // No common strings, no indices.
    blob.extend_from_slice(&i32_le(0));
    blob.extend_from_slice(&i32_le(0));
    // One node (`<HHihbIH`): s0 s1, size -1, version 1, flags 0, meta
    // 0x4000, no sub-nodes.
    blob.extend_from_slice(&i32_le(1));
    blob.extend_from_slice(&[0, 0, 1, 0]);
    blob.extend_from_slice(&i32_le(-1));
    blob.extend_from_slice(&[1, 0, 0]);
    blob.extend_from_slice(&0x4000_u32.to_le_bytes());
    blob.extend_from_slice(&[0, 0]);
    // The strings.
    blob.extend_from_slice(&i32_le(2));
    blob.extend_from_slice(&[2, b's', b'0', 2, b's', b'1']);
    let size = u32::try_from(blob.len()).unwrap().to_le_bytes();
    let mut out = b"TPK*".to_vec();
    out.extend_from_slice(&[1, 0, 0, 0]);
    out.extend_from_slice(&0_u32.to_le_bytes());
    out.extend_from_slice(&size);
    out.extend_from_slice(&size);
    out.extend_from_slice(&blob);
    out
}

fn provider() -> TpkProvider {
    TpkProvider::from_bytes(&tpk(), &Limits::default()).unwrap()
}

/// A file with no type trees, of one object of class 1 with no bytes: its
/// tree is the TPK's.
fn stripped() -> Vec<u8> {
    build(&SerializedSpec {
        type_trees: false,
        unity_version: b"2021.3.2f1".to_vec(),
        ..one_object(1, Vec::new(), Vec::new())
    })
}

/// The dump of [`stripped`] with `providers` as the context's.
fn dumped(providers: ProviderChain) -> unityrs_dump::InputDump {
    let mut ctx = FileContext::unitypy();
    ctx.providers = Arc::new(providers);
    let opts = DumpOptions {
        context: Arc::new(ctx),
        ..DumpOptions::default()
    };
    unityrs_dump::dump_input(b"stripped", Bytes::from(stripped()), &opts)
}

fn dump(providers: ProviderChain) -> String {
    dumped(providers).text()
}

/// The note of an input's one `crash` record, or a panic naming what the
/// dump wrote instead.
fn crash_note(text: &str) -> &str {
    let mut lines = text.lines();
    let (Some(line), None) = (lines.next(), lines.next()) else {
        panic!("not one record: {text}");
    };
    let at = line
        .find(r#""error":{"kind":"crash","note":""#)
        .unwrap_or_else(|| panic!("not a crash record: {text}"));
    let note = &line[at + r#""error":{"kind":"crash","note":""#.len()..];
    note.strip_suffix("\"}}").unwrap()
}

/// Arms a refusal of the next allocation the provider makes, on its next
/// ask only.
#[derive(Debug)]
struct Arming(TpkProvider);

static ARMED: AtomicBool = AtomicBool::new(false);

impl TypeTreeProvider for Arming {
    fn name(&self) -> &'static str {
        TpkProvider::NAME
    }
    fn common_strings(&self) -> Option<&CommonStrings> {
        self.0.common_strings()
    }
    fn tree(&self, req: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
        let arm = ARMED.swap(false, Ordering::SeqCst);
        REFUSE_NEXT.with(|c| c.set(arm));
        let got = self.0.tree(req);
        REFUSE_NEXT.with(|c| c.set(false));
        got
    }
}

/// The claude4-e and claude4-x round-4 finding: the TPK provider kept an
/// allocator's refusal as the class's outcome, for every later input of
/// the run, and the dump wrote it as the object's `no_typetree`.
#[test]
fn a_tpk_walk_the_allocator_refuses_is_the_inputs_crash_and_is_not_kept() {
    let fresh = dump(ProviderChain::new(vec![Arc::new(provider())]));
    // The object's tree is the TPK's.
    assert!(
        fresh.contains(r#""l2":{"root_type":"s0","source":"tpk","error":null,"#),
        "{fresh}"
    );
    let chain = ProviderChain::new(vec![Arc::new(Arming(provider()))]);
    let chain = Arc::new(chain);
    let dump_on = |chain: &Arc<ProviderChain>| {
        let mut ctx = FileContext::unitypy();
        ctx.providers = chain.clone();
        let opts = DumpOptions {
            context: Arc::new(ctx),
            ..DumpOptions::default()
        };
        unityrs_dump::dump_input(b"stripped", Bytes::from(stripped()), &opts).text()
    };
    ARMED.store(true, Ordering::SeqCst);
    let refused = dump_on(&chain);
    assert!(
        !ARMED.load(Ordering::SeqCst),
        "the provider was never asked"
    );
    let note = crash_note(&refused);
    assert!(
        note.starts_with("unityrs ran out of memory dumping this input (memory budget refused ")
            && note.ends_with(" bytes for Provider)"),
        "{note}"
    );
    // The same options, memory back: the provider walks again.
    assert_eq!(dump_on(&chain), fresh);
    // And the provider answers its own callers again too, through either
    // door, each on a provider that has walked nothing yet.
    let version = unityrs_core::UnityVersion::from_packed(2 << 48);
    let req = TreeRequest {
        class_id: 1,
        version: &version,
        script: None,
    };
    let p = provider();
    let e = refusing_next(|| p.tree(&req)).unwrap_err();
    assert_eq!(
        e.memory.as_ref().map(|m| m.what),
        Some(Reservation::Provider),
        "{e:?}"
    );
    assert!(e.limit.is_none(), "{e:?}");
    assert!(p.tree(&req).unwrap().is_some());
    let p = provider();
    let e = refusing_next(|| p.node_list(1, &version)).unwrap_err();
    assert!(e.memory.is_some(), "{e:?}");
    assert!(p.node_list(1, &version).unwrap().is_some());
}

/// `f`'s result with the first allocation it makes refused (none if it
/// makes none).
fn refusing_next<T>(f: impl FnOnce() -> T) -> T {
    REFUSE_NEXT.with(|c| c.set(true));
    let got = f();
    REFUSE_NEXT.with(|c| c.set(false));
    got
}

/// Answers a refusal of memory at its `on`-th ask, and the TPK's tree at
/// every other.
#[derive(Debug)]
struct RefusingAt {
    tpk: TpkProvider,
    asks: AtomicUsize,
    on: usize,
}

impl TypeTreeProvider for RefusingAt {
    fn name(&self) -> &'static str {
        TpkProvider::NAME
    }
    fn tree(&self, req: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
        if self.asks.fetch_add(1, Ordering::SeqCst) + 1 == self.on {
            let b = BudgetExceeded {
                requested: 64,
                what: Reservation::Provider,
            };
            return Err(ProviderError {
                provider: TpkProvider::NAME,
                detail: b.to_string().into(),
                limit: None,
                memory: Some(b),
            });
        }
        self.tpk.tree(req)
    }
}

fn refusing_at(on: usize) -> Arc<RefusingAt> {
    Arc::new(RefusingAt {
        tpk: provider(),
        asks: AtomicUsize::new(0),
        on,
    })
}

/// A refusal at the object's tree lookup, at its read's or at its resave's
/// is the input's crash: before, it was a `no_typetree`, an `other` L2
/// error and a resave `error` in turn.
#[test]
fn a_tree_refused_at_any_of_the_dumps_asks_is_the_inputs_crash() {
    // No cache: every ask reaches the provider.
    let uncached = |p: Arc<RefusingAt>| ProviderChain::with_cache_bound(vec![p], 0, 0);
    let never = refusing_at(usize::MAX);
    let whole = dumped(uncached(never.clone()));
    // Review round 7: the run counts each input whose record is a crash
    // (`crashes`, which the oracle exits 3 on when every input has one).
    assert_eq!(whole.crashes, 0);
    let whole = whole.text();
    assert_eq!(
        never.asks.load(Ordering::SeqCst),
        3,
        "the dump asks for the object's tree for its L2 lookup, its read and its resave: {whole}"
    );
    assert!(
        whole.contains(r#""source":"tpk","error":null,"#) && whole.contains(r#""resave":"equal""#),
        "{whole}"
    );
    for on in 1..=3 {
        let d = dumped(uncached(refusing_at(on)));
        assert_eq!(d.crashes, 1, "ask {on}");
        let text = d.text();
        assert_eq!(
            crash_note(&text),
            "unityrs ran out of memory dumping this input (memory budget refused 64 bytes for Provider)",
            "ask {on}"
        );
    }
}
