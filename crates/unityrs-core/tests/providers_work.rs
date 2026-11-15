//! Input-driven provider work must be refused before it is performed.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use unityrs_core::RawStr;
use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::limits::{ChargeError, Meter, MeteredVec};
use unityrs_core::provider::generator::{
    DumpEntry, DumpProvider, GeneratorProvider, ScriptGenerator, generator_node_list,
};
use unityrs_core::typetree::{
    FromListError, NodeDict, NodeList, NodeRow, Tree, TreeError, TypeTreeNode,
};

fn work_meter(work: u64) -> Arc<Meter> {
    Meter::with_decode_work(u64::MAX, work, Arc::new(Unbounded), Reservation::Provider)
}

#[test]
fn fallback_warning_copies_refuse_zero_and_small_work_allowances() {
    use unityrs_core::config::{FallbackWarnings, UnityVersionFallbackError, get_fallback_version};
    let value = RawStr::from("x".repeat(10_000).as_str());
    for allowance in [0, 20] {
        let meter = work_meter(allowance);
        let mut warnings = FallbackWarnings::new(&meter);
        assert!(matches!(
            get_fallback_version(Some(&value), &mut warnings, |_| panic!("refused warning")),
            Err(UnityVersionFallbackError::Charge(ChargeError::Limit(e)))
                if e.kind() == unityrs_core::LimitKind::DecodeWork
        ));
        assert_eq!(meter.decoded(), 0);
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn fallback_warning_comparisons_charge_steps_and_bytes_before_comparing() {
    use unityrs_core::config::{FallbackWarnings, UnityVersionFallbackError, get_fallback_version};
    let value = RawStr::from("x".repeat(10_000).as_str());
    for allowance in [10_000, 10_020, 20_001] {
        let meter = work_meter(allowance);
        let mut warnings = FallbackWarnings::new(&meter);
        let mut emitted = 0;
        get_fallback_version(Some(&value), &mut warnings, |_| emitted += 1).unwrap();
        assert_eq!(meter.decoded(), 10_000);
        let used = meter.used();
        let result = get_fallback_version(Some(&value), &mut warnings, |_| emitted += 1);
        if allowance == 20_001 {
            result.unwrap();
            assert_eq!(meter.decoded(), 20_001);
        } else {
            assert!(matches!(result,
                Err(UnityVersionFallbackError::Charge(ChargeError::Limit(e)))
                    if e.kind() == unityrs_core::LimitKind::DecodeWork));
            assert_eq!(meter.decoded(), 10_000);
        }
        assert_eq!(meter.used(), used);
        assert_eq!(emitted, 1);
    }
    // Unequal lengths still cost one step, including empty warning keys.
    let empty = RawStr::from_static(b"");
    for allowance in [0, 1] {
        let meter = work_meter(allowance);
        let mut warnings = FallbackWarnings::new(&meter);
        get_fallback_version(Some(&empty), &mut warnings, |_| {}).unwrap();
        let result = get_fallback_version(Some(&empty), &mut warnings, |_| panic!("duplicate"));
        assert_eq!(result.is_ok(), allowance == 1);
        assert_eq!(meter.decoded(), allowance);
    }
    let meter = work_meter(1);
    let mut warnings = FallbackWarnings::new(&meter);
    get_fallback_version(Some(&empty), &mut warnings, |_| {}).unwrap();
    assert!(get_fallback_version(Some(&value), &mut warnings, |_| panic!("refused")).is_err());
    assert_eq!(meter.decoded(), 1);
}

fn tree() -> Arc<Tree> {
    Tree::compile(
        Arc::new(NodeList::trusted(vec![NodeRow::new(0, "int", "x")]).unwrap()),
        &unityrs_core::limits::Limits::default(),
    )
    .unwrap()
}

#[test]
fn catalogue_comparisons_refuse_zero_and_small_work_allowances() {
    let tree = tree();
    for allowance in [0, 20] {
        let meter = work_meter(allowance);
        let entries = (0..1000)
            .map(|_| {
                DumpEntry::il2cpp(
                    RawStr::from_static(b"Assembly.dll"),
                    RawStr::from_static(b"Synthetic.Example"),
                    tree.clone(),
                )
            })
            .collect();
        let provider = DumpProvider::new(MeteredVec::from_vec(entries, &meter).unwrap());
        let error = provider
            .get_nodes(
                &RawStr::from_static(b"Assembly.dll"),
                &RawStr::from_static(b"Synthetic.Missing"),
            )
            .unwrap_err();
        assert_eq!(
            error.limit.unwrap().kind(),
            unityrs_core::LimitKind::DecodeWork
        );
        assert!(meter.decoded() <= allowance);
    }
}

#[test]
fn catalogue_counts_each_comparison_before_reading_its_bytes() {
    let tree = tree();
    for allowance in [3, 4] {
        let meter = work_meter(allowance);
        let entry = DumpEntry::il2cpp(
            RawStr::from_static(b"a"),
            RawStr::from_static(b"x"),
            tree.clone(),
        );
        let provider = DumpProvider::new(MeteredVec::from_vec(vec![entry], &meter).unwrap());
        let answer = provider.get_nodes(&RawStr::from_static(b"a"), &RawStr::from_static(b"x"));
        if allowance == 3 {
            assert_eq!(
                answer.unwrap_err().limit.unwrap().kind(),
                unityrs_core::LimitKind::DecodeWork
            );
            assert_eq!(meter.decoded(), 2);
        } else {
            assert!(Arc::ptr_eq(&answer.unwrap().unwrap(), &tree));
            assert_eq!(meter.decoded(), 4);
        }
    }
}

#[test]
fn dictionary_and_api_name_copies_refuse_before_copying_long_strings() {
    let name = RawStr::from("x".repeat(1000).as_str());
    let tree = tree();
    for allowance in [0, 20] {
        let meter = work_meter(allowance);
        let dict = NodeDict {
            level: Some(0),
            ty: Some(RawStr::from_static(b"int")),
            name: Some(name.clone()),
            ..NodeDict::default()
        };
        assert!(matches!(TypeTreeNode::from_list(&[dict], &meter),
            Err(FromListError::Tree(TreeError::Charge(ChargeError::Limit(e))))
                if e.kind() == unityrs_core::LimitKind::DecodeWork));
        assert_eq!(meter.decoded(), u64::from(allowance != 0));
        assert_eq!(meter.used(), 0);
        let meter = work_meter(allowance);
        assert!(
            matches!(DumpEntry::api(RawStr::from_static(b"Assembly.dll"), &name,
            RawStr::from_static(b"Example"), tree.clone(), &meter),
            Err(ChargeError::Limit(e)) if e.kind() == unityrs_core::LimitKind::DecodeWork)
        );
        assert_eq!(meter.decoded(), 0);
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn dictionary_and_generator_node_processing_refuse_zero_and_small_allowances() {
    let dicts: Vec<_> = (0..1000)
        .map(|i| NodeDict {
            level: Some(i32::from(i != 0)),
            ty: Some(RawStr::from_static(b"int")),
            name: Some(RawStr::from_static(b"x")),
            ..NodeDict::default()
        })
        .collect();
    for allowance in [0, 20] {
        let meter = work_meter(allowance);
        let error = TypeTreeNode::from_list(&dicts, &meter).unwrap_err();
        assert!(
            matches!(error, FromListError::Tree(TreeError::Charge(ChargeError::Limit(e)))
            if e.kind() == unityrs_core::LimitKind::DecodeWork)
        );
        assert!(meter.decoded() <= allowance);
        assert_eq!(meter.used(), 0);
        let meter = work_meter(allowance);
        let rows = MeteredVec::from_vec(
            (0..1000)
                .map(|i| NodeRow::new(i32::from(i != 0), "int", "x"))
                .collect(),
            &meter,
        )
        .unwrap();
        assert!(
            matches!(generator_node_list(rows), Err(TreeError::Charge(ChargeError::Limit(e)))
            if e.kind() == unityrs_core::LimitKind::DecodeWork)
        );
    }
}

#[derive(Debug)]
struct Answer(Arc<Tree>, Arc<AtomicUsize>);
impl ScriptGenerator for Answer {
    fn get_nodes(
        &self,
        _: &RawStr,
        _: &RawStr,
    ) -> Result<Option<Arc<Tree>>, unityrs_core::provider::ProviderError> {
        self.1.fetch_add(1, Ordering::Relaxed);
        Ok(Some(self.0.clone()))
    }
}

#[test]
fn cache_hits_and_string_copies_spend_work_before_lookup_or_retention() {
    // An uncached answer would be cheap, but normalizing/copying this key is not.
    for allowance in [0, 20] {
        let meter = work_meter(allowance);
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = GeneratorProvider::new(
            Answer(tree(), calls.clone()),
            &meter,
            1000,
            Arc::from("refused"),
        );
        let error = provider
            .get_nodes_up(&RawStr::from("a".repeat(1000).as_str()), &RawStr::from("x"))
            .unwrap_err();
        assert_eq!(
            error.limit.unwrap().kind(),
            unityrs_core::LimitKind::DecodeWork
        );
        assert_eq!(provider.cached(), 0);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }
    let meter = work_meter(1000);
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = GeneratorProvider::new(
        Answer(tree(), calls.clone()),
        &meter,
        1000,
        Arc::from("refused"),
    );
    let assembly = RawStr::from_static(b"Assembly.dll");
    let fullname = RawStr::from_static(b"Synthetic.Example");
    provider.get_nodes_up(&assembly, &fullname).unwrap();
    // Exhaust only the remaining allowance; the cached tree is already built.
    meter.count_decode(1000 - meter.decoded()).unwrap();
    let error = provider.get_nodes_up(&assembly, &fullname).unwrap_err();
    assert_eq!(
        error.limit.unwrap().kind(),
        unityrs_core::LimitKind::DecodeWork
    );
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[test]
fn cache_key_copies_are_charged_after_the_callback_before_retention() {
    let meter = work_meter(20);
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = GeneratorProvider::new(
        Answer(tree(), calls.clone()),
        &meter,
        1,
        Arc::from("refused"),
    );
    let error = provider
        .get_nodes_up(
            &RawStr::from_static(b"A.dll"),
            &RawStr::from("x".repeat(1000).as_str()),
        )
        .unwrap_err();
    assert_eq!(
        error.limit.unwrap().kind(),
        unityrs_core::LimitKind::DecodeWork
    );
    assert_eq!(meter.decoded(), 10); // suffix check (5), assembly copy (5).
    assert_eq!(provider.cached(), 0);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}
