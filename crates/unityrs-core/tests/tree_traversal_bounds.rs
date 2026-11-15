//! Mutable graph snapshots reject cycles before repeating the cyclic edge.
#![allow(clippy::unwrap_used)]
use std::sync::Arc;
use unityrs_core::{
    budget::{Reservation, Unbounded},
    limits::{Limits, Meter},
    typetree::{NodeRow, TypeTreeNode},
};

#[test]
fn traversal_and_snapshot_reject_cycles_before_depth_exhaustion() {
    let meter = Meter::with_decode_work(100_000, 10_000, Arc::new(Unbounded), Reservation::Input);
    let leaf = Arc::new(TypeTreeNode::new(&NodeRow::new(0, "int", "leaf"), &[], &meter).unwrap());
    let mut graph = TypeTreeNode::new(&NodeRow::new(1, "Root", "root"), &[leaf], &meter).unwrap();
    // Direct construction accepts arbitrary levels. Relinking the leaf then
    // the root appends the root under its own descendant.
    graph.relink_nodes(&[1, 0]).unwrap();
    let limits = Limits::default();
    for error in [
        graph.traversal(&limits).unwrap_err(),
        graph.node_list(&limits).unwrap_err(),
    ] {
        assert!(error.to_string().contains("cycle"), "{error}");
    }
    assert!(meter.decoded() < 100);
}

#[test]
fn old_parser_checks_depth_before_reading_the_next_node() {
    use unityrs_core::{
        Bytes,
        stream::{Endian, Reader},
        typetree::parse::parse_old,
    };
    let mut data = Vec::new();
    for _ in 0..4 {
        data.extend_from_slice(b"R\0n\0");
        for value in [0_i32, 0, 1, 1] {
            data.extend_from_slice(&value.to_le_bytes());
        }
    }
    let data = Bytes::from(data);
    let mut limits = Limits::default();
    limits.max_tree_depth = 3;
    let meter = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input);
    let mut reader = Reader::new(&data, Endian::Little);
    let error = parse_old(&mut reader, &data, 3, &meter, &limits).unwrap_err();
    assert!(error.to_string().contains("TreeDepth"), "{error}");
    assert_eq!(reader.position(), 60);
}

#[test]
fn blob_common_string_rows_cannot_bypass_work_limits() {
    use unityrs_core::{
        Bytes,
        stream::{Endian, Reader},
        typetree::parse::parse_blob,
    };
    let mut data = Vec::new();
    data.extend_from_slice(&100_i32.to_le_bytes());
    data.extend_from_slice(&0_i32.to_le_bytes());
    for _ in 0..100 {
        data.extend_from_slice(&[1, 0, 0, 0]);
        data.extend_from_slice(&0x8000_0000_u32.to_le_bytes());
        data.extend_from_slice(&0x8000_0000_u32.to_le_bytes());
        data.extend_from_slice(&[0; 12]);
    }
    let data = Bytes::from(data);
    let mut limits = Limits::default();
    limits.max_decode_work = 3;
    let meter = Meter::for_input(&limits, Arc::new(Unbounded), Reservation::Input);
    let mut reader = Reader::new(&data, Endian::Little);
    let error = parse_blob(&mut reader, &data, 18, &meter, &limits, None).unwrap_err();
    assert!(error.to_string().contains("DecodeWork"), "{error}");
    assert_eq!(meter.decoded(), 3);
    assert_eq!(meter.used(), 0);
    assert!(
        meter.granted() < 10000,
        "allocated the whole row table before checking work"
    );
}
