//! Save work shares the reader's cumulative, inclusive allowance.
#![allow(clippy::unwrap_used, clippy::panic)]
use std::sync::Arc;
use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::files::{SaveError, SaveLayout, SaveOptions, SerializedFile};
use unityrs_core::limits::{ChargeError, Meter, MeteredVec};
use unityrs_core::options::FileContext;
use unityrs_core::stream::Endian;
use unityrs_core::typetree::commonstring::CommonStrings;
use unityrs_core::typetree::{NodeList, NodeRow};
use unityrs_core::{Bytes, LimitKind, RawStr};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::serialized::{SerializedSpec, build, one_object};

fn meter(work: u64) -> Arc<Meter> {
    Meter::with_decode_work(1 << 20, work, Arc::new(Unbounded), Reservation::Input)
}

fn refused(result: Result<Bytes, SaveError>, bound: u64) -> u64 {
    let SaveError::Charge(ChargeError::Limit(error)) = result.unwrap_err() else {
        panic!("save must refuse decode work")
    };
    assert_eq!(error.kind(), LimitKind::DecodeWork);
    assert_eq!(error.max(), bound);
    assert!(error.value() > bound);
    error.value()
}

fn file(spec: &SerializedSpec, meter: &Arc<Meter>) -> Arc<SerializedFile> {
    SerializedFile::parse(
        RawStr::from_static(b"CAB-work"),
        Bytes::from(build(spec)),
        None,
        meter,
        &Arc::new(FileContext::unitypy()),
    )
    .unwrap()
}

#[test]
fn empty_file_save_counts_both_metadata_and_final_copies_at_exact_bounds() {
    let spec = SerializedSpec {
        format: 1,
        ..SerializedSpec::default()
    };
    // Three counts in metadata (12), then header (16), endian (1),
    // and the metadata copy (12): 41 bytes of work for 29 output bytes.
    for layout in [SaveLayout::Semantic, SaveLayout::UnityPy] {
        for bound in [1, 40, 41, 82] {
            let meter = meter(bound);
            let file = file(&spec, &meter);
            assert_eq!(meter.decoded(), 0);
            let before = meter.used();
            for _ in 0..bound / 41 {
                assert_eq!(file.save(SaveOptions { layout }).unwrap().len(), 29);
            }
            let done = meter.decoded();
            assert_eq!(done, (bound / 41) * 41);
            refused(file.save(SaveOptions { layout }), bound);
            assert!(meter.decoded() <= bound);
            assert_eq!(meter.used(), before);
            // Dropping a refused save cannot restore its work allowance.
            refused(file.save(SaveOptions { layout }), bound);
            assert!(meter.decoded() >= done);
        }
    }
}

#[test]
fn memory_refused_empty_save_can_retry_with_its_exact_work_allowance() {
    let spec = SerializedSpec {
        format: 1,
        ..SerializedSpec::default()
    };
    for layout in [SaveLayout::Semantic, SaveLayout::UnityPy] {
        let meter = meter(41);
        let file = file(&spec, &meter);
        let before = meter.used();
        let held = meter.charge(meter.limit() - before).unwrap();
        let SaveError::Charge(ChargeError::Limit(error)) =
            file.save(SaveOptions { layout }).unwrap_err()
        else {
            panic!("save must refuse memory")
        };
        assert_eq!(error.kind(), LimitKind::InputMemory);
        assert_eq!(meter.decoded(), 0, "refused growth writes nothing");
        drop(held);
        assert_eq!(meter.used(), before);
        assert_eq!(file.save(SaveOptions { layout }).unwrap().len(), 29);
        assert_eq!(meter.decoded(), 41);
        assert_eq!(meter.used(), before);
    }
}

#[test]
fn file_save_counts_object_bytes_padding_and_embedded_old_tree() {
    let mut spec = one_object(49, vec![NodeSpec::new(0, "aa", "aa")], vec![7; 3]);
    spec.format = 1;
    spec.objects[0].type_id = 49;
    // Reader searches two three-byte C strings and indexes their bytes (8).
    // Save writes metadata (66),
    // padded data (8), and final file (91), for another 165 bytes.
    for bound in [178, 179] {
        let meter = meter(bound);
        let file = file(&spec, &meter);
        assert_eq!(meter.decoded(), 14);
        let before = meter.used();
        let result = file.save(SaveOptions::default());
        if bound == 179 {
            assert_eq!(result.unwrap().len(), 91);
            assert_eq!(meter.decoded(), 179);
        } else {
            refused(result, bound);
        }
        assert_eq!(meter.used(), before);
    }
}

fn tree(meter: &Arc<Meter>) -> NodeList {
    let mut row = NodeRow::new(0, "aa", "aa");
    row.type_flags = Some(0);
    row.index = Some(0);
    row.meta_flag = Some(0);
    let mut rows = MeteredVec::new(meter);
    rows.push(row).unwrap();
    NodeList::from_rows(rows).unwrap()
}

#[test]
fn old_tree_save_has_an_inclusive_cumulative_work_bound() {
    // Two C strings (6) and six i32 fields (24).
    for bound in [29, 30, 60] {
        let meter = meter(bound);
        let tree = tree(&meter);
        assert_eq!(meter.decoded(), 0);
        let before = meter.used();
        for _ in 0..bound / 30 {
            assert_eq!(tree.dump_old(1, Endian::Little).unwrap().len(), 30);
        }
        assert_eq!(meter.decoded(), (bound / 30) * 30);
        refused(tree.dump_old(1, Endian::Little), bound);
        assert!(meter.decoded() <= bound);
        assert_eq!(meter.used(), before);
    }
}

#[test]
fn blob_tree_save_counts_string_comparisons_and_each_buffer_copy() {
    // One two-byte sort comparison, two binary searches of two equal
    // strings (four comparisons), a local string (3), node (24), and final
    // counts plus copied buffers (35): 72 work bytes, 35 output bytes.
    for (strings, size) in [(vec![], 35), (vec![RawStr::from_static(b"aa"); 2], 32)] {
        // Two equal common strings add one sort comparison and two lookup
        // comparisons (6 work bytes), replacing two copies of a 3-byte
        // local string. The last duplicate offset (3) wins in both layouts.
        let common = CommonStrings::new(strings, &Meter::unlimited()).unwrap();
        for layout in [SaveLayout::Semantic, SaveLayout::UnityPy] {
            for bound in [71, 72, 144] {
                let meter = meter(bound);
                let tree = tree(&meter);
                let before = meter.used();
                for _ in 0..bound / 72 {
                    assert_eq!(
                        tree.dump_blob(18, Endian::Little, layout, Some(&common))
                            .unwrap()
                            .len(),
                        size
                    );
                }
                assert_eq!(meter.decoded(), (bound / 72) * 72);
                refused(
                    tree.dump_blob(18, Endian::Little, layout, Some(&common)),
                    bound,
                );
                assert!(meter.decoded() <= bound);
                assert_eq!(meter.used(), before);
            }
        }
    }
}

#[test]
fn blob_sort_refuses_comparisons_before_writing_any_node() {
    let common = CommonStrings::new(
        vec![
            RawStr::from_static(
                b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            );
            2
        ],
        &Meter::unlimited(),
    )
    .unwrap();
    for bound in [1, 3] {
        let meter = meter(bound);
        let tree = tree(&meter);
        let before = meter.used();
        let attempted = refused(
            tree.dump_blob(18, Endian::Little, SaveLayout::Semantic, Some(&common)),
            bound,
        );
        assert_eq!(attempted, if bound == 1 { 2 } else { 66 });
        assert_eq!(meter.decoded(), bound - 1);
        assert_eq!(meter.used(), before);
    }
}
