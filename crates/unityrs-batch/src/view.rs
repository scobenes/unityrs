//! Views (`docs/BATCH_SPEC.md` 4.1, 4.2; `docs/plans/batch.md` 2.3).
//!
//! A bundle's index is its place in [`crate::spec::model::Spec::bundles`],
//! `inputs` first. The [`Graph`] holds the id table in bytewise order (the
//! order bundles take wherever they are ordered, 4.1) and each bundle's
//! `deps` as indices. `view(X)` is X and the closure of its `deps` through
//! the graph (4.2); a [`Walker`] walks it iteratively, so no spec can
//! deepen the stack, and stops once it holds `max_view_bundles + 1`
//! bundles or has read its budget of ids, so the `view_size` rule reads at
//! most 2^24 + `max_view_bundles` ids (`BATCH_SPEC` 1.1, `batch.md` 2.3
//! rule 4).

use std::cmp::Ordering;
use std::hash::{BuildHasher, RandomState};

use crate::spec::model::Spec;
use crate::spec::store::sort_by_bytes;

/// The `deps` graph of a validated spec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Graph {
    /// Every bundle's index, in bytewise order of its id.
    order: Vec<u32>,
    /// Each bundle's place in `order`.
    rank: Vec<u32>,
    /// Bundle `i`'s deps are `targets[offsets[i]..offsets[i + 1]]`.
    offsets: Vec<u32>,
    /// The deps lists, as indices, each in its list's order.
    targets: Vec<u32>,
    /// How many of the bundles are inputs (indices `0..inputs`).
    inputs: u32,
}

/// Every bundle's index in bytewise order of its id, ties (a repeated id)
/// by index.
#[must_use]
pub fn id_order(spec: &Spec) -> Vec<u32> {
    // Exactly sized (a 64 MiB document holds fewer than 2^32 bundles).
    let n = u32::try_from(spec.bundle_count()).unwrap_or(u32::MAX);
    sort_by_bytes(n, |i| {
        spec.bundle(i as usize)
            .map_or(&b""[..], |b| b.id().as_bytes())
    })
}

/// The bundles by id, for `deps`' lookups: an open-addressed table of
/// indices + 1 (0 empty) by a keyed hash of the id, so no spec can choose
/// ids that collide (`RandomState`'s `SipHash`). A lookup reads one slot run
/// and the ids it holds, where a binary search of [`id_order`] reads about
/// 21 ids far apart on a spec of 2M bundles.
pub(crate) struct IdIndex {
    slots: Vec<u32>,
    hasher: RandomState,
}

impl IdIndex {
    /// The table of `spec`'s bundles, their ids unique; at most three
    /// quarters full.
    pub(crate) fn new(spec: &Spec) -> Self {
        Self::over(spec, spec.bundle_count())
    }

    /// The table of `spec`'s first `n` bundles (its inputs, for `prefer`'s
    /// `input`: an id not among them is not found).
    pub(crate) fn over(spec: &Spec, n: usize) -> Self {
        let mut size = n.next_power_of_two().max(16);
        if n * 4 > size * 3 {
            size *= 2;
        }
        let mut t = Self {
            slots: vec![0; size],
            hasher: RandomState::new(),
        };
        for i in 0..n {
            let Some(b) = spec.bundle(i) else { continue };
            let mut at = t.home(b.id());
            while t.slots.get(at).is_some_and(|&s| s != 0) {
                at = (at + 1) & (size - 1);
            }
            if let Some(slot) = t.slots.get_mut(at) {
                *slot = u32::try_from(i + 1).unwrap_or(u32::MAX);
            }
        }
        t
    }

    fn home(&self, id: &str) -> usize {
        // Truncation keeps the low bits, which the mask takes.
        #[allow(clippy::cast_possible_truncation)]
        let h = self.hasher.hash_one(id) as usize;
        h & (self.slots.len() - 1)
    }

    /// The index of the bundle whose id is `id`.
    pub(crate) fn find(&self, spec: &Spec, id: &str) -> Option<u32> {
        let mask = self.slots.len() - 1;
        let mut at = self.home(id);
        loop {
            let i = self.slots.get(at)?.checked_sub(1)?;
            if spec.bundle(i as usize).is_some_and(|b| b.id() == id) {
                return Some(i);
            }
            at = (at + 1) & mask;
        }
    }
}

impl Graph {
    /// A graph over `order` (from [`id_order`], its ids unique) whose
    /// bundle `i` has the deps `targets[offsets[i]..offsets[i + 1]]`.
    pub(crate) fn new(order: Vec<u32>, offsets: Vec<u32>, targets: Vec<u32>, inputs: u32) -> Self {
        let mut rank = vec![0u32; order.len()];
        for (r, &i) in order.iter().enumerate() {
            if let (Some(slot), Ok(r)) = (rank.get_mut(i as usize), u32::try_from(r)) {
                *slot = r;
            }
        }
        Self {
            order,
            rank,
            offsets,
            targets,
            inputs,
        }
    }

    /// How many bundles the spec declares.
    #[must_use]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Whether the spec declares no bundle (never, once validated).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// How many bundles are inputs: indices `0..inputs()`.
    #[must_use]
    pub fn inputs(&self) -> u32 {
        self.inputs
    }

    /// Every bundle's index in bytewise order of its id.
    #[must_use]
    pub fn order(&self) -> &[u32] {
        &self.order
    }

    /// Bundle `i`'s place in [`Self::order`].
    #[must_use]
    pub fn rank(&self, i: u32) -> Option<u32> {
        self.rank.get(i as usize).copied()
    }

    /// Bundle `i`'s `deps`, as indices, in its list's order.
    #[must_use]
    pub fn deps(&self, i: u32) -> &[u32] {
        let i = i as usize;
        match (self.offsets.get(i), self.offsets.get(i + 1)) {
            (Some(&a), Some(&b)) => self.targets.get(a as usize..b as usize).unwrap_or_default(),
            _ => &[],
        }
    }

    /// The index of the bundle whose id is `id`.
    #[must_use]
    pub fn find(&self, spec: &Spec, id: &str) -> Option<u32> {
        find(&self.order, spec, id)
    }
}

/// The index of the bundle whose id is `id`, by binary search of `order`.
pub(crate) fn find(order: &[u32], spec: &Spec, id: &str) -> Option<u32> {
    order
        .binary_search_by(|&i| match spec.bundle(i as usize) {
            Some(b) => b.id().cmp(id),
            None => Ordering::Less,
        })
        .ok()
        .and_then(|at| order.get(at).copied())
}

/// A view past its bound: it holds more than `max_view_bundles` bundles,
/// or its walk passed its budget of ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewTooLarge;

/// Walks views, its buffers kept between walks: a stamp per bundle (a
/// walk's generation marks what it reached), the walk's stack and the
/// view's members.
#[derive(Debug)]
pub struct Walker {
    stamp: Vec<u32>,
    generation: u32,
    stack: Vec<u32>,
    members: Vec<u32>,
}

impl Walker {
    /// A walker for a graph of `bundles` bundles.
    #[must_use]
    pub fn new(bundles: usize) -> Self {
        Self::with_capacity(bundles, 0)
    }

    /// Phase-1 controller scratch: allocate the bounded walk buffers now so
    /// their retained capacity is inside the measurement, not the first walk.
    #[must_use]
    pub fn with_capacity(bundles: usize, max_view: usize) -> Self {
        Self {
            stamp: vec![0; bundles],
            generation: 0,
            stack: Vec::with_capacity(max_view),
            members: Vec::with_capacity(max_view),
        }
    }

    fn next_generation(&mut self) {
        if self.generation == u32::MAX {
            self.stamp.fill(0);
            self.generation = 0;
        }
        self.generation += 1;
    }

    /// Marks `i` reached; false when it already was.
    fn mark(&mut self, i: u32) -> bool {
        match self.stamp.get_mut(i as usize) {
            Some(s) if *s != self.generation => {
                *s = self.generation;
                true
            }
            _ => false,
        }
    }

    /// `view(x)`'s members, `x` first, then in the walk's order; or
    /// [`ViewTooLarge`] once the walk reaches `max + 1` members, where it
    /// stops.
    pub fn view(&mut self, graph: &Graph, x: u32, max: u32) -> Result<&[u32], ViewTooLarge> {
        let mut work = u64::MAX;
        self.view_within(graph, x, max, &mut work)
    }

    /// [`Self::view`], reading each member's `deps` only while `work`
    /// holds as many ids, which it takes from `work`: [`ViewTooLarge`]
    /// once a member's list is longer than what is left.
    pub fn view_within(
        &mut self,
        graph: &Graph,
        x: u32,
        max: u32,
        work: &mut u64,
    ) -> Result<&[u32], ViewTooLarge> {
        self.next_generation();
        self.stack.clear();
        self.members.clear();
        self.mark(x);
        self.members.push(x);
        self.stack.push(x);
        let max = max as usize;
        while let Some(v) = self.stack.pop() {
            let deps = graph.deps(v);
            *work = work.checked_sub(deps.len() as u64).ok_or(ViewTooLarge)?;
            for &t in deps {
                if self.mark(t) {
                    if self.members.len() >= max {
                        return Err(ViewTooLarge);
                    }
                    self.members.push(t);
                    self.stack.push(t);
                }
            }
        }
        Ok(&self.members)
    }

    /// `view(x)`'s members in bytewise order of their ids (registration's
    /// order, `batch.md` 2.3 rule 1), or [`ViewTooLarge`].
    pub fn view_in_order(
        &mut self,
        graph: &Graph,
        x: u32,
        max: u32,
    ) -> Result<Vec<u32>, ViewTooLarge> {
        let mut v = self.view(graph, x, max)?.to_vec();
        v.sort_by_key(|&i| graph.rank(i));
        Ok(v)
    }

    /// Marks each of `ids` in a new generation: the first already marked,
    /// a repeat, by its place, or `None`.
    pub(crate) fn first_repeat(&mut self, ids: &[u32]) -> Option<usize> {
        self.next_generation();
        ids.iter().position(|&i| !self.mark(i))
    }
}

/// Worker-local visited indices, bounded by the view cap rather than the
/// run's bundle count. The sorted index set avoids a full graph stamp table.
/// Registration receives bytewise member order, with X excluded (X is last).
pub struct BoundedWalker {
    visited: Vec<u32>,
    stack: Vec<u32>,
    members: Vec<u32>,
}
impl BoundedWalker {
    #[must_use]
    pub fn new(max: usize) -> Self {
        Self {
            visited: Vec::with_capacity(max),
            stack: Vec::with_capacity(max),
            members: Vec::with_capacity(max),
        }
    }
    pub fn try_new(max: usize) -> std::io::Result<Self> {
        let reserve = || {
            let mut values = Vec::new();
            values
                .try_reserve_exact(max)
                .map_err(|_| std::io::ErrorKind::OutOfMemory)?;
            Ok::<_, std::io::Error>(values)
        };
        Ok(Self {
            visited: reserve()?,
            stack: reserve()?,
            members: reserve()?,
        })
    }
    pub fn view(&mut self, graph: &Graph, x: u32, max: u32) -> Result<&[u32], ViewTooLarge> {
        self.visited.clear();
        self.stack.clear();
        self.members.clear();
        if max == 0 || x as usize >= graph.len() {
            return Err(ViewTooLarge);
        }
        self.visited.push(x);
        self.stack.push(x);
        while let Some(i) = self.stack.pop() {
            for &j in graph.deps(i) {
                if let Err(at) = self.visited.binary_search(&j) {
                    if self.visited.len() >= max as usize {
                        return Err(ViewTooLarge);
                    }
                    self.visited.insert(at, j);
                    self.stack.push(j);
                    self.members.push(j);
                }
            }
        }
        self.members.sort_unstable_by_key(|&i| graph.rank(i));
        Ok(&self.members)
    }
}

#[cfg(test)]
mod tests {
    use super::{Graph, ViewTooLarge, Walker};

    /// Bundles 0..n in id order, with `lists`.
    fn graph(lists: &[Vec<u32>]) -> Graph {
        let n = u32::try_from(lists.len()).unwrap();
        let mut offsets = vec![0];
        let mut targets = Vec::new();
        for l in lists {
            targets.extend_from_slice(l);
            offsets.push(u32::try_from(targets.len()).unwrap());
        }
        Graph::new((0..n).collect(), offsets, targets, 1)
    }

    #[test]
    fn a_view_is_the_closure_of_deps() {
        let g = graph(&[vec![1], vec![2, 0], vec![2], vec![0]]);
        let mut w = Walker::new(g.len());
        assert_eq!(w.view(&g, 0, 10).unwrap(), [0, 1, 2]);
        assert_eq!(w.view(&g, 3, 10).unwrap(), [3, 0, 1, 2]);
        assert_eq!(w.view_in_order(&g, 3, 10).unwrap(), [0, 1, 2, 3]);
        assert_eq!(w.view(&g, 2, 1).unwrap(), [2]);
    }

    #[test]
    fn the_walk_stops_at_max_plus_one() {
        // A chain 0 -> 1 -> ... -> 9.
        let lists: Vec<Vec<u32>> = (0..10u32)
            .map(|i| if i < 9 { vec![i + 1] } else { vec![] })
            .collect();
        let g = graph(&lists);
        let mut w = Walker::new(g.len());
        assert_eq!(w.view(&g, 0, 10).unwrap().len(), 10);
        assert_eq!(w.view(&g, 0, 9), Err(ViewTooLarge));
        assert_eq!(w.members.len(), 9, "the walk stopped at the tenth");
        assert_eq!(w.view(&g, 5, 5).map(<[u32]>::len), Ok(5));
    }

    #[test]
    fn the_walk_stops_past_its_work() {
        // 0 -> 1, 2; 1 -> 2; 2 -> nothing: view(0) reads 3 ids.
        let g = graph(&[vec![1, 2], vec![2], vec![]]);
        let mut w = Walker::new(g.len());
        let mut work = 3;
        assert_eq!(w.view_within(&g, 0, 10, &mut work).unwrap(), [0, 1, 2]);
        assert_eq!(work, 0);
        let mut work = 2;
        assert_eq!(w.view_within(&g, 0, 10, &mut work), Err(ViewTooLarge));
    }

    #[test]
    fn generations_wrap() {
        let g = graph(&[vec![1], vec![]]);
        let mut w = Walker::new(g.len());
        w.generation = u32::MAX - 1;
        for _ in 0..4 {
            assert_eq!(w.view(&g, 0, 5).unwrap(), [0, 1]);
        }
        assert_eq!(w.first_repeat(&[0, 1, 0]), Some(2));
        assert_eq!(w.first_repeat(&[0, 1]), None);
    }
}
