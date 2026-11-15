//! K1 allocations are charged before the allocator sees them, including growth.
#![allow(clippy::unwrap_used)]
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use unityrs_core::RawStr;
use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation};
use unityrs_core::config::{FallbackWarnings, get_fallback_version};
use unityrs_core::limits::{Limits, Meter, MeteredVec, heap_cost};
use unityrs_core::provider::generator::{DumpEntry, DumpProvider, GeneratorProvider};
use unityrs_core::typetree::{NodeDict, NodeList, NodeRow, Tree, TypeTreeNode};
use unityrs_testkit::counted::{Allocator, Tally, count_this_thread};

static LIVE: AtomicU64 = AtomicU64::new(0);
static BASE: AtomicU64 = AtomicU64::new(0);
static CHARGED: AtomicU64 = AtomicU64::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);
static VIOLATION: AtomicBool = AtomicBool::new(false);
struct Heap;
impl Tally for Heap {
    fn growing(&self, size: usize) {
        if ARMED.load(Ordering::Relaxed)
            && LIVE.load(Ordering::Relaxed) + heap_cost(size as u64)
                > BASE.load(Ordering::Relaxed) + CHARGED.load(Ordering::Relaxed)
        {
            VIOLATION.store(true, Ordering::Relaxed);
        }
    }
    fn grew(&self, size: usize, _: bool) {
        LIVE.fetch_add(heap_cost(size as u64), Ordering::Relaxed);
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(heap_cost(size as u64), Ordering::Relaxed);
    }
}
#[global_allocator]
static ALLOC: Allocator<Heap> = Allocator(Heap);
#[derive(Debug)]
struct Home;
impl PermitHome for Home {
    fn release(&self, bytes: u64) {
        CHARGED.fetch_sub(bytes, Ordering::Relaxed);
    }
}
#[derive(Debug)]
struct Budget(Arc<Home>);
impl MemoryBudget for Budget {
    fn try_reserve(&self, bytes: u64, _: Reservation) -> Result<Permit, BudgetExceeded> {
        CHARGED.fetch_add(bytes, Ordering::Relaxed);
        Ok(Permit::new(bytes, self.0.clone()))
    }
}
fn check(f: impl FnOnce()) {
    BASE.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
    VIOLATION.store(false, Ordering::Relaxed);
    ARMED.store(true, Ordering::Relaxed);
    f();
    ARMED.store(false, Ordering::Relaxed);
    assert!(
        !VIOLATION.load(Ordering::Relaxed),
        "allocation preceded its charge"
    );
    assert_eq!(CHARGED.load(Ordering::Relaxed), 0);
    assert_eq!(LIVE.load(Ordering::Relaxed), BASE.load(Ordering::Relaxed));
}

#[test]
fn node_graph_warning_keys_and_dump_cache_charge_before_allocating_and_release() {
    count_this_thread();
    let meter = Meter::new(
        u64::MAX,
        Arc::new(Budget(Arc::new(Home))),
        Reservation::Provider,
    );
    let dictionaries: Vec<_> = (0..256)
        .map(|i| NodeDict {
            level: Some(i32::from(i != 0)),
            ty: Some(RawStr::from(b"int".to_vec())),
            name: Some(RawStr::from(b"x".to_vec())),
            ..NodeDict::default()
        })
        .collect();
    check(|| {
        let mut nodes = TypeTreeNode::from_list(&dictionaries, &meter).unwrap();
        nodes.relink().unwrap();
        let snapshot = nodes
            .node_list(&Limits::default())
            .unwrap()
            .into_shared()
            .unwrap();
        assert_eq!(snapshot.len(), 511);
    });
    let text = RawStr::from("a".repeat(16384).as_str());
    check(|| {
        let mut warnings = FallbackWarnings::new(&meter);
        get_fallback_version(Some(&text), &mut warnings, |_| {}).unwrap();
        get_fallback_version(Some(&text), &mut warnings, |_| {}).unwrap();
    });
    let tree = Tree::compile(
        NodeList::trusted(vec![NodeRow::new(0, "int", "x")])
            .unwrap()
            .into_shared()
            .unwrap(),
        &Limits::default(),
    )
    .unwrap();
    let diagnostic: Arc<str> = Arc::from("refused");
    check(|| {
        let mut entries = MeteredVec::new(&meter);
        entries
            .push(
                DumpEntry::api(
                    RawStr::from_static(b"Assembly.dll"),
                    &text,
                    RawStr::from_static(b"Example"),
                    tree.clone(),
                    &meter,
                )
                .unwrap(),
            )
            .unwrap();
        let provider =
            GeneratorProvider::new(DumpProvider::new(entries), &meter, 1, diagnostic.clone());
        let fullname = RawStr::charged_concat(&[text.as_bytes(), b".Example"], &meter).unwrap();
        let assembly = RawStr::from_static(b"Assembly");
        assert!(
            provider
                .get_nodes_up(&assembly, &fullname)
                .unwrap()
                .is_some()
        );
        assert!(
            provider
                .get_nodes_up(&assembly, &fullname)
                .unwrap()
                .is_some()
        );
    });
    let mut input = dictionaries;
    for i in 0..128 {
        let at = i % input.len();
        input[at].level = Some(i32::try_from(i).unwrap() - 64);
        let small = Meter::new(
            256,
            Arc::new(unityrs_core::budget::Unbounded),
            Reservation::Input,
        );
        assert!(TypeTreeNode::from_list(&input, &small).is_err());
        assert_eq!(small.used(), 0);
    }
}
