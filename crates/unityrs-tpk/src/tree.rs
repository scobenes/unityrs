// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/Tpk.py. See THIRD_PARTY_LICENSES.
//! `generate_node` (`helpers/Tpk.py:58-88`): a class's tree, walked from its
//! release root over the node buffer, as the rows UnityPy builds before
//! `TypeTreeNode.from_list`.

use std::sync::Arc;

use unityrs_core::limits::{ChargeError, Meter, MeteredVec};
use unityrs_core::typetree::NodeRow;

use crate::blob::TypeTreeBlob;

/// Why a walk stopped.
#[derive(Debug)]
pub(crate) enum WalkError {
    /// A node at `depth` levels (the root's is 1), past the `max` the walk
    /// allows: a level of `max` is a depth of `max + 1`, as unityrs-core's
    /// `Tree::compile` counts.
    Depth { depth: usize, max: usize },
    /// The rows walked and still to walk, past the `max` the walk allows.
    Rows { rows: usize, max: usize },
    /// A node or string id past its table, where UnityPy raises
    /// `IndexError`.
    PastTable(&'static str),
    /// A growth the allocator refused.
    Memory(ChargeError),
}

impl From<ChargeError> for WalkError {
    fn from(e: ChargeError) -> Self {
        Self::Memory(e)
    }
}

/// The walk's level bound as [`generate_node`] applies it: a level is an
/// `i32` (`NodeRow::level`), so no bound is above `i32::MAX`.
fn level_bound(max_depth: usize) -> usize {
    max_depth.min(usize::try_from(i32::MAX).unwrap_or(usize::MAX))
}

/// What [`generate_node`] makes of a release root under its bounds, found
/// by [`builds`] without walking it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Build {
    /// A tree of this many rows.
    Rows(usize),
    /// An error: a level past the depth bound, rows past the row bound, or
    /// an id past its table, after the walk builds at most this many rows
    /// (the tree's rows, counting an id past its table as one, held at one
    /// past the row bound; the walk stops at or before each of them).
    Fails(usize),
}

impl Build {
    /// The most rows [`generate_node`] builds from the root, whether it
    /// ends with its tree or with an error.
    pub(crate) fn work(self) -> usize {
        match self {
            Self::Rows(rows) | Self::Fails(rows) => rows,
        }
    }
}

/// One node's tree as [`builds`] finds it: its rows (each node counted once
/// per time the walk reaches it) and its height (a leaf's is 1), each held
/// at one past its bound, and whether the walk meets an id past its table.
#[derive(Clone, Copy)]
struct Size {
    rows: usize,
    height: usize,
    bad: bool,
}

/// A node during [`builds`]'s search.
#[derive(Clone, Copy)]
enum Seen {
    New,
    /// On the search's stack: reaching it again is a cycle.
    Open,
    Done(Size),
}

/// A node on [`builds`]'s stack: its id, its next sub-node, and its
/// sub-nodes' sizes so far, summed (rows), maxed (height) and or-ed (bad).
struct Frame {
    node: u16,
    next: usize,
    sum: Size,
}

/// For each of `roots`, what [`generate_node`] with these bounds makes of
/// it, in one pass over the node buffer, every node sized once: a tree's
/// rows are its root's plus its sub-nodes' (each counted as often as it is
/// listed), its height its tallest sub-node's plus one, and a node that is
/// reached again while it is being sized (a cycle) makes every node above
/// it endless. The walk ends with an error exactly where the tree's height
/// passes `max_depth`, its rows pass `max_rows` (the walk counts the rows
/// still to walk, never more than the tree has) or it reaches an id past
/// its table, having built at most the rows counted here; anywhere else it
/// builds the rows counted here. The search
/// keeps a stack, never recursing, of at most one frame per node, and
/// charges its tables to `meter`.
pub(crate) fn builds(
    blob: &TypeTreeBlob,
    roots: &[u16],
    max_depth: usize,
    max_rows: usize,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<Build>, ChargeError> {
    let max_depth = level_bound(max_depth);
    let (row_cap, height_cap) = (max_rows.saturating_add(1), max_depth.saturating_add(1));
    let endless = Size {
        rows: row_cap,
        height: height_cap,
        bad: false,
    };
    let past_table = Size {
        rows: 1,
        height: 1,
        bad: true,
    };
    let fold = |into: &mut Size, s: Size| {
        into.rows = into.rows.saturating_add(s.rows).min(row_cap);
        into.height = into.height.max(s.height);
        into.bad |= s.bad;
    };
    let strings = blob.strings.len();
    let mut seen = MeteredVec::with_capacity(meter, blob.nodes.len())?;
    for _ in 0..blob.nodes.len() {
        seen.push(Seen::New)?;
    }
    let mut stack: MeteredVec<Frame> = MeteredVec::new(meter);
    let mut out = MeteredVec::with_capacity(meter, roots.len())?;
    for &root in roots {
        let size = match seen.as_slice().get(usize::from(root)) {
            None => past_table,
            Some(Seen::Done(size)) => *size,
            Some(Seen::New | Seen::Open) => {
                let empty = Size {
                    rows: 0,
                    height: 0,
                    bad: false,
                };
                if let Some(s) = seen.as_mut_slice().get_mut(usize::from(root)) {
                    *s = Seen::Open;
                }
                stack.push(Frame {
                    node: root,
                    next: 0,
                    sum: empty,
                })?;
                let mut found = past_table;
                while let Some(top) = stack.as_mut_slice().last_mut() {
                    let node = blob.nodes.get(top.node);
                    let subs = node.map_or(&[][..], |n| blob.nodes.sub_nodes(n));
                    if let Some(&sub) = subs.get(top.next) {
                        top.next += 1;
                        match seen.as_slice().get(usize::from(sub)) {
                            None => fold(&mut top.sum, past_table),
                            Some(Seen::Done(size)) => fold(&mut top.sum, *size),
                            Some(Seen::Open) => fold(&mut top.sum, endless),
                            Some(Seen::New) => {
                                if let Some(s) = seen.as_mut_slice().get_mut(usize::from(sub)) {
                                    *s = Seen::Open;
                                }
                                stack.push(Frame {
                                    node: sub,
                                    next: 0,
                                    sum: empty,
                                })?;
                            }
                        }
                        continue;
                    }
                    // Every sub-node sized: this node's own row, level and
                    // strings.
                    let named = node.is_some_and(|n| {
                        usize::from(n.type_name) < strings && usize::from(n.name) < strings
                    });
                    let size = Size {
                        rows: top.sum.rows.saturating_add(1).min(row_cap),
                        height: top.sum.height.saturating_add(1).min(height_cap),
                        bad: top.sum.bad || !named,
                    };
                    let id = top.node;
                    stack.truncate(stack.len().saturating_sub(1));
                    if let Some(s) = seen.as_mut_slice().get_mut(usize::from(id)) {
                        *s = Seen::Done(size);
                    }
                    match stack.as_mut_slice().last_mut() {
                        Some(parent) => fold(&mut parent.sum, size),
                        None => found = size,
                    }
                }
                found
            }
        };
        out.push(
            if size.bad || size.rows > max_rows || size.height > max_depth {
                Build::Fails(size.rows)
            } else {
                Build::Rows(size.rows)
            },
        )?;
    }
    Ok(out)
}

/// `generate_node(class_info)` from `root`, its release root (`Tpk.py:66-85`):
/// every node in pre-order (`stack.pop(0)`, its sub-nodes put in front, in
/// their order), each row with its level (its parent's plus one), its
/// position as `m_Index`, its byte size, version and meta flag (unsigned, as
/// stored), its type and name from the string buffer, and its type flags,
/// variable count and ref-type hash unset (`TypeTreeNode`'s keywords there).
///
/// UnityPy's walk has no bound; this one stops at a level of `max_depth` or
/// past `max_rows` rows, counting those still to walk, since each becomes a
/// row (crate docs, "Limits"). So a node that lists itself or an ancestor,
/// on which UnityPy's walk never ends, is an error. The root itself is a
/// depth of 1 and a row, so with `max_depth` or `max_rows` at 0 every walk
/// is an error before it builds anything, as [`builds`] finds. Every row and
/// the pending stack are charged to `meter`.
pub(crate) fn generate_node(
    blob: &TypeTreeBlob,
    root: u16,
    max_depth: usize,
    max_rows: usize,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<NodeRow>, WalkError> {
    walk(blob, root, max_depth, max_rows, meter, &mut 0)
}

/// [`generate_node`], counting in `made` the rows it builds, whether it
/// ends with its tree or with an error.
fn walk(
    blob: &TypeTreeBlob,
    root: u16,
    max_depth: usize,
    max_rows: usize,
    meter: &Arc<Meter>,
    made: &mut usize,
) -> Result<MeteredVec<NodeRow>, WalkError> {
    // A level is an `i32` (`NodeRow::level`); every one is below this.
    let max_depth = level_bound(max_depth);
    let string = |id: u16| {
        blob.strings
            .as_slice()
            .get(usize::from(id))
            .cloned()
            .ok_or(WalkError::PastTable(
                "IndexError: a string id past the string buffer",
            ))
    };
    // The root's own depth and row, which every later check counts from.
    if max_depth == 0 {
        return Err(WalkError::Depth { depth: 1, max: 0 });
    }
    if max_rows == 0 {
        return Err(WalkError::Rows { rows: 1, max: 0 });
    }
    let mut nodes = MeteredVec::new(meter);
    // `stack = [(class_info.ReleaseRootNode, 0)]`, its front at the end.
    let mut stack = MeteredVec::new(meter);
    stack.push((root, 0_usize))?;
    while let Some(&(node_id, level)) = stack.as_slice().last() {
        // `node_id, level = stack.pop(0)`.
        stack.truncate(stack.len().saturating_sub(1));
        // `node: TpkUnityNode = NODES[node_id]`.
        let node = blob.nodes.get(node_id).ok_or(WalkError::PastTable(
            "IndexError: a node id past the node buffer",
        ))?;
        let row = NodeRow {
            // Below `max_depth`, which is at most `i32::MAX`.
            level: i32::try_from(level).unwrap_or(i32::MAX),
            ty: string(node.type_name)?,
            name: string(node.name)?,
            byte_size: node.byte_size,
            version: i32::from(node.version),
            type_flags: None,
            variable_count: None,
            // `m_Index=index`: below `max_rows`.
            index: Some(i32::try_from(nodes.len()).unwrap_or(i32::MAX)),
            // UnityPy keeps the stored `u32` (`Tpk.py:361`), zero-extended.
            meta_flag: Some(i64::from(node.meta_flag)),
            ref_type_hash: None,
        };
        nodes.push(row)?;
        *made = nodes.len();
        let subs = blob.nodes.sub_nodes(node);
        if subs.is_empty() {
            continue;
        }
        // Each bound is its own error. A sub-node's depth, counting the
        // root as 1, is its level plus one.
        if level + 1 >= max_depth {
            return Err(WalkError::Depth {
                depth: level + 2,
                max: max_depth,
            });
        }
        let held = nodes.len() + stack.len() + subs.len();
        if held > max_rows {
            return Err(WalkError::Rows {
                rows: held,
                max: max_rows,
            });
        }
        // `stack = [(node_id, level + 1) for node_id in node.SubNodes] + stack`.
        for &id in subs.iter().rev() {
            stack.push((id, level + 1))?;
        }
    }
    Ok(nodes)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use unityrs_core::limits::Meter;

    use super::{Build, builds, walk};
    use crate::blob::TypeTreeBlob;

    /// A blob of no versions, classes or common strings, over `nodes` (each
    /// its type and name string ids and its sub-node ids) and `strings`
    /// strings.
    fn blob(nodes: &[(u16, u16, Vec<u16>)], strings: usize, meter: &Arc<Meter>) -> TypeTreeBlob {
        let mut out = 1_i64.to_le_bytes().to_vec();
        for _ in 0..4 {
            out.extend_from_slice(&0_i32.to_le_bytes());
        }
        out.extend_from_slice(&i32::try_from(nodes.len()).unwrap().to_le_bytes());
        for (ty, name, subs) in nodes {
            out.extend_from_slice(&ty.to_le_bytes());
            out.extend_from_slice(&name.to_le_bytes());
            out.extend_from_slice(&[0; 7]);
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&u16::try_from(subs.len()).unwrap().to_le_bytes());
            for s in subs {
                out.extend_from_slice(&s.to_le_bytes());
            }
        }
        out.extend_from_slice(&i32::try_from(strings).unwrap().to_le_bytes());
        for _ in 0..strings {
            out.extend_from_slice(&[1, b's']);
        }
        TypeTreeBlob::read(&out, meter).unwrap()
    }

    /// [`builds`] gives, for every root of thousands of random node graphs
    /// (shared sub-nodes, cycles, ids past the node and string tables), and
    /// under tight and loose bounds (a depth or a row bound of 0 and 1
    /// among them), exactly what [`generate_node`] does: the walk's row
    /// count where it builds, and a failure where it fails, never past the
    /// rows the walk built before it failed, nor more than one past the row
    /// bound; asked for every root at once (each node sized once across
    /// them) or for one root alone.
    #[test]
    fn builds_finds_what_the_walk_makes() {
        let meter = Meter::unlimited();
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let (mut built, mut failed) = (0, 0);
        for _ in 0..3_000 {
            let n = 1 + next(9);
            let strings = 1 + next(3);
            let nodes: Vec<(u16, u16, Vec<u16>)> = (0..n)
                .map(|_| {
                    // Mostly in the string table, sometimes one past it.
                    let past = u64::from(next(8) == 0);
                    let ty = u16::try_from(next(strings + past)).unwrap();
                    let past = u64::from(next(8) == 0);
                    let name = u16::try_from(next(strings + past)).unwrap();
                    let subs = (0..next(4))
                        .map(|_| u16::try_from(next(n + 1)).unwrap())
                        .collect();
                    (ty, name, subs)
                })
                .collect();
            let blob = blob(&nodes, usize::try_from(strings).unwrap(), &meter);
            let roots: Vec<u16> = (0..=u16::try_from(n).unwrap()).collect();
            for (depth, rows) in [
                (0, 6),
                (1, 6),
                (2, 0),
                (2, 1),
                (3, 6),
                (5, 40),
                (256, 1 << 16),
            ] {
                let all = builds(&blob, &roots, depth, rows, &meter).unwrap();
                for (&root, &got) in roots.iter().zip(all.as_slice()) {
                    let mut made = 0;
                    let walked = walk(&blob, root, depth, rows, &meter, &mut made);
                    let want = if let Ok(walked) = &walked {
                        built += 1;
                        Build::Rows(walked.len())
                    } else {
                        failed += 1;
                        Build::Fails(got.work())
                    };
                    assert_eq!(
                        got, want,
                        "{nodes:?} root {root}, depth {depth}, rows {rows}"
                    );
                    // A walk, failing or not, builds no more than `builds`
                    // counts for it.
                    assert!(
                        made <= got.work() && got.work() <= rows + 1,
                        "{nodes:?} root {root}, depth {depth}, rows {rows}: {made} rows made, {got:?}"
                    );
                    let alone = builds(&blob, &[root], depth, rows, &meter).unwrap();
                    assert_eq!(alone.as_slice(), [want], "{nodes:?} root {root} alone");
                }
            }
        }
        assert!(
            built > 10_000 && failed > 10_000,
            "{built} built, {failed} failed"
        );
    }
}
