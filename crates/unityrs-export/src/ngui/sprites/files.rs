//! A SerializedFile's identity in an attempt, for 4.4's sprite groups and
//! the plan's caches (B3's review rounds 1 and 2, R4 and S2): its row chain
//! from the view, the top-level row that holds it (by the name the core
//! opened it under, one per bundle of the view, 4.3) and, inside it, the
//! directory row each container down to the file's own was opened from
//! (the core's `ParentKind::Bundle`'s `dir_index`, never a search of
//! `BundleFile::files()`, which names only the last row of a path). Two
//! files of one CAB name, one nested and one at the top level, or each at
//! one of two rows of one path, are two files; one file reached twice, even
//! reopened after an eviction (2.6 step 7), is one.
//!
//! Each identity is interned once per attempt as a small number ([`Files`]):
//! a top-level row's name, and each (container, row) pair, charged to `M_X`
//! when first met, so a Sprite costs a number, not a copy of a name. A
//! file's number is found again by its address, which the table pins with a
//! weak reference (so the address is not reused while the entry lives),
//! charged as the whole allocation it keeps; the walk up its containers
//! runs once per file opened, one unit of decode work and one table lookup
//! a level. A container gone before its child (a broken chain) is the
//! run's `internal`, as `deref`'s `ContainerDropped` is. A name's or a
//! pin's charge is kept only once its entry is in ([`Map::insert_held`]:
//! B3's review round 3, S7).

use std::sync::{Arc, Weak};

use unityrs_core::files::{ParentKind, SerializedFile};
use unityrs_core::limits::{Charge, Meter, MeteredVec, heap_cost};

use super::super::classify::{self, Class, Reason};
use super::super::table::{self, Map};

fn internal() -> Class {
    Class::Run(Reason::plain("internal"))
}

/// What a retained pin keeps: the file's whole `Arc` allocation (its two
/// counts and the file), as the core charges a file.
pub(super) fn pin_cost() -> u64 {
    heap_cost(u64::try_from(size_of::<SerializedFile>() + 16).unwrap_or(u64::MAX))
}

/// The attempt's file identities.
pub(super) struct Files {
    tops: Map<Vec<u8>, u32>,
    rows: Map<(u32, usize), u32>,
    seen: Map<usize, (Weak<SerializedFile>, u32)>,
    held: Charge,
    next: u32,
}

impl Files {
    /// None yet.
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            tops: Map::new(meter),
            rows: Map::new(meter),
            seen: Map::new(meter),
            held: Charge::zero(meter),
            next: 0,
        }
    }

    fn fresh(&mut self) -> Result<u32, Class> {
        let id = self.next;
        self.next = id.checked_add(1).ok_or_else(internal)?;
        Ok(id)
    }

    /// `file`'s number.
    pub fn id(&mut self, file: &Arc<SerializedFile>, meter: &Arc<Meter>) -> Result<u32, Class> {
        let at = Arc::as_ptr(file) as usize;
        if let Some((_, id)) = self.seen.get(&at) {
            return Ok(*id);
        }
        // The rows from the file's up to the top-level container's, and the
        // name of the top-level row.
        let mut rows: MeteredVec<usize> = MeteredVec::new(meter);
        let mut top = file.name().clone();
        let mut parent = file.parent().cloned();
        while let Some(p) = parent {
            meter.count_decode(1).map_err(|e| classify::charge(&e))?;
            let ParentKind::Bundle {
                bundle, dir_index, ..
            } = &p.kind
            else {
                // No WebFile's entries are registered (4.3), so no pointer
                // reaches a file of one.
                return Err(internal());
            };
            let b = bundle.upgrade().ok_or_else(internal)?;
            table::push(&mut rows, *dir_index)?;
            top = b.name().clone();
            parent = b.parent().cloned();
        }
        let mut id = self.top(top.as_bytes())?;
        for row in rows.as_slice().iter().rev() {
            id = if let Some(n) = self.rows.get(&(id, *row)) {
                *n
            } else {
                let n = self.fresh()?;
                self.rows.insert((id, *row), n)?;
                n
            };
        }
        drop(rows);
        let pin = (Arc::downgrade(file), id);
        self.seen
            .insert_held(&mut self.held, pin_cost(), || Ok(at), pin)?;
        Ok(id)
    }

    fn top(&mut self, name: &[u8]) -> Result<u32, Class> {
        if let Some(n) = self.tops.get(name) {
            return Ok(*n);
        }
        let n = self.fresh()?;
        let cost = table::string(name.len());
        self.tops
            .insert_held(&mut self.held, cost, || table::copied(name), n)?;
        Ok(n)
    }
}
