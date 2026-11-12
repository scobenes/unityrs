use std::sync::Arc;

use super::{ObjectInfo, SerializedType};
use crate::raw::RawStr;
use crate::typetree::{NodeList, NodeRow};

#[test]
fn package_b_can_build_ref_types_without_package_c() {
    let nodes = Arc::new(
        NodeList::trusted(vec![
            NodeRow::new(0, "Foo", "Base"),
            NodeRow::new(1, "int", "x"),
        ])
        .unwrap(),
    );
    let t = SerializedType::ref_type(
        114,
        RawStr::from("Foo"),
        RawStr::from("Ns"),
        RawStr::from("Asm.dll"),
        Some(nodes),
    );
    assert_eq!(
        t.class_name.as_ref().map(RawStr::as_bytes),
        Some(&b"Foo"[..])
    );
    assert_eq!(t.script_type_index, -1);
    let plain = SerializedType::new(1, None);
    assert!(plain.class_name.is_none());
    assert!(
        plain.tree(&crate::limits::Limits::default()).is_none(),
        "no stored tree, nothing to compile"
    );
}

#[test]
fn a_refused_compile_is_not_kept() {
    use crate::budget::{Counting, Reservation};
    use crate::limits::{ChargeError, Meter, MeteredVec};
    use crate::typetree::{Tree, TreeError};
    // Round 3: a compile the pool refused (while another reader held it)
    // was published, failing every object of the type for good.
    let pool = Arc::new(Counting::new(4096));
    let meter = Meter::new(u64::MAX, pool.clone(), Reservation::Dependency);
    let rows = MeteredVec::from_vec(vec![NodeRow::new(0, "Base", "Base")], &meter).unwrap();
    let t = SerializedType::new(1, Some(Arc::new(NodeList::from_rows(rows).unwrap())));
    let compile = |nodes: Arc<NodeList>| {
        let _scratch = nodes.meter().charge(1024).map_err(TreeError::Charge)?;
        Ok(Tree::for_tests(nodes))
    };
    let other = meter.charge(4096 - pool.used()).unwrap();
    let refused = t.tree_with(compile).unwrap();
    assert!(
        matches!(&refused, Err(TreeError::Charge(ChargeError::Budget(_)))),
        "{refused:?}"
    );
    drop(other);
    assert!(
        t.tree_with(compile).unwrap().is_ok(),
        "compiled again once the pool had room"
    );
    // A failure the rows decide is kept.
    let deep = SerializedType::new(2, t.nodes.clone());
    let too_deep = |_: Arc<NodeList>| {
        Err(TreeError::Limit(crate::LimitError::refused(
            crate::LimitKind::TreeDepth,
            2,
            1,
        )))
    };
    assert!(deep.tree_with(too_deep).unwrap().is_err());
    assert!(
        deep.tree_with(compile).unwrap().is_err(),
        "the kept error answers"
    );
}

#[test]
fn a_compile_the_meter_refuses_on_its_own_runs_a_bounded_number_of_times() {
    // Round 5 (claude5 code M1): a compile that by itself needs more than
    // the input's meter was refused as transient, never kept, and run
    // again by every ask (every object of the type), each time up to the
    // meter's limit. What each refused run was granted now counts as the
    // open's work, and once that is used up the cell keeps a refusal.
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::budget::{Reservation, Unbounded};
    use crate::limits::{Limits, Meter, MeteredVec};
    use crate::typetree::{Tree, TreeError};
    let limits = Limits {
        max_input_memory: 1 << 20,
        max_decode_work: 4 << 20,
        ..Limits::default()
    };
    let meter = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input);
    let rows = MeteredVec::from_vec(vec![NodeRow::new(0, "Base", "Base")], &meter).unwrap();
    let t = SerializedType::new(1, Some(Arc::new(NodeList::from_rows(rows).unwrap())));
    let rows_held = meter.used();
    let runs = AtomicUsize::new(0);
    let compile = |nodes: Arc<NodeList>| -> Result<Arc<Tree>, TreeError> {
        runs.fetch_add(1, Ordering::Relaxed);
        // A table that grows towards twice the meter.
        let mut table: MeteredVec<u8> = MeteredVec::new(nodes.meter());
        for _ in 0..(2 << 20) / 4096 {
            table.extend_from_slice(&[0; 4096])?;
        }
        Ok(Tree::for_tests(nodes))
    };
    let outcomes: Vec<TreeError> = (0..200)
        .map(|_| t.tree_with(compile).unwrap().unwrap_err())
        .collect();
    // Each refused run is granted just over 1 MiB (its table doubling up
    // to 512 KiB, at `heap_cost`), so the fourth uses up the 4 MiB.
    let runs = runs.load(Ordering::Relaxed);
    assert_eq!(runs, 4, "{runs} compiles for 200 asks");
    let memory = |e: &TreeError| matches!(e, TreeError::Charge(crate::limits::ChargeError::Limit(l)) if l.kind() == crate::LimitKind::InputMemory);
    let work = |e: &TreeError| matches!(e, TreeError::Limit(l) if l.kind() == crate::LimitKind::DecodeWork);
    assert!(
        outcomes[..runs].iter().all(memory),
        "{:?}",
        &outcomes[..runs]
    );
    assert!(outcomes[runs..].iter().all(work), "{:?}", outcomes.last());
    assert!(meter.decoded() >= 4 << 20);
    assert_eq!(
        meter.used(),
        rows_held,
        "every refused run returned its memory"
    );
}

#[test]
// The conversion keeps the test meaningful against a narrower field type.
#[allow(clippy::useless_conversion)]
fn byte_start_holds_what_python_adds() {
    // A format-22 object stored at i64::MAX in a file with data_offset 144
    // loads in UnityPy 1.25.2 with byte_start 9223372036854775951.
    let start = i128::from(i64::MAX) + 144;
    let info = ObjectInfo {
        path_id: 1,
        byte_start: start.try_into().unwrap(),
        byte_size: 4,
        type_id: 0,
        class_id: 1,
        type_index: Some(0),
        is_destroyed: None,
        is_stripped: None,
        script_type_index: None,
    };
    assert_eq!(info.byte_start, start);
}
