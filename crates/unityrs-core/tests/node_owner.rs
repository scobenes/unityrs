//! The shared list owner is charged at its actual allocation size on every tier.
use std::cell::Cell;
use std::sync::Arc;

use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::limits::{Limits, Meter, heap_cost};
use unityrs_core::stream::{Endian, Reader};
use unityrs_core::typetree::{NodeList, NodeRow, parse::parse_blob};
use unityrs_core::{Bytes, Error, LimitKind};
use unityrs_testkit::counted::{Allocator, Tally};

thread_local! {
    static REQUESTED: Cell<Option<usize>> = const { Cell::new(None) };
}
struct Owner;
impl Tally for Owner {
    fn grew(&self, size: usize, _: bool) {
        REQUESTED.with(|r| {
            if let Some(total) = r.get() {
                r.set(Some(total + size));
            }
        });
    }
    fn shrank(&self, _: usize) {}
}
#[global_allocator]
static ALLOC: Allocator<Owner> = Allocator(Owner);

fn data() -> Bytes {
    // 1.25.2 blob format 19: one root, names from the common-string table,
    // no local strings and therefore no NUL-index allocation.
    let mut bytes = Vec::new();
    bytes.extend(1i32.to_le_bytes());
    bytes.extend(0i32.to_le_bytes());
    bytes.extend([1, 0, 0, 0]);
    for offset in [0x8000_0000u32, 0x8000_0031] {
        bytes.extend(offset.to_le_bytes());
    }
    bytes.extend([0; 20]);
    bytes.into()
}

fn read(data: &Bytes, meter: &Arc<Meter>) -> Result<unityrs_core::typetree::SharedNodes, Error> {
    let mut reader = Reader::new(data, Endian::Little);
    let rows = parse_blob(&mut reader, data, 19, meter, &Limits::default(), None)?;
    Ok(NodeList::from_rows(rows)?.into_shared()?)
}

#[test]
fn legacy_blob_owner_matches_the_heap_and_exact_memory_boundary() {
    unityrs_testkit::counted::count_this_thread();
    let data = data();
    let meter = Meter::unlimited();
    let mut reader = Reader::new(&data, Endian::Little);
    let rows = parse_blob(&mut reader, &data, 19, &meter, &Limits::default(), None).unwrap();
    let row_cost = heap_cost((rows.capacity() * size_of::<NodeRow>()) as u64);
    let list = NodeList::from_rows(rows).unwrap();
    let owner_cost = list.charge().bytes() - row_cost - heap_cost(8);
    REQUESTED.with(|r| r.set(Some(0)));
    let shared = list.into_shared().unwrap();
    let requested = REQUESTED.with(|r| r.replace(None).unwrap());
    assert_eq!(owner_cost, heap_cost(requested as u64));
    assert_eq!(requested, size_of::<NodeList>() + size_of::<usize>());
    drop(shared);
    assert_eq!(meter.used(), 0);

    // Peak: rows + owner + child_start + parents + cursor. The parent
    // stack is already dropped when the last three tables coexist.
    let bound = row_cost + owner_cost + 3 * heap_cost(8);
    for (limit, passes) in [(bound - 1, false), (bound, true)] {
        let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
        let result = read(&data, &meter);
        if passes {
            assert_eq!(result.as_ref().unwrap().len(), 1);
        } else {
            assert!(
                matches!(result,
                Err(Error::Tree(unityrs_core::typetree::TreeError::Charge(
                    unityrs_core::limits::ChargeError::Limit(ref l))))
                    if l.kind() == LimitKind::InputMemory),
                "{result:?}"
            );
        }
        drop(result);
        assert_eq!(meter.used(), 0);
    }
}
