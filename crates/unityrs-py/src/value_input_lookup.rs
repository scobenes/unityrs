// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp (clean_name). See LICENSE-UNITYPY and THIRD_PARTY_LICENSES.
// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/TypeTreeHelper.cpp (clean_name). See THIRD_PARTY_LICENSES.
//! Metered schema-name indexes, reused for every occurrence of a class node.
use crate::errors;
use pyo3::prelude::*;
use std::sync::Arc;
use unityrs_core::{
    RawStr,
    budget::DecodeBuf,
    limits::{Meter, MeteredVec},
    typetree::{NodeId, Tree},
};

struct Table {
    entries: MeteredVec<(RawStr, NodeId)>,
    max_name: usize,
}
pub(super) struct Lookups {
    slots: MeteredVec<Option<usize>>,
    tables: MeteredVec<Table>,
}
impl Lookups {
    pub(super) fn new(tree: Option<&Tree>, meter: &Arc<Meter>) -> PyResult<Self> {
        let count = tree
            .map_or(0, |tree| tree.list().len())
            .checked_mul(2)
            .ok_or_else(|| pyo3::exceptions::PyMemoryError::new_err("schema index size"))?;
        let mut slots = MeteredVec::with_capacity(meter, count).map_err(errors::charge)?;
        meter.count_decode(count as u64).map_err(errors::charge)?;
        for _ in 0..count {
            slots.push(None).map_err(errors::charge)?;
        }
        Ok(Self {
            slots,
            tables: MeteredVec::new(meter),
        })
    }
    pub(super) fn find(
        &mut self,
        tree: &Tree,
        node: NodeId,
        dict: bool,
        name: &[u8],
        meter: &Arc<Meter>,
    ) -> PyResult<Option<NodeId>> {
        let slot = usize::try_from(node.0)?
            .checked_mul(2)
            .and_then(|n| n.checked_add(usize::from(dict)))
            .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("schema node index"))?;
        let old = self.slots.as_slice().get(slot).copied().flatten();
        let index = if let Some(index) = old {
            index
        } else {
            let table = Table::new(tree, node, dict, meter)?;
            let index = self.tables.len();
            self.tables.push(table).map_err(errors::charge)?;
            let entry = self
                .slots
                .as_mut_slice()
                .get_mut(slot)
                .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("schema node index"))?;
            *entry = Some(index);
            index
        };
        let table = self
            .tables
            .as_slice()
            .get(index)
            .ok_or_else(|| pyo3::exceptions::PyValueError::new_err("schema lookup index"))?;
        let comparisons = logarithm(table.entries.len()).saturating_add(2);
        meter
            .count_decode(
                (comparisons as u64).saturating_mul(name.len().max(table.max_name) as u64 + 1),
            )
            .map_err(errors::charge)?;
        let entries = table.entries.as_slice();
        let at = entries.partition_point(|(key, _)| key.as_bytes() < name);
        Ok(entries
            .get(at)
            .filter(|(key, _)| key.as_bytes() == name)
            .map(|(_, node)| *node))
    }
}

fn logarithm(count: usize) -> usize {
    usize::try_from(count.saturating_sub(1).bit_width()).unwrap_or(usize::MAX)
}
impl Table {
    fn new(tree: &Tree, node: NodeId, dict: bool, meter: &Arc<Meter>) -> PyResult<Self> {
        let children = tree.list().children(node);
        let mut entries =
            MeteredVec::with_capacity(meter, children.len()).map_err(errors::charge)?;
        meter
            .count_decode(children.len() as u64)
            .map_err(errors::charge)?;
        let mut max_name = 0;
        for &child in children {
            let raw = tree.list().name(child);
            meter
                .count_decode(raw.len() as u64)
                .map_err(errors::charge)?;
            let name = if dict {
                RawStr::charged(raw, meter).map_err(errors::charge)?
            } else {
                clean(raw, meter)?
            };
            max_name = max_name.max(name.as_bytes().len());
            entries.push((name, child)).map_err(errors::charge)?;
        }
        // sort_unstable allocates no scratch. Charge its worst-case comparison
        // work up front, including a full comparison of each longest key.
        let work = (entries.len() as u64)
            .saturating_mul(logarithm(entries.len()).saturating_add(1) as u64)
            .saturating_mul(4)
            .saturating_mul(max_name as u64 + 1);
        meter.count_decode(work).map_err(errors::charge)?;
        entries
            .as_mut_slice()
            .sort_unstable_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()).then(a.1.0.cmp(&b.1.0)));
        Ok(Self { entries, max_name })
    }
}

// UnityPyBoost/TypeTreeHelper.cpp:28-65, with metered, unbounded name storage.
pub(super) fn clean(mut raw: &[u8], meter: &Arc<Meter>) -> PyResult<RawStr> {
    raw = raw.split(|b| *b == 0).next().unwrap_or_default();
    raw = raw.strip_prefix(b"(int&)").unwrap_or(raw);
    raw = raw.strip_suffix(b"?").unwrap_or(raw);
    let mut out = DecodeBuf::new(meter, 0, u64::MAX, unityrs_core::LimitKind::InputMemory)
        .map_err(errors::charge)?;
    if raw.first().is_some_and(u8::is_ascii_digit) {
        out.extend_from_slice(b"x").map_err(errors::charge)?;
    }
    for &byte in raw {
        let byte = if matches!(byte, b' ' | b'.' | b':' | b'-' | b'[' | b']') {
            b'_'
        } else {
            byte
        };
        out.extend_from_slice(&[byte]).map_err(errors::charge)?;
    }
    if matches!(raw, b"pass" | b"from") {
        out.extend_from_slice(b"_").map_err(errors::charge)?;
    }
    Ok(RawStr::from(out.into_bytes().map_err(errors::charge)?))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::Lookups;
    use unityrs_core::{
        limits::{Limits, Meter},
        typetree::{NodeId, NodeList, NodeRow, Tree},
    };

    #[test]
    fn repeated_wide_schema_lookups_reuse_the_charged_sorted_table() {
        let mut rows = vec![NodeRow::new(0, "Synthetic", "Base")];
        rows.extend(
            (0..512).map(|index| NodeRow::new(1, "int", format!("field.{index}").into_bytes())),
        );
        let tree = Tree::compile(
            NodeList::trusted(rows).unwrap().into_shared().unwrap(),
            &Limits::default(),
        )
        .unwrap();
        let meter = Meter::unlimited();
        let mut tables = Lookups::new(Some(&tree), &meter).unwrap();
        assert_eq!(
            tables
                .find(&tree, NodeId(0), false, b"field_511", &meter)
                .unwrap(),
            Some(NodeId(512))
        );
        let retained = meter.used();
        let work = meter.decoded();
        for _ in 0..1000 {
            assert_eq!(
                tables
                    .find(&tree, NodeId(0), false, b"field_511", &meter)
                    .unwrap(),
                Some(NodeId(512))
            );
        }
        assert_eq!(meter.used(), retained);
        assert!(meter.decoded() - work < 200_000);
        drop(tables);
        assert_eq!(meter.used(), 0);
    }
}
