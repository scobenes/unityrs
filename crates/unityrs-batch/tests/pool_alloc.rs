mod common;
mod controller_support;
mod publication_support;
mod stage_support;
use std::{
    path::PathBuf,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};
use unityrs_batch::{
    alloc::{Counting, Hook},
    finish,
    phase::{self, Account},
    publish,
    sink::directory::Directory,
    spool::{FileSystem, Host, workspace::Workspace},
    sys::{Lock, Slot},
    winners,
};
use unityrs_testkit::{child, guard, scratch::Scratch};
static ACTIVE: AtomicBool = AtomicBool::new(false);
static SITE: AtomicUsize = AtomicUsize::new(0);
static SEEN: AtomicUsize = AtomicUsize::new(0);
struct Refuse;
impl Hook for Refuse {
    fn refuses(&self, _: usize) -> bool {
        ACTIVE.load(Ordering::SeqCst)
            && SEEN.fetch_add(1, Ordering::SeqCst) + 1 == SITE.load(Ordering::SeqCst)
    }
}
#[global_allocator]
static ALLOC: Counting<Refuse> = Counting(Refuse);
#[test]
#[cfg_attr(debug_assertions, ignore = "scale: run in release")]
fn every_normal_tail_allocation_is_fallible_after_the_running_head() {
    let output = guard::finishes("normal tail refusal sweep", || {
        child::output(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "pool_refusal_child", "--nocapture"])
                .env("UNITYRS_POOL_REFUSALS", "1"),
        )
        .unwrap()
    });
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", String::from_utf8_lossy(&output.stdout));
}

struct Tail {
    prepared: unityrs_batch::phase1::PreparedRun,
    done: PathBuf,
    counter: Arc<unityrs_batch::spool::Counter>,
    directory: Arc<Directory>,
    account: Arc<Account>,
    fs: Arc<dyn FileSystem>,
    control: Arc<AtomicBool>,
    hooks: Arc<publish::NoHooks>,
    slot: Slot<publish::Owned>,
    workspace_path: PathBuf,
    outcomes_path: PathBuf,
    head_path: PathBuf,
}
impl Tail {
    fn new() -> (Self, Scratch) {
        let scene = publication_support::Scene::new(&["a", "b"], false, 4096);
        let sink = scene.scratch.join("sink");
        std::fs::create_dir(&sink).unwrap();
        let directory = Arc::new(
            Directory::preflight(
                &sink,
                "r",
                1,
                Arc::new(publication_support::Portable),
                &mut vec![0; unityrs_batch::manifest::head::HEAD_LIMIT + 1],
            )
            .unwrap(),
        );
        let workspace_path = scene.scratch.join("selected-workspace");
        let outcomes_path = scene.scratch.join("selected-outcomes");
        std::fs::create_dir(&outcomes_path).unwrap();
        let head_path = sink.join("_unityrs/r/manifest.json");
        let tail = Self {
            prepared: scene.prepared,
            done: scene.done,
            counter: scene.counter,
            directory,
            account: Arc::new(Account::new(64 << 20)),
            fs: Arc::new(Host),
            control: Arc::new(AtomicBool::new(false)),
            hooks: Arc::new(publish::NoHooks),
            slot: Slot::reserve().unwrap(),
            workspace_path,
            outcomes_path,
            head_path,
        };
        finish::running(
            &tail.prepared,
            &tail.directory,
            &Account::new(unityrs_batch::bound::C_FIN),
            &finish::Identity {
                started_at: "2026-01-01T00:00:00Z",
                memory_bound: 0,
                spool_peak: 0,
                vm_hwm: 0,
            },
        )
        .unwrap();
        (tail, scene.scratch)
    }

    fn run(
        self,
        execution: &unityrs_batch::execution::Execution,
        stages: &mut [usize; 4],
    ) -> Result<publish::Report, phase::Error> {
        // Paths, adapters and the final shared owner were reserved before the
        // running head. The done-path helper has its own targeted refusal test;
        // everything below exercises selection and publication allocations.
        let mut workspace =
            Workspace::create(self.workspace_path, self.counter.clone(), self.fs.clone())?;
        stages[0] = SEEN.load(Ordering::SeqCst);
        let inputs = winners::committed::Committed::received(
            &self.done,
            &self.prepared.inputs,
            &*self.fs,
            &|| false,
        )?;
        let handoff = winners::complete(&mut winners::Context {
            spec: &self.prepared.spec,
            prepared: &self.prepared.winners,
            inputs: &inputs,
            workspace: &mut workspace,
            account: &self.account,
            options: unityrs_batch::sort::Options::default(),
            stopped: &|| false,
            hooks: &unityrs_batch::sort::NoHooks,
            observer: &winners::NoObserver,
        })?;
        stages[1] = SEEN.load(Ordering::SeqCst);
        let outcomes = unityrs_batch::outcomes::Outcomes::create(
            &self.outcomes_path,
            handoff.counts.winners,
            u64::try_from(self.prepared.spec.archives.len()).unwrap(),
            &self.counter,
            &*self.fs,
            &[0; 128],
            &|| false,
        )?;
        stages[2] = SEEN.load(Ordering::SeqCst);
        let owned = self.slot.write(publish::Owned {
            prepared: Lock::new(self.prepared),
            done: self.done,
            workspace,
            handoff,
            directory: self.directory,
            outcomes,
            account: self.account,
            gate: publish::Gate::default(),
            fs: self.fs,
            control: self.control,
            hooks: self.hooks,
            progress: None,
            spool_lease: None,
        });
        let result = publish::parallel(&owned, execution, 1);
        stages[3] = SEEN.load(Ordering::SeqCst);
        result
    }
}
#[test]
fn pool_refusal_child() {
    if std::env::var_os("UNITYRS_POOL_REFUSALS").is_none() {
        return;
    }
    let mut sites = 0;
    for site in 0..1024 {
        let mut harness = controller_support::Harness::new(
            &[("a", controller_support::clear(1), vec![])],
            &[],
            1,
            1,
        );
        let controller = harness.controller(Arc::new(controller_support::Observe::default()));
        let mut execution = unityrs_batch::execution::Execution::new(&controller).unwrap();
        // Keep scratch outside the armed closure and every live pool task.
        let (tail, _scratch) = Tail::new();
        let account = tail.account.clone();
        let head_path = tail.head_path.clone();
        let mut stages = [0; 4];
        println!("refusing site {site}");
        SITE.store(site, Ordering::SeqCst);
        SEEN.store(0, Ordering::SeqCst);
        ACTIVE.store(true, Ordering::SeqCst);
        let result = phase::on_pool(|| tail.run(&execution, &mut stages));
        // A refused task can still be unwinding its charged owner.
        execution.quiesce().unwrap();
        ACTIVE.store(false, Ordering::SeqCst);
        if site == 0 {
            sites = SEEN.load(Ordering::SeqCst);
            assert_eq!(result.unwrap().written, 2);
            assert!(stages.windows(2).all(|pair| pair[0] < pair[1]));
            assert!(stages[0] > 0);
            println!(
                "POOL_TAIL_BOUNDARIES workspace={} selection={} outcomes={} publication={}",
                stages[0], stages[1], stages[2], stages[3]
            );
        } else {
            assert!(SEEN.load(Ordering::SeqCst) >= site, "unreached {site}");
            assert_eq!(
                result,
                Err(unityrs_batch::phase::Error::memory()),
                "site {site}"
            );
        }
        let snapshot = account.snapshot();
        assert!(snapshot.is_clear(), "site {site}: {snapshot:?}");
        let head: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&head_path).unwrap()).unwrap();
        assert_eq!(head["status"], "running", "site {site}");
        assert_eq!(head["write_seq"], 0, "site {site}");
        if site == sites {
            println!("POOL_REFUSALS sites={sites}");
            return;
        }
    }
    panic!("normal tail allocation sweep exceeded cap");
}
