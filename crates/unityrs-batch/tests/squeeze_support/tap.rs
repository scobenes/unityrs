//! The engine's test tap (`unityrs_batch::work::tap`, `test-hooks`) read
//! into a run's records, for X13's reference model: each row's export-work
//! increments (those counted since the row before it was appended), each
//! skip row's phase (the plan's or the export's) and an export skip's
//! anchor (the rows written when the cut recorded it, its place in export
//! order), and the increments no row follows.

use std::cell::RefCell;

use serde_json::{Value as J, json};
use unityrs_batch::work::tap::{self, Event};

thread_local! {
    static EVENTS: RefCell<Vec<Event>> = RefCell::new(Vec::with_capacity(1 << 16));
}

fn record(e: Event) {
    EVENTS.with(|v| v.borrow_mut().push(e));
}

/// Runs `f` with this thread's tap on; its events.
pub fn tapped<T>(f: impl FnOnce() -> T) -> (T, Vec<Event>) {
    EVENTS.with(|v| v.borrow_mut().clear());
    tap::set(Some(record));
    let out = f();
    tap::set(None);
    (out, EVENTS.with(|v| std::mem::take(&mut *v.borrow_mut())))
}

/// `rows` (the summary last, when there is one) annotated from `events`:
/// `work` on every row but the summary, `phase` on a skip row and `anchor`
/// on an export skip; and the increments no row follows. A records flush
/// is not an output's work: the model counts the records file from its
/// length.
pub fn annotate(rows: &mut [J], events: &[Event]) -> Vec<J> {
    let mut pending = Vec::new();
    let mut work = Vec::new();
    let mut pushes = Vec::new();
    for e in events {
        match *e {
            Event::Pixels(n) => pending.push(json!(["pixels", n])),
            Event::File(n) => pending.push(json!(["file", n])),
            Event::Member(n) => pending.push(json!(["member", n])),
            Event::Records(_) => {}
            Event::Record => work.push(std::mem::take(&mut pending)),
            Event::Skip(exporting) => {
                pushes.push((exporting, work.len(), std::mem::take(&mut pending)));
            }
        }
    }
    let mut pushes = pushes.into_iter();
    for (i, row) in rows.iter_mut().enumerate() {
        if row["type"] == "input_summary" {
            continue;
        }
        row["work"] = J::from(work.get(i).cloned().unwrap_or_default());
        if row["type"] == "skip"
            && let Some((exporting, anchor, w)) = pushes.next()
        {
            row["work"] = J::from(w);
            row["phase"] = J::from(if exporting { "export" } else { "plan" });
            if exporting {
                row["anchor"] = J::from(anchor);
            }
        }
    }
    pending
}
