use super::*;
use crate::budget::Reservation;
use crate::budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome};
use crate::{FileContext, LimitKind};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Default)]
struct Peak {
    used: AtomicU64,
    peak: AtomicU64,
}

#[derive(Debug)]
struct PeakBudget(Arc<Peak>);

impl MemoryBudget for PeakBudget {
    fn try_reserve(&self, bytes: u64, _: Reservation) -> Result<Permit, BudgetExceeded> {
        let used = self.0.used.fetch_add(bytes, Ordering::Relaxed) + bytes;
        self.0.peak.fetch_max(used, Ordering::Relaxed);
        Ok(Permit::new(bytes, self.0.clone()))
    }
}

impl PermitHome for Peak {
    fn release(&self, bytes: u64) {
        self.used.fetch_sub(bytes, Ordering::Relaxed);
    }
}

fn nested(meter: &Arc<Meter>, depth: u64) -> MeteredVec<Handler> {
    let mut handlers = MeteredVec::new(meter);
    let mut rows = MeteredVec::new(meter);
    rows.push(NodeRow::new(0, "Synthetic", "Base")).unwrap();
    for level in 1..=depth {
        let mut row = NodeRow::new(i32::try_from(level).unwrap(), "Part", "child");
        row.type_flags = Some(32);
        row.ref_type_hash = Some(0);
        rows.push(row).unwrap();
    }
    let mut refs = MeteredVec::new(meter);
    refs.push(key(1)).unwrap();
    handlers
        .push(Handler {
            version: 19,
            original_nodes: None,
            hash: key(0),
            nodes: Some(NodeList::from_rows(rows).unwrap().into_shared().unwrap()),
            refs,
        })
        .unwrap();
    let mut rows = MeteredVec::new(meter);
    rows.push(NodeRow::new(0, "Empty", "Base")).unwrap();
    handlers
        .push(Handler {
            version: 19,
            original_nodes: None,
            hash: key(1),
            nodes: Some(NodeList::from_rows(rows).unwrap().into_shared().unwrap()),
            refs: MeteredVec::new(meter),
        })
        .unwrap();
    handlers
}

fn nested_import(meter: &Arc<Meter>, depth: u64, wide: bool) -> MeteredVec<Handler> {
    let mut handlers = nested(meter, depth);
    let mut rows = MeteredVec::new(meter);
    rows.push(NodeRow::new(0, "Part", "Ignored")).unwrap();
    if wide {
        for _ in 0..depth {
            rows.push(NodeRow::new(1, "int", "value")).unwrap();
        }
    } else {
        rows.push(NodeRow::new(1, "Part", "ordinary")).unwrap();
        rows.push(NodeRow::new(2, "int", "value")).unwrap();
    }
    handlers.as_mut_slice()[1].nodes =
        Some(NodeList::from_rows(rows).unwrap().into_shared().unwrap());
    handlers
}

#[test]
fn nested_ordinary_imports_have_linear_work_and_peak_memory_at_the_depth_limit() {
    nested_import_growth(false);
}

#[test]
fn nested_wide_imports_have_linear_work_and_peak_memory_at_the_depth_limit() {
    nested_import_growth(true);
}

fn nested_import_growth(wide: bool) {
    let mut previous = None;
    for depth in [127, 255] {
        let peak = Arc::new(Peak::default());
        let mut ctx = FileContext::unitypy();
        ctx.limits.max_tree_depth = u32::try_from(depth + 1).unwrap();
        ctx.budget = Arc::new(PeakBudget(peak.clone()));
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        let mut handlers = nested_import(&meter, depth, wide);
        resolve(&mut handlers, 1, &meter, &ctx.limits).unwrap();
        let nodes = handlers.as_slice()[0].nodes.as_ref().unwrap();
        assert_eq!(
            nodes.len(),
            if wide {
                usize::try_from(depth).unwrap() + 2
            } else {
                4
            }
        );
        assert_eq!(nodes.rows()[1].name.as_bytes(), b"child");
        assert_eq!(nodes.rows().last().unwrap().name.as_bytes(), b"value");
        let work = meter.decoded();
        let memory = peak.peak.load(Ordering::Relaxed);
        eprintln!("wide={wide} depth={depth}: {work} work, {memory} peak bytes");
        if let Some((old_work, old_memory)) = previous {
            assert!(work * 2 < old_work * 5, "work: {old_work} -> {work}");
            assert!(
                memory * 2 < old_memory * 5,
                "peak: {old_memory} -> {memory}"
            );
        }
        previous = Some((work, memory));
        drop(handlers);
        assert_eq!(meter.used(), 0);
        assert_eq!(peak.used.load(Ordering::Relaxed), 0);
    }
}

#[test]
fn a_shared_placeholder_mutation_invalidates_ordinary_wrappers_and_all_list_owners() {
    let ctx = FileContext::unitypy();
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let mut handlers = nested_import(&meter, 3, false);
    let mut rows = MeteredVec::new(&meter);
    rows.push(NodeRow::new(0, "Part", "Ignored")).unwrap();
    rows.push(NodeRow::new(1, "Part", "ordinary")).unwrap();
    let mut placeholder = NodeRow::new(2, "Part", "shared");
    placeholder.type_flags = Some(32);
    placeholder.ref_type_hash = Some(1);
    rows.push(placeholder).unwrap();
    handlers.as_mut_slice()[1].nodes =
        Some(NodeList::from_rows(rows).unwrap().into_shared().unwrap());
    handlers.as_mut_slice()[0].refs.push(key(2)).unwrap();
    for _ in 0..2 {
        handlers.as_mut_slice()[1].refs.push(key(2)).unwrap();
    }
    let mut rows = MeteredVec::new(&meter);
    rows.push(NodeRow::new(0, "Part", "Ignored")).unwrap();
    rows.push(NodeRow::new(1, "int", "value")).unwrap();
    handlers
        .push(Handler {
            version: 19,
            original_nodes: None,
            hash: key(2),
            nodes: Some(NodeList::from_rows(rows).unwrap().into_shared().unwrap()),
            refs: MeteredVec::new(&meter),
        })
        .unwrap();
    handlers
        .push(Handler {
            version: 19,
            original_nodes: None,
            hash: key(3),
            nodes: None,
            refs: MeteredVec::new(&meter),
        })
        .unwrap();
    let index = HashIndex::new(handlers.as_slice(), 1, &meter).unwrap();
    let mut arena = Arena::new(handlers.as_slice(), &meter).unwrap();
    arena
        .resolve_roots(handlers.as_slice(), 1, &index, &meter, &ctx.limits)
        .unwrap();
    let list = arena.entry(1).unwrap().children;
    for owner in 1..=3 {
        assert_eq!(arena.entry(owner).unwrap().children, list);
        assert_eq!(arena.entry(owner).unwrap().marks.resolved, Some(0));
        assert!(
            arena
                .stable_children(owner, 0, handlers.as_slice(), &meter)
                .unwrap()
        );
    }
    let ordinary = arena.edge(arena.children(1).unwrap().start).unwrap();
    let shared = arena.edge(arena.children(ordinary).unwrap().start).unwrap();
    // Replacing the shared placeholder's children must dirty every importer,
    // through the ordinary wrapper and the one shared child-list identity.
    arena.assign(shared, 3, &meter).unwrap();
    for owner in 1..=3 {
        assert_eq!(arena.entry(owner).unwrap().marks.resolved, None);
        assert!(
            !arena
                .stable_children(owner, 0, handlers.as_slice(), &meter)
                .unwrap()
        );
    }
    drop((arena, index, handlers));
    assert_eq!(meter.used(), 0);
}

#[test]
fn shared_snapshots_have_linear_work_and_peak_memory() {
    for is_nested in [true, false] {
        let mut previous = None;
        for depth in [64, 128, 256] {
            let peak = Arc::new(Peak::default());
            let mut ctx = FileContext::unitypy();
            ctx.limits.max_tree_depth = 1024;
            ctx.budget = Arc::new(PeakBudget(peak.clone()));
            let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
            let mut handlers = if is_nested {
                nested(&meter, depth)
            } else {
                chain(&meter, depth)
            };
            resolve(&mut handlers, 1, &meter, &ctx.limits).unwrap();
            assert_eq!(
                handlers.as_slice()[0].nodes.as_ref().unwrap().len(),
                if is_nested {
                    2
                } else {
                    usize::try_from(depth).unwrap() + 2
                }
            );
            let work = meter.decoded();
            let memory = peak.peak.load(Ordering::Relaxed);
            eprintln!("nested={is_nested} depth={depth}: {work} work, {memory} peak bytes");
            if let Some((old_work, old_memory)) = previous {
                // Allow hash sort/search comparisons and vector growth rounding.
                assert!(work * 2 < old_work * 5, "work: {old_work} -> {work}");
                assert!(
                    memory * 2 < old_memory * 5,
                    "peak: {old_memory} -> {memory}"
                );
            }
            previous = Some((work, memory));
            drop(handlers);
            assert_eq!(meter.used(), 0);
            assert_eq!(peak.used.load(Ordering::Relaxed), 0);
        }
    }
}

fn chain(meter: &Arc<Meter>, links: u64) -> MeteredVec<Handler> {
    let mut handlers = MeteredVec::new(meter);
    for i in 0..=links {
        let mut rows = MeteredVec::new(meter);
        rows.push(NodeRow::new(0, "Synthetic", "Base")).unwrap();
        let mut child = NodeRow::new(1, "Part", "child");
        let mut refs = MeteredVec::new(meter);
        if i < links {
            child.type_flags = Some(32);
            child.ref_type_hash = Some(0);
            refs.push(key(i + 1)).unwrap();
        }
        rows.push(child).unwrap();
        handlers
            .push(Handler {
                version: 19,
                original_nodes: None,
                hash: key(i),
                nodes: Some(NodeList::from_rows(rows).unwrap().into_shared().unwrap()),
                refs,
            })
            .unwrap();
    }
    handlers
}

fn key(i: u64) -> Hash {
    let mut key = [0; 16];
    key[..8].copy_from_slice(&i.to_le_bytes());
    Hash::from(key)
}

// Literal HEAD algorithm for small trees: take a fresh descendant snapshot
// on every call, visit every placeholder in order, then replace children.
// It deliberately has neither snapshot sharing nor node-resolution caching.
fn head_order(
    arena: &mut Arena,
    handlers: &[Handler],
    node: usize,
    hashes: usize,
    resolved: &mut [u8],
) -> Result<(), Error> {
    fn descendants(arena: &Arena, node: usize, out: &mut Vec<usize>) -> Result<(), Error> {
        for at in arena.children(node)? {
            let child = arena.edge(at)?;
            out.push(child);
            descendants(arena, child, out)?;
        }
        Ok(())
    }
    let mut snapshot = Vec::new();
    descendants(arena, node, &mut snapshot)?;
    for child in snapshot {
        if arena.row(handlers, child)?.type_flags.unwrap_or(0) & 32 != 0 {
            head_order(arena, handlers, child, hashes, resolved)?;
        }
    }
    let row = arena.row(handlers, node)?;
    if row.type_flags.unwrap_or(0) & 32 == 0 {
        return Ok(());
    }
    let hash = row
        .ref_type_hash
        .and_then(|i| usize::try_from(i).ok())
        .and_then(|i| handlers[hashes].refs.as_slice().get(i))
        .ok_or_else(|| invalid("shared hash index out of range"))?;
    let dep = handlers
        .iter()
        .rposition(|h| h.hash == *hash)
        .ok_or_else(|| invalid("shared subtree hash not found"))?;
    match resolved[dep] {
        1 => return Err(invalid("cyclic shared subtree")),
        0 => {
            resolved[dep] = 1;
            if let Some(root) = arena.root(dep) {
                head_order(arena, handlers, root, dep, resolved)?;
            }
            resolved[dep] = 2;
        }
        _ => {}
    }
    let children = arena
        .root(dep)
        .map(|id| arena.entry(id).map(|entry| entry.children))
        .transpose()?
        .unwrap_or_else(|| {
            let id = arena.lists.len();
            arena
                .lists
                .push(ChildList {
                    edges: 0..0,
                    marks: Marks::default(),
                })
                .unwrap();
            id
        });
    arena.replace(node, children)
}

#[test]
fn snapshot_caching_preserves_heads_mutation_order() {
    let mut rng = unityrs_testkit::Rng::new(0x0417_d595);
    for case in 0..512 {
        let ctx = FileContext::unitypy();
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        let mut handlers = MeteredVec::new(&meter);
        for h in 0..4 {
            let mut rows = MeteredVec::new(&meter);
            rows.push(NodeRow::new(0, "Synthetic", "Base")).unwrap();
            let levels = if rng.next_u64() & 1 == 0 {
                [1, 2, 3]
            } else {
                [1, 2, 1]
            };
            for level in levels {
                let mut row = NodeRow::new(level, "Part", "child");
                if h < 3 && rng.next_u64() & 1 == 0 {
                    row.type_flags = Some(32);
                    row.ref_type_hash = Some(if h == 2 {
                        (rng.next_u64() % 2) * 2
                    } else {
                        rng.next_u64() % 3
                    });
                }
                rows.push(row).unwrap();
            }
            let mut refs = MeteredVec::new(&meter);
            for target in if h < 2 { vec![3, 2] } else { vec![3] } {
                refs.push(key(target)).unwrap();
            }
            handlers
                .push(Handler {
                    version: 19,
                    original_nodes: None,
                    hash: key(h),
                    nodes: Some(NodeList::from_rows(rows).unwrap().into_shared().unwrap()),
                    refs,
                })
                .unwrap();
        }
        let index = HashIndex::new(handlers.as_slice(), 2, &meter).unwrap();
        let mut expected = Arena::new(handlers.as_slice(), &meter).unwrap();
        let mut actual = Arena::new(handlers.as_slice(), &meter).unwrap();
        let mut resolved = [0; 4];
        let want = (0..2).try_for_each(|h| {
            let node = expected.root(h).unwrap();
            head_order(&mut expected, handlers.as_slice(), node, h, &mut resolved)
        });
        let got = actual.resolve_roots(handlers.as_slice(), 2, &index, &meter, &ctx.limits);
        assert_eq!(
            got.as_ref().err().map(ToString::to_string),
            want.as_ref().err().map(ToString::to_string),
            "case {case}"
        );
        for id in 0..expected.nodes.len() {
            assert_eq!(
                actual.children(id).unwrap(),
                expected.children(id).unwrap(),
                "case {case}, node {id}"
            );
        }
        drop((actual, expected, index, handlers));
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn a_mutation_created_cycle_hits_the_depth_limit_before_flattening() {
    let mut ctx = FileContext::unitypy();
    ctx.limits.max_tree_depth = 16;
    ctx.limits.max_decode_work = 1000;
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let handlers = chain(&meter, 1);
    let mut arena = Arena::new(handlers.as_slice(), &meter).unwrap();
    let root = arena.root(0).unwrap();
    let children = arena.children(root).unwrap();
    let child = arena.edge(children.start).unwrap();
    arena
        .replace(child, arena.entry(root).unwrap().children)
        .unwrap();
    let error = arena.check_heights(1, &meter, &ctx.limits).unwrap_err();
    assert!(matches!(error, Error::Limit(ref l) if l.kind() == LimitKind::TreeDepth));
    assert!(meter.decoded() < 1000);
    drop((arena, handlers));
    assert_eq!(meter.used(), 0);
}

#[test]
fn shared_chain_materialises_only_the_final_tree() {
    let mut previous = None;
    for links in [800, 1600] {
        let mut ctx = FileContext::unitypy();
        ctx.limits.max_input_memory = 128 << 20;
        ctx.limits.max_decode_work = 3_000_000;
        ctx.limits.max_tree_depth = 2000;
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        let mut handlers = chain(&meter, links);
        resolve(&mut handlers, 1, &meter, &ctx.limits).unwrap();
        assert_eq!(
            handlers.as_slice()[0].nodes.as_ref().unwrap().rows().len(),
            usize::try_from(links).unwrap() + 2
        );
        let work = meter.decoded();
        let memory = meter.used();
        assert!(work < links * 1500, "work: {work}");
        assert!(memory < 8 << 20, "memory: {memory}");
        if let Some((old_work, old_memory)) = previous {
            // Doubling the chain must not quadruple expansion work or storage.
            // The fixed-hash sort/search also pays its logarithmic comparisons.
            assert!(work * 2 < old_work * 5, "work: {old_work} -> {work}");
            assert!(
                memory * 2 < old_memory * 5,
                "memory: {old_memory} -> {memory}"
            );
        }
        eprintln!("{links} shared links: {work} work, {memory} retained bytes");
        previous = Some((work, memory));
        drop(handlers);
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn shared_chain_depth_is_refused_before_materialising() {
    let ctx = FileContext::unitypy();
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let mut handlers = chain(&meter, 300);
    let error = resolve(&mut handlers, 1, &meter, &ctx.limits).unwrap_err();
    assert!(matches!(error, Error::Limit(ref l) if l.kind() == LimitKind::TreeDepth));
    drop(handlers);
    assert_eq!(meter.used(), 0);
}

#[test]
fn unused_shared_handlers_spend_decode_work() {
    let mut ctx = FileContext::unitypy();
    ctx.limits.max_decode_work = 128;
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let mut handlers = MeteredVec::new(&meter);
    for i in 0..100_000 {
        handlers
            .push(Handler {
                version: 19,
                original_nodes: None,
                hash: key(i),
                nodes: None,
                refs: MeteredVec::new(&meter),
            })
            .unwrap();
    }
    let error = resolve(&mut handlers, 0, &meter, &ctx.limits).unwrap_err();
    assert!(matches!(error, Error::Limit(ref l) if l.kind() == LimitKind::DecodeWork));
    drop(handlers);
    assert_eq!(meter.used(), 0);
}

#[test]
fn reference_hash_reads_spend_decode_work() {
    fn data(count: u32) -> crate::Bytes {
        let mut data = vec![0; 16];
        data.extend(8u32.to_be_bytes());
        data.extend(b"mhtt");
        for n in [19i32, 1, 2] {
            data.extend(n.to_be_bytes());
        }
        data.extend([0; 32]);
        data.extend(b"x\0");
        data.extend(count.to_be_bytes());
        data.resize(data.len() + count as usize * 16, 0);
        data.into()
    }
    let mut ctx = FileContext::unitypy();
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let bytes = data(0);
    drop(
        Parser::new(
            &bytes,
            None,
            &meter,
            &ctx,
            crate::stream::ReaderKind::Memory,
        )
        .head_handler()
        .unwrap(),
    );
    ctx.limits.max_decode_work = meter.decoded() + 128;
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let bytes = data(100_000);
    let result = Parser::new(
        &bytes,
        None,
        &meter,
        &ctx,
        crate::stream::ReaderKind::Memory,
    )
    .head_handler();
    assert!(matches!(result, Err(Error::Limit(ref l)) if l.kind() == LimitKind::DecodeWork));
    assert_eq!(meter.used(), 0);
}

/// A small DAG doubles its children at every level. HEAD shares those
/// child lists; the flat Rust tree must pay for every materialised row.
#[test]
fn shared_expansion_charges_each_materialised_child() {
    for work_bound in [false, true] {
        let mut ctx = FileContext::unitypy();
        if work_bound {
            ctx.limits.max_decode_work = 1000;
        } else {
            ctx.limits.max_input_memory = 128 << 10;
        }
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        let mut handlers = MeteredVec::new(&meter);
        for i in 0u8..14 {
            let mut rows = MeteredVec::new(&meter);
            rows.push(NodeRow::new(0, "Synthetic", "Base")).unwrap();
            let mut refs = MeteredVec::new(&meter);
            if i < 13 {
                refs.push(Hash::from([i + 1; 16])).unwrap();
                for name in ["left", "right"] {
                    let mut row = NodeRow::new(1, "Synthetic", name);
                    row.type_flags = Some(32);
                    row.ref_type_hash = Some(0);
                    rows.push(row).unwrap();
                }
            } else {
                rows.push(NodeRow::new(1, "int", "value")).unwrap();
            }
            handlers
                .push(Handler {
                    version: 19,
                    original_nodes: None,
                    hash: Hash::from([i; 16]),
                    nodes: Some(NodeList::from_rows(rows).unwrap().into_shared().unwrap()),
                    refs,
                })
                .unwrap();
        }
        let error = resolve(&mut handlers, 1, &meter, &ctx.limits).unwrap_err();
        let Error::Limit(limit) = error else {
            panic!("unexpected refusal: {error}")
        };
        assert_eq!(
            limit.kind(),
            if work_bound {
                LimitKind::DecodeWork
            } else {
                LimitKind::InputMemory
            }
        );
        drop(handlers);
        assert_eq!(meter.used(), 0);
    }
}
