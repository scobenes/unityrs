//! Constructor-supplied node instances retain arbitrary levels and aliases.
#![allow(clippy::unwrap_used)]

use serde_json::{Value, json};
use std::sync::Arc;
use unityrs_core::RawStr;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::typetree::{NodeDict, NodeId, NodeList, NodeRow, TypeTreeNode};

fn row(v: &Value) -> NodeRow {
    NodeRow {
        level: i32::try_from(v["m_Level"].as_i64().unwrap()).unwrap(),
        ty: RawStr::from(v["m_Type"].as_str().unwrap()),
        name: RawStr::from(v["m_Name"].as_str().unwrap()),
        byte_size: i32::try_from(v["m_ByteSize"].as_i64().unwrap()).unwrap(),
        version: i32::try_from(v["m_Version"].as_i64().unwrap()).unwrap(),
        ..NodeRow::default()
    }
}

fn snapshot(list: &NodeList) -> Value {
    json!({"rows": list.rows().iter().map(|r| json!([
        r.level, r.ty.to_str(), r.name.to_str(), r.byte_size, r.version,
        r.type_flags, r.variable_count, r.index, r.meta_flag, r.ref_type_hash
    ])).collect::<Vec<_>>(), "children": (0..list.len()).map(|i| {
        list.children(NodeId(u32::try_from(i).unwrap())).iter().map(|id| id.index()).collect::<Vec<_>>()
    }).collect::<Vec<_>>()})
}

#[test]
fn supplied_children_preserve_indirect_shared_identities_when_relinked() {
    let f: Value = serde_json::from_str(include_str!("fixtures/providers/api.json")).unwrap();
    let c = &f["shared_children"];
    let meter = Meter::unlimited();
    let mut supplied = Vec::new();
    for node in c["nodes"].as_array().unwrap() {
        let children: Vec<_> = node["m_Children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| Arc::clone(&supplied[usize::try_from(i.as_u64().unwrap()).unwrap()]))
            .collect();
        supplied.push(Arc::new(
            TypeTreeNode::new(&row(node), &children, &meter).unwrap(),
        ));
    }
    let root = &c["root"];
    let dict = NodeDict {
        level: Some(i32::try_from(root["m_Level"].as_i64().unwrap()).unwrap()),
        ty: Some(RawStr::from(root["m_Type"].as_str().unwrap())),
        name: Some(RawStr::from(root["m_Name"].as_str().unwrap())),
        children: root["m_Children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| Arc::clone(&supplied[usize::try_from(i.as_u64().unwrap()).unwrap()]))
            .collect(),
        ..NodeDict::default()
    };
    let mut graph = TypeTreeNode::from_list(&[dict], &meter).unwrap();
    assert_eq!(
        snapshot(&graph.node_list(&Limits::default()).unwrap()),
        c["initial"]
    );
    graph.relink().unwrap();
    assert_eq!(
        snapshot(&graph.node_list(&Limits::default()).unwrap()),
        c["relinked"]
    );
    drop((graph, supplied));
    assert_eq!(meter.used(), 0);
}
