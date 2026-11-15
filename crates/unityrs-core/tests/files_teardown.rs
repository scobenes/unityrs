//! Teardown must work on small stacks with no allocator or budget headroom.
#![allow(clippy::unwrap_used)]

use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};

use unityrs_core::{
    BundleFile, Bytes, File, FileContext, RawStr, WebFile,
    budget::{Reservation, Unbounded},
    limits::Meter,
};
use unityrs_testkit::{
    counted::{Allocator, Tally, count_this_thread},
    web,
};

thread_local! {
    static REFUSE: Cell<bool> = const { Cell::new(false) };
    static ATTEMPTS: Cell<usize> = const { Cell::new(0) };
    static DROP_METER: RefCell<Option<Arc<Meter>>> = const { RefCell::new(None) };
    static DIRECTORY_SIZE: Cell<usize> = const { Cell::new(0) };
    static USED_AT_DIRECTORY_FREE: Cell<Option<u64>> = const { Cell::new(None) };
}

struct Refusing;
impl Tally for Refusing {
    fn grew(&self, _: usize, _: bool) {}
    fn shrank(&self, size: usize) {
        // Field order frees the directory before the files table. Sample
        // inside deallocation, while that remaining table is still live.
        let _ = DIRECTORY_SIZE.try_with(|expected| {
            if size == expected.get() {
                DROP_METER.with(|meter| {
                    if let Some(meter) = meter.borrow().as_ref() {
                        USED_AT_DIRECTORY_FREE.set(Some(meter.used()));
                    }
                });
            }
        });
    }

    fn refuses(&self, _: usize) -> bool {
        if REFUSE.try_with(Cell::get).unwrap_or(false) {
            ATTEMPTS.with(|n| n.set(n.get() + 1));
            true
        } else {
            false
        }
    }
}

#[global_allocator]
static ALLOCATOR: Allocator<Refusing> = Allocator(Refusing);

fn without_allocations(f: impl FnOnce()) {
    ATTEMPTS.set(0);
    REFUSE.set(true);
    f();
    REFUSE.set(false);
    assert_eq!(ATTEMPTS.get(), 0, "teardown attempted an allocation");
}

#[test]
fn web_root_tables_stay_charged_until_they_are_freed() {
    root_tables_stay_charged(false);
}

#[test]
fn bundle_root_tables_stay_charged_until_they_are_freed() {
    root_tables_stay_charged(true);
}

fn root_tables_stay_charged(bundle: bool) {
    count_this_thread();
    let names: Vec<_> = (0..4000).map(|i| format!("row{i}")).collect();
    let rows: Vec<_> = names
        .iter()
        .map(|n| (n.as_str(), b"raw".as_slice()))
        .collect();
    let data: Bytes = if bundle {
        web::legacy(b"UnityRaw", 6, &rows)
    } else {
        web::plain("UnityWebData1.0", &rows)
    }
    .into();
    let meter = Meter::new(u64::MAX, Arc::new(Unbounded), Reservation::Input);
    let ctx = Arc::new(FileContext::unitypy());
    let name = RawStr::from_static(b"root");
    let root = if bundle {
        File::Bundle(BundleFile::open(name, data, None, &meter, &ctx).unwrap())
    } else {
        File::Web(WebFile::open(name, data, None, &meter, &ctx).unwrap())
    };
    let files_bytes = size_of_val(root.container_files()) as u64;
    assert!(files_bytes > 0);
    DIRECTORY_SIZE.set(size_of_val(root.container_directory()));
    USED_AT_DIRECTORY_FREE.set(None);
    DROP_METER.with(|slot| *slot.borrow_mut() = Some(meter.clone()));
    without_allocations(|| drop(root));
    DROP_METER.with(|slot| *slot.borrow_mut() = None);
    let used = USED_AT_DIRECTORY_FREE
        .get()
        .expect("directory free was sampled");
    assert!(
        used >= files_bytes,
        "bundle={bundle}: {files_bytes} live table bytes, only {used} charged"
    );
    assert_eq!(meter.used(), 0);
}

#[test]
fn deep_shared_and_branched_graphs_drop_without_allocating_on_a_small_stack() {
    let mut data = b"raw".to_vec();
    for _ in 0..16_000 {
        data = web::plain("UnityWebData1.0", &[("n", &data)]);
    }
    // Multiple populated sibling tables must stay on the pending list.
    let branch = web::legacy(b"UnityRaw", 5, &[("leaf", b"raw")]);
    data = web::plain(
        "UnityWebData1.0",
        &[("deep", &data), ("branch", &branch), ("twin", &branch)],
    );
    std::thread::Builder::new()
        .stack_size(512 * 1024)
        .spawn(move || {
            for _ in 0..5 {
                let mut ctx = FileContext::unitypy();
                ctx.limits.max_container_depth = 16_001;
                let ctx = Arc::new(ctx);
                let meter = Meter::new(u64::MAX, Arc::new(Unbounded), Reservation::Input);
                let root = WebFile::open(
                    RawStr::from("root"),
                    data.clone().into(),
                    None,
                    &meter,
                    &ctx,
                )
                .unwrap();
                for i in 1..3 {
                    if let File::Bundle(file) = root.entry(i).unwrap() {
                        file.load_all().unwrap();
                    }
                }
                let mut current = root.entry(0).unwrap();
                let shared = current.clone();
                for _ in 0..16_000 {
                    current = current.container_entry(0).unwrap();
                }
                drop(current);
                // Exhaust the meter too: destruction cannot request a charge.
                let headroom = meter.charge(meter.limit() - meter.used()).unwrap();
                without_allocations(|| drop(root));
                assert!(meter.used() > headroom.bytes());
                without_allocations(|| drop(shared));
                assert_eq!(meter.used(), headroom.bytes());
                drop(headroom);
                assert_eq!(meter.used(), 0);
            }
        })
        .unwrap()
        .join()
        .unwrap();
}
