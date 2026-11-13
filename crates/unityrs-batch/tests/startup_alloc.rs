//! Counted children pin the zero-allocation parser and fallible path boundary.
use std::{
    fs::File,
    io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use unityrs_batch::{
    manifest::head,
    sink::{self, fs::FileSystem, path::Root},
};
use unityrs_testkit::{
    child,
    counted::{self, Allocator, Tally},
    guard,
    scratch::Scratch,
};

static ARMED: AtomicBool = AtomicBool::new(false);
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
struct Counts;
impl Tally for Counts {
    fn refuses(&self, _: usize) -> bool {
        counted::counted() && ARMED.load(Ordering::SeqCst)
    }
    fn grew(&self, _: usize, _: bool) {
        ALLOCS.fetch_add(1, Ordering::SeqCst);
    }
    fn shrank(&self, _: usize) {}
}
#[global_allocator]
static ALLOC: Allocator<Counts> = Allocator(Counts);
struct Fs;
impl FileSystem for Fs {
    fn release(&self) -> io::Result<(u32, u32)> {
        Ok((5, 8))
    }
    fn probe(&self, _: &File) -> io::Result<()> {
        Err(io::Error::from_raw_os_error(libc::ENOSYS))
    }
    fn mount(&self, _: &File) -> io::Result<Option<u64>> {
        Ok(Some(1))
    }
}
#[test]
fn bounded_startup_allocations_are_checked_in_children() {
    for scene in ["head", "path"] {
        let out = guard::finishes("startup allocation probe", move || {
            child::output(
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "startup_allocation_child"])
                    .env("UNITYRS_STARTUP_ALLOC_PROBE", scene),
            )
            .unwrap()
        });
        assert!(
            out.status.success(),
            "{scene}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
#[test]
fn startup_allocation_child() {
    let Some(scene) = std::env::var_os("UNITYRS_STARTUP_ALLOC_PROBE") else {
        return;
    };
    if scene == "head" {
        let bytes=format!(r#"{{"manifest_version":999,"run_id":"r","attempt":1,"write_seq":0,"binary_version":"future","spec_version":1,"spec_sha256":"{}","unknown":["{}",{{"nested":[true,null,3]}}]}}"#,"a".repeat(64),"\\u0061".repeat(100_000)).into_bytes();
        counted::count_this_thread();
        let before = ALLOCS.load(Ordering::SeqCst);
        ARMED.store(true, Ordering::SeqCst);
        let result = head::recognize(std::hint::black_box(&bytes), "r");
        ARMED.store(false, Ordering::SeqCst);
        assert_eq!(result.unwrap().attempt, 1);
        assert_eq!(ALLOCS.load(Ordering::SeqCst), before);
    } else {
        let scratch = Scratch::new("startup-refused-path");
        let fs: Arc<dyn FileSystem> = Arc::new(Fs);
        counted::count_this_thread();
        ARMED.store(true, Ordering::SeqCst);
        let result = Root::open(Path::new(scratch.path()), fs);
        ARMED.store(false, Ordering::SeqCst);
        assert_eq!(result.err().unwrap().class, sink::Class::HostMemory);
        assert!(!scratch.join("_unityrs").exists());
    }
}
