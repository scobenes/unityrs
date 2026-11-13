use std::{cell::Cell, fs, sync::Arc};
use unityrs_batch::{
    alloc::{Counting, Hook},
    spool::{Counter, Directory, Error, Host},
};
use unityrs_testkit::{guard, scratch::Scratch};

thread_local! { static REFUSE_STACK: Cell<bool> = const { Cell::new(false) }; }
struct Refuse;
impl Hook for Refuse {
    fn refuses(&self, size: usize) -> bool {
        // Component names are short in this scene. The first allocation of
        // at least 128 bytes is the complete bounded cursor stack.
        size >= 128 && REFUSE_STACK.try_with(|r| r.replace(false)).unwrap_or(false)
    }
}
#[global_allocator]
static ALLOC: Counting<Refuse> = Counting(Refuse);

#[test]
fn a_refused_cleanup_stack_keeps_artifacts_and_the_complete_claim() {
    let () = guard::finishes("fallible cleanup stack", || {
        let scratch = Scratch::new("removal-alloc");
        let counter = Counter::new(1 << 20, 0, 4_096).unwrap();
        let mut directory =
            Directory::create(scratch.join("owned"), counter.clone(), Arc::new(Host)).unwrap();
        let mut file = directory.output(0).unwrap();
        file.append(b"keep").unwrap();
        file.finish().unwrap();
        REFUSE_STACK.with(|r| r.set(true));
        let result = directory.remove_stoppable(&|| false);
        let unused = REFUSE_STACK.with(|r| r.replace(false));
        assert!(!unused, "cursor-stack refusal must be exercised");
        assert!(matches!(result, Err(Error::HostMemory)));
        assert_eq!(directory.claimed(), 4_096);
        assert_eq!(counter.used(), 4_096);
        assert_eq!(fs::read(directory.path().join("o0")).unwrap(), b"keep");
        directory.remove_stoppable(&|| false).unwrap();
        assert_eq!(counter.used(), 0);
    });
}
