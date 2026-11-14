mod common;
mod publication_support;
use publication_support::{Portable, Scene};
use std::{cell::Cell, process::Command, sync::Arc};
use unityrs_batch::{
    alloc::{self, Counting, Hook},
    attempt::ReasonCode,
    manifest::{self, chunks::Directory as RowDirectory},
    outcomes::Outcomes,
    phase::{Account, Error},
    publish::{self, Context, Gate, payload::Committed},
    sink::directory::Directory,
    spool::Host,
    winners,
};
use unityrs_testkit::child;
thread_local! {
    static BIG:Cell<usize>=const{Cell::new(0)};
    static VERIFY_REUSE:Cell<bool>=const{Cell::new(false)};
    static ACTIVE:Cell<bool>=const{Cell::new(false)};
    static SITE:Cell<usize>=const{Cell::new(0)};
    static SEEN:Cell<usize>=const{Cell::new(0)};
}
struct Refuse;
impl Hook for Refuse {
    fn refuses(&self, size: usize) -> bool {
        if !ACTIVE.try_with(Cell::get).unwrap_or(false) {
            return false;
        }
        if size == publish::BUFFER {
            BIG.with(|s| s.set(s.get() + 1));
        }
        let n = SEEN.with(|s| {
            let n = s.get() + 1;
            s.set(n);
            n
        });
        if SITE.with(Cell::get) == n {
            SITE.with(|s| s.set(0));
            true
        } else {
            false
        }
    }
}
#[global_allocator]
static ALLOC: Counting<Refuse> = Counting(Refuse);
fn measured_case(
    scene: &Scene,
    directory: &Directory,
    account: &Account,
    finishing: &Account,
    root: &std::path::Path,
    mode: u8,
) -> Result<(), Error> {
    let out = Outcomes::create(
        root,
        scene.handoff.counts.winners,
        scene.prepared.spec.archives.len() as u64,
        &scene.counter,
        &Host,
        &[0; 128],
        &|| false,
    )?;
    let gate = Gate::default();
    let admission = publish::drive(&Context {
        handoff: &scene.handoff,
        workspace: &scene.workspace,
        payloads: &Committed {
            done: &scene.done,
            states: &scene.prepared.inputs,
            fs: &Host,
        },
        directory,
        outcomes: &out,
        gate: &gate,
        account,
        run_id: "r",
        resume: mode == 1 || mode == 2,
        stopped: &|| false,
        hooks: &publish::NoHooks,
    })?;
    if VERIFY_REUSE.with(Cell::get) {
        let expected = match mode {
            0 | 2 => publish::Report {
                written: 3,
                ..publish::Report::default()
            },
            1 => publish::Report {
                present: 3,
                ..publish::Report::default()
            },
            3 => publish::Report {
                failed: 3,
                ..publish::Report::default()
            },
            _ => publish::Report::default(),
        };
        assert_eq!(admission, publish::Admission::Drained(expected));
        assert_eq!(
            BIG.with(Cell::get),
            usize::from(!scene.handoff.publication_blocked),
            "one transfer/hash allocation per serial drive"
        );
    }
    let frozen = gate.freeze(&out)?;
    let inputs = winners::committed::Committed::received(
        &scene.done,
        &scene.prepared.inputs,
        &Host,
        &|| false,
    )?;
    manifest::assemble(
        &manifest::rows::Context {
            prepared: &scene.prepared,
            inputs: &inputs,
            handoff: &scene.handoff,
            workspace: &scene.workspace,
            frozen: &frozen,
            account: finishing,
            stopped: &|| false,
            taken_at: std::time::Instant::now(),
        },
        &mut RowDirectory {
            directory,
            run_id: "r",
            attempt: 1,
        },
    )?;
    Ok(())
}
#[test]
fn allocation_child() {
    if std::env::var_os("UNITYRS_PUBLICATION_ALLOC").is_none() {
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(runtime.spawn_blocking(sweep)).unwrap();
}
fn sweep() {
    sweep_resolver(false);
    sweep_resolver(true);
}
fn sweep_resolver(native: bool) {
    let scene = Scene::new(&["parent/a", "parent/b", "z"], true, 4096);
    let blocked = Scene::with_failure(&["parent/a", "parent/b", "z"], true, 4096, true);
    let root = scene.scratch.join("sink");
    std::fs::create_dir(&root).unwrap();
    #[cfg(target_os = "linux")]
    if native {
        let root_file = unityrs_batch::sys::open_directory(&root).unwrap();
        if let Err(error) = unityrs_batch::sys::beneath_probe(&root_file) {
            assert!(
                matches!(error.raw_os_error(), Some(libc::ENOSYS | libc::EPERM)),
                "{error}"
            );
            eprintln!("native allocation sweep unavailable: {error}");
            return;
        }
    }
    let fs: Arc<dyn unityrs_batch::sink::fs::FileSystem> = if native {
        native_adapter()
    } else {
        Arc::new(Portable)
    };
    let mut allocations = 0;
    for mode in 0..5 {
        let scene = if mode == 4 { &blocked } else { &scene };
        for site in 0..10_000 {
            if site > 0 && site > allocations {
                break;
            }
            let account = Account::new(64 << 20);
            let finishing = Account::new(unityrs_batch::bound::C_FIN);
            let out_root = scene.scratch.join(format!("o-{mode}-{site}"));
            std::fs::create_dir(&out_root).unwrap();
            // Counting and refusal trials must start from identical sink state.
            // In particular, first creation of _unityrs/r/1 allocates a native
            // revalidation path; reusing it made counted site 190 unreachable.
            std::fs::remove_dir_all(&root).unwrap();
            std::fs::create_dir(&root).unwrap();
            let dir =
                Directory::preflight(&root, "r", 1, fs.clone(), &mut vec![0; 1_048_577]).unwrap();
            if mode != 0 && mode != 4 {
                std::fs::create_dir(root.join("parent")).unwrap();
                for key in ["parent/a", "parent/b", "z"] {
                    let path = root.join(key);
                    match mode {
                        1 => std::fs::write(path, b"payload").unwrap(),
                        2 => std::fs::write(path, b"payloae").unwrap(),
                        3 => std::fs::create_dir(path).unwrap(),
                        _ => unreachable!(),
                    }
                }
            }
            BIG.with(|s| s.set(0));
            VERIFY_REUSE.with(|s| s.set(site == 0));
            SEEN.with(|s| s.set(0));
            SITE.with(|s| s.set(site));
            ACTIVE.with(|s| s.set(true));
            let (result, measured) = alloc::measured(|| {
                measured_case(scene, &dir, &account, &finishing, &out_root, mode)
            });
            ACTIVE.with(|s| s.set(false));
            if site == 0 {
                result.unwrap();
                allocations = SEEN.with(Cell::get);
                assert!(allocations < 10_000);
                assert!(measured.peak <= publish::WORKING_SET, "{measured:?}");
                println!(
                    "PUBLICATION_ALLOC native={native} mode={mode} sites={allocations} counted_peak={} account_peak={}",
                    measured.peak,
                    account
                        .snapshot()
                        .input
                        .peak
                        .max(finishing.snapshot().input.peak)
                );
            } else {
                assert_eq!(SITE.with(Cell::get), 0, "unreached {site}");
                assert!(
                    matches!(result,Err(Error::Run(f)) if f.reason==ReasonCode::HostMemory),
                    "{mode} {site}: {result:?}"
                );
            }
            assert_eq!(account.snapshot().input.used, 0);
            assert_eq!(finishing.snapshot().input.used, 0);
            assert_eq!(measured.kept, 0);
        }
    }
}
#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's `scale` job does"
)]
fn each_reachable_allocation_is_fallible_in_a_counted_child() {
    let output = child::output(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "allocation_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("UNITYRS_PUBLICATION_ALLOC", "1"),
    )
    .unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    print!("{}", String::from_utf8_lossy(&output.stdout));
}

// Exercise Root's allocating beneath revalidation on macOS too. This adapter
// emulates only resolver selection; Linux still uses the real openat2 host.
fn native_adapter() -> Arc<dyn unityrs_batch::sink::fs::FileSystem> {
    #[cfg(target_os = "linux")]
    {
        Arc::new(unityrs_batch::sink::fs::Host)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Arc::new(Revalidation)
    }
}
#[cfg(not(target_os = "linux"))]
struct Revalidation;
#[cfg(not(target_os = "linux"))]
impl unityrs_batch::sink::fs::FileSystem for Revalidation {
    fn release(&self) -> std::io::Result<(u32, u32)> {
        Ok((5, 8))
    }
    fn probe_sink_semantics(&self) -> bool {
        false
    }
    fn probe(&self, _: &std::fs::File) -> std::io::Result<()> {
        Ok(())
    }
    fn mount(&self, file: &std::fs::File) -> std::io::Result<Option<u64>> {
        use std::os::unix::fs::MetadataExt;
        Ok(Some(file.metadata()?.dev()))
    }
    fn writeback(&self, _: &std::fs::File) -> std::io::Result<()> {
        Ok(())
    }
}
