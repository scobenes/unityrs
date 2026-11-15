//! Allocation-free container destruction. The entry tables themselves form
//! the work list, so even a full budget or allocator refusal cannot prevent
//! progress. Their link storage is charged fallibly when the tables grow.
use std::sync::Arc;

use super::File;
use crate::{error::Error, limits::Charge, once::ResultCell};

pub(super) struct FileCell {
    value: ResultCell<File, Error>,
    // Only the first cell is used as a link, and only during destruction.
    next: Option<Pending>,
}

impl FileCell {
    pub(super) fn new() -> Self {
        Self {
            value: ResultCell::new(),
            next: None,
        }
    }
}

impl std::ops::Deref for FileCell {
    type Target = ResultCell<File, Error>;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

pub(super) struct Pending {
    entries: Box<[FileCell]>,
    // Keep the table charged after its container has been deallocated.
    _charge: Charge,
}

fn take(entries: &mut Box<[FileCell]>, charge: &mut Charge) -> Pending {
    let zero = Charge::zero(charge.meter());
    Pending {
        entries: std::mem::take(entries),
        _charge: std::mem::replace(charge, zero),
    }
}

pub(super) fn drain(entries: &mut Box<[FileCell]>, charge: &Charge) {
    // The root's other fields are still live during its Drop. Leave their
    // charge with the root, where it drops last. Descendants transfer their
    // charge to Pending only when their remaining fields are being freed.
    let mut current = Pending {
        entries: std::mem::take(entries),
        _charge: Charge::zero(charge.meter()),
    };
    loop {
        let mut next = current
            .entries
            .first_mut()
            .and_then(|cell| cell.next.take());
        for cell in &mut current.entries {
            let file = match cell.value.take() {
                Some(Ok(file)) => file,
                Some(Err(kept)) => {
                    let (mut error, _charge) = kept.into_parts();
                    // Cached entry failures form a singly linked Arc chain.
                    while let Error::Entry { source, .. } = error {
                        let Some(inner) = Arc::into_inner(source) else {
                            break;
                        };
                        error = inner;
                    }
                    continue;
                }
                None => continue,
            };
            // into_inner also handles simultaneous final releases: exactly
            // one owner takes the graph. Weak parent links do not obstruct it.
            let child =
                match file {
                    File::Bundle(file) => Arc::into_inner(file)
                        .map(|mut file| take(&mut file.entries, &mut file.charge)),
                    File::Web(file) => Arc::into_inner(file)
                        .map(|mut file| take(&mut file.entries, &mut file.charge)),
                    File::Serialized(_) | File::Raw(_) => None,
                };
            if let Some(mut child) = child
                && let Some(first) = child.entries.first_mut()
            {
                first.next = next;
                next = Some(child);
            }
        }
        // All child handles and the link have been removed. Drop one level
        // before advancing, with constant call-stack space and no growth.
        drop(current);
        let Some(pending) = next else { break };
        current = pending;
    }
}
