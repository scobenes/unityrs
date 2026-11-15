// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/TypeTreeNode.py. See THIRD_PARTY_LICENSES.
// Ported from UnityPy git 417d595 (MIT): UnityPy/files/SerializedFile.py, UnityPy/helpers/TypeTreeNode.py. See THIRD_PARTY_LICENSES.
//! Stability and reverse dependencies share the identity of each child list.
use super::{Arena, Handler, Identity, Marks, Parent, invalid};
use crate::Error;
use crate::limits::{Meter, MeteredVec};
use std::sync::Arc;

impl Arena {
    fn marks(&self, id: Identity) -> Result<&Marks, Error> {
        match id {
            Identity::Node(id) => Ok(&self.entry(id)?.marks),
            Identity::List(id) => Ok(&self.list(id)?.marks),
        }
    }

    fn marks_mut(&mut self, id: Identity) -> Result<&mut Marks, Error> {
        match id {
            Identity::Node(id) => Ok(&mut self.entry_mut(id)?.marks),
            Identity::List(id) => self
                .lists
                .as_mut_slice()
                .get_mut(id)
                .map(|list| &mut list.marks)
                .ok_or_else(|| invalid("child list index")),
        }
    }

    pub(super) fn mark(&mut self, id: Identity, hashes: Option<usize>) -> Result<(), Error> {
        // Changing an ordinary node/list's proof context has no side effect.
        // Actual frame-context changes are counted by finish_frame; mutations
        // clear these proofs here and must prevent snapshot replay elision.
        if self.marks(id)?.resolved.is_some() && hashes.is_none() {
            self.invalidations = self.invalidations.saturating_add(1);
        }
        self.marks_mut(id)?.resolved = hashes;
        Ok(())
    }

    // Child -> list links are built once. Assigning that list to another node
    // adds one list -> node link, irrespective of its width. Historical links
    // may invalidate extra results, but never miss a shared child's mutation.
    pub(super) fn link(
        &mut self,
        child: Identity,
        owner: Identity,
        meter: &Arc<Meter>,
    ) -> Result<(), Error> {
        meter.count_decode(1)?;
        let next = self.marks(child)?.parents;
        let at = self.parents.len();
        self.parents.push(Parent { owner, next })?;
        self.marks_mut(child)?.parents = Some(at);
        Ok(())
    }

    pub(super) fn invalidate(&mut self, node: usize, meter: &Arc<Meter>) -> Result<(), Error> {
        let mut pending = MeteredVec::new(meter);
        meter.count_decode(1)?;
        pending.push(Identity::Node(node))?;
        let mut first = true;
        while let Some(&id) = pending.as_slice().last() {
            meter.count_decode(1)?;
            pending.truncate(pending.len() - 1);
            let was_clean = self.marks(id)?.resolved.is_some();
            self.mark(id, None)?;
            let propagate = first || was_clean;
            first = false;
            if !propagate {
                continue;
            }
            let mut at = self.marks(id)?.parents;
            while let Some(i) = at {
                meter.count_decode(1)?;
                let parent = self
                    .parents
                    .as_slice()
                    .get(i)
                    .ok_or_else(|| invalid("parent index"))?;
                pending.push(parent.owner)?;
                at = parent.next;
            }
        }
        Ok(())
    }

    pub(super) fn stable_children(
        &mut self,
        node: usize,
        hashes: usize,
        handlers: &[Handler],
        meter: &Arc<Meter>,
    ) -> Result<bool, Error> {
        let root = Identity::List(self.entry(node)?.children);
        let mut pending = MeteredVec::new(meter);
        meter.count_decode(1)?;
        pending.push((root, false))?;
        while let Some(&(id, finish)) = pending.as_slice().last() {
            meter.count_decode(1)?;
            pending.truncate(pending.len() - 1);
            if self.marks(id)?.resolved == Some(hashes) {
                continue;
            }
            match id {
                Identity::Node(node) => {
                    // Only HEAD's frames can mark placeholders. Inspecting an
                    // ordinary subtree here never resolves or mutates a node.
                    if self.row(handlers, node)?.type_flags.unwrap_or(0) & 32 != 0 {
                        continue;
                    }
                    let list = Identity::List(self.entry(node)?.children);
                    if finish {
                        if self.marks(list)?.resolved == Some(hashes) {
                            self.mark(id, Some(hashes))?;
                        }
                    } else {
                        pending.push((id, true))?;
                        pending.push((list, false))?;
                    }
                }
                Identity::List(list) => {
                    if finish {
                        let mut stable = true;
                        for at in self.list(list)?.edges.clone() {
                            meter.count_decode(1)?;
                            stable &= self.entry(self.edge(at)?)?.marks.resolved == Some(hashes);
                        }
                        if stable {
                            self.mark(id, Some(hashes))?;
                        }
                    } else {
                        pending.push((id, true))?;
                        for at in self.list(list)?.edges.clone().rev() {
                            meter.count_decode(1)?;
                            pending.push((Identity::Node(self.edge(at)?), false))?;
                        }
                    }
                }
            }
        }
        Ok(self.marks(root)?.resolved == Some(hashes))
    }
}
