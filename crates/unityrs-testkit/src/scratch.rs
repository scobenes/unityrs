//! Scratch folders: the rule every Rust test in the workspace keeps for
//! what it writes outside its own memory. Its scratch is removed whatever
//! happens: when it passes, fails or panics, and when a child it runs
//! ends, aborts, overflows its stack or is killed. (Review round 9: a stack
//! probe's folder, made in a child its bisection overflows by design,
//! outlived every such child; a test whose own process a mutant aborted
//! left a chain of 996 folders; two tests removed their files only after
//! their asserts.)
//!
//! - **Every scratch folder is a [`Scratch`]**, removed with what it holds
//!   when dropped, which a panic's unwinding does too. No test names
//!   [`std::env::temp_dir`] but this module ([`direct_scratch`] scans for
//!   it).
//! - **A child's scratch is its parent's.** [`crate::child::output`] and
//!   [`crate::child::status`] make a folder for each run of a child, name
//!   it in the child's [`VAR`], and remove it when the run ends, however it
//!   ends. A child that makes a [`Scratch`] (this test binary run again as
//!   a probe, or a Python test through `tools/parity/scratch.py`, which
//!   keeps the same rule) makes it inside that folder. A child that aborts
//!   or is killed runs no drop of its own, so its parent's removal is the
//!   one that runs.
//! - **A process with no parent's folder** makes its folders in a root of
//!   its own in the temp folder, `unityrs-test-<pid namespace>-<pid>`,
//!   which goes with the last of them. A process that dies in its own code
//!   (an abort, a stack overflow, a kill) leaves its root behind, so the
//!   first folder any process makes removes every such root whose process
//!   no longer runs ([`sweep`]): a root is its own process's alone, and
//!   only roots of this pid namespace are compared with its processes.
//!
//! Owner: package B.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

/// The variable naming the folder a parent made for its child to make its
/// scratch in (module docs).
pub const VAR: &str = "UNITYRS_TEST_SCRATCH";

/// The name of every process's own root, before its pid namespace and pid.
pub const PREFIX: &str = "unityrs-test-";

struct Root {
    path: PathBuf,
    /// The parent's folder ([`VAR`]), which the parent removes.
    given: bool,
    /// The folders made in it and not yet dropped.
    live: usize,
}

static ROOT: Mutex<Option<Root>> = Mutex::new(None);
static NEXT: AtomicU64 = AtomicU64::new(0);

/// A new empty folder, removed with everything in it when dropped (module
/// docs).
#[derive(Debug)]
pub struct Scratch(PathBuf);

impl Scratch {
    /// A new empty folder named after `tag`, inside the parent's folder or
    /// this process's root (module docs).
    ///
    /// # Panics
    /// When the folder cannot be made.
    #[must_use]
    pub fn new(tag: &str) -> Self {
        let mut root = ROOT.lock().unwrap_or_else(PoisonError::into_inner);
        let root = root.get_or_insert_with(open_root);
        let path = root.path.join(format!(
            "{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        root.live += 1;
        Self(path)
    }

    /// The folder.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.0
    }

    /// `path` inside the folder.
    #[must_use]
    pub fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.0.join(path)
    }
}

impl std::ops::Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for Scratch {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        remove_tree(&self.0);
        let mut root = ROOT.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(r) = root.as_mut() {
            r.live -= 1;
            if r.live == 0 {
                if !r.given {
                    let _ = std::fs::remove_dir(&r.path);
                }
                *root = None;
            }
        }
    }
}

/// This process's root: the parent's folder when [`VAR`] names one, else a
/// new one of its own in the temp folder, after a [`sweep`].
fn open_root() -> Root {
    if let Some(given) = std::env::var_os(VAR).filter(|v| !v.is_empty()) {
        let path = PathBuf::from(given);
        if path.is_dir() {
            return Root {
                path,
                given: true,
                live: 0,
            };
        }
    }
    sweep();
    let path = std::env::temp_dir().join(format!(
        "{PREFIX}{}-{}",
        namespace().unwrap_or(0),
        std::process::id()
    ));
    // A dead process's of the same pid, which only this process can have now.
    remove_tree(&path);
    std::fs::create_dir(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Root {
        path,
        given: false,
        live: 0,
    }
}

/// Removes `path` and everything under it a folder at a time, without
/// following a link: a chain of folders past the stack or the open-file
/// limit, and a folder of mode 000, go too. Nothing when it is not there.
pub fn remove_tree(path: &Path) {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return;
    };
    if !meta.is_dir() {
        let _ = std::fs::remove_file(path);
        return;
    }
    let mut todo = vec![path.to_path_buf()];
    let mut folders = Vec::new();
    while let Some(dir) = todo.pop() {
        open_up(&dir);
        let entries = std::fs::read_dir(&dir);
        folders.push(dir);
        let Ok(entries) = entries else {
            continue;
        };
        for e in entries.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                todo.push(e.path());
            } else {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    for dir in folders.iter().rev() {
        let _ = std::fs::remove_dir(dir);
    }
}

#[cfg(unix)]
fn open_up(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn open_up(_: &Path) {}

/// Removes every root in the temp folder of a process of this pid
/// namespace that no longer runs (module docs). Nothing on a system
/// without `/proc`'s pid namespaces.
pub fn sweep() {
    let Some(ns) = namespace() else {
        return;
    };
    let own = format!("{PREFIX}{ns}-");
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.strip_prefix(&own)) else {
            continue;
        };
        if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        if pid != std::process::id().to_string() && !Path::new("/proc").join(pid).exists() {
            remove_tree(&e.path());
        }
    }
}

/// This process's pid namespace, as `/proc/self/ns/pid` names it.
fn namespace() -> Option<u64> {
    let link = std::fs::read_link("/proc/self/ns/pid").ok()?;
    let link = link.to_str()?;
    link.strip_prefix("pid:[")?.strip_suffix(']')?.parse().ok()
}

/// Every place under `root` (its `.rs` files, `target` directories left
/// out, this file too) that names the temp folder other than through this
/// module: each line outside a `//` comment that names `temp_dir()`.
///
/// # Panics
/// When a directory or a file under `root` cannot be read.
#[must_use]
pub fn direct_scratch(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    crate::guard::rust_files(root, &mut files);
    files.sort();
    let mut found = Vec::new();
    for path in files {
        if path.ends_with("unityrs-testkit/src/scratch.rs") {
            continue;
        }
        let text =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        for (i, line) in text.lines().enumerate() {
            let code = line.trim_start();
            if !code.starts_with("//") && code.contains("temp_dir()") {
                found.push(format!("{}:{}: {}", path.display(), i + 1, code));
            }
        }
    }
    found
}

#[cfg(all(test, unix))]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::{PoisonError, ROOT, Scratch, VAR, direct_scratch, remove_tree, sweep};
    use crate::child::{self, Marks};

    const PROBE_VAR: &str = "UNITYRS_TESTKIT_SCRATCH_PROBE";

    /// Run by the tests below, as `WHAT:DIR`. `write` makes a [`Scratch`],
    /// records its path, the parent's folder and this process's root in a
    /// file of `DIR` named by its run, writes into the folder and dies
    /// half way: killed by `SIGKILL` on its first run, aborted on the next.
    /// `last` makes two folders, records its root and whether it is there
    /// after the first goes and after the second, then passes.
    #[test]
    #[ignore = "run by the scratch tests"]
    fn probe() {
        let Ok(spec) = std::env::var(PROBE_VAR) else {
            return;
        };
        let (what, dir) = spec.split_once(':').unwrap();
        let dir = Path::new(dir);
        let runs = std::fs::read_dir(dir).unwrap().count();
        let first = Scratch::new("probe");
        let root = root();
        let parent = std::env::var(VAR).unwrap_or_default();
        let mut record = format!("{}\n{parent}\n{}\n", first.display(), root.display());
        if what == "last" {
            let second = Scratch::new("probe");
            drop(first);
            record.push_str(if root.exists() { "true\n" } else { "false\n" });
            drop(second);
            record.push_str(if root.exists() { "true\n" } else { "false\n" });
            std::fs::write(dir.join(runs.to_string()), record).unwrap();
            return;
        }
        std::fs::write(dir.join(runs.to_string()), record).unwrap();
        std::fs::write(first.join("half"), vec![7_u8; 4096]).unwrap();
        if runs == 0 {
            child::kill_self();
        }
        std::process::abort();
    }

    fn root() -> PathBuf {
        let root = ROOT.lock().unwrap_or_else(PoisonError::into_inner);
        root.as_ref().unwrap().path.clone()
    }

    /// The probe run as `what`, its marks in `marks`, with no parent's
    /// folder when `own_root`: its last run's status and each run's
    /// record, as lines.
    fn probe_run(
        what: &str,
        marks: &Marks,
        own_root: bool,
    ) -> (std::process::ExitStatus, Vec<Vec<String>>) {
        let exe = std::env::current_exe().unwrap();
        let mut cmd = Command::new(exe);
        cmd.args([
            "--exact",
            "scratch::tests::probe",
            "--ignored",
            "--test-threads=1",
        ])
        .env(PROBE_VAR, format!("{what}:{}", marks.0.display()));
        if own_root {
            cmd.env_remove(VAR);
        }
        let o = child::output(&mut cmd).unwrap();
        let mut records = Vec::new();
        for run in 0..marks.runs() {
            let text = std::fs::read_to_string(marks.0.join(run.to_string())).unwrap();
            records.push(text.lines().map(str::to_owned).collect());
        }
        (o.status, records)
    }

    /// A folder goes when it is dropped, a panic's unwinding included.
    #[test]
    fn a_scratch_goes_on_drop_and_on_a_panic() {
        let s = Scratch::new("drop");
        std::fs::write(s.join("f"), b"x").unwrap();
        let path = s.to_path_buf();
        drop(s);
        assert!(!path.exists());
        let kept = std::sync::Mutex::new(PathBuf::new());
        let r = std::panic::catch_unwind(|| {
            let s = Scratch::new("panic");
            std::fs::write(s.join("f"), b"x").unwrap();
            *kept.lock().unwrap() = s.to_path_buf();
            panic!("as designed");
        });
        assert!(r.is_err());
        assert!(!kept.into_inner().unwrap().exists());
    }

    /// The parent removes the folder it gave a child however the child
    /// ends: killed by `SIGKILL`, then aborted, each half way through a
    /// write, neither ran a drop (review round 9: a stack probe's folder
    /// outlived each child its bisection overflowed).
    #[test]
    fn a_child_that_dies_mid_write_leaves_nothing() {
        use std::os::unix::process::ExitStatusExt;
        let marks = Marks::new("mid-write");
        let (status, records) = probe_run("write", &marks, false);
        // A child's own root, were it given none, is left there: removed on this test's failure.
        let _roots: Vec<Guard> = records
            .iter()
            .map(|r| Guard(PathBuf::from(&r[2])))
            .collect();
        assert_eq!(status.signal(), Some(6), "{status:?}");
        assert_eq!(records.len(), 2, "killed once, then aborted");
        for r in &records {
            let (folder, parent, root) = (Path::new(&r[0]), Path::new(&r[1]), Path::new(&r[2]));
            assert_eq!(root, parent, "a child's root is its parent's folder");
            assert!(folder.starts_with(parent), "{r:?}");
            assert!(!parent.exists(), "{r:?}");
        }
        assert_ne!(
            records[0][1], records[1][1],
            "each run has a folder of its own"
        );
    }

    /// A process with no parent's folder that dies leaves its root, and the
    /// next sweep removes it (review round 9: a test whose own process a
    /// mutant aborted left a chain of 996 folders).
    #[test]
    fn a_process_that_dies_with_no_parent_folder_has_its_root_swept() {
        let marks = Marks::new("swept");
        let (status, records) = probe_run("write", &marks, true);
        // Left there, on this test's failure.
        let roots: Vec<Guard> = records
            .iter()
            .map(|r| Guard(PathBuf::from(&r[2])))
            .collect();
        assert!(!status.success());
        for r in &records {
            assert_eq!(r[1], "", "no parent's folder");
            let root = Path::new(&r[2]);
            assert_eq!(root.parent(), Some(std::env::temp_dir().as_path()));
            let name = root.file_name().unwrap().to_str().unwrap();
            let rest = name.strip_prefix(super::PREFIX).unwrap();
            assert!(rest.split('-').all(|n| n.parse::<u64>().is_ok()), "{name}");
        }
        sweep();
        for g in &roots {
            assert!(
                !g.0.exists(),
                "a dead process's root outlived a sweep: {}",
                g.0.display()
            );
        }
    }

    struct Guard(PathBuf);

    impl Drop for Guard {
        fn drop(&mut self) {
            remove_tree(&self.0);
        }
    }

    /// A process's own root goes with its last folder.
    #[test]
    fn the_root_goes_with_its_last_folder() {
        let marks = Marks::new("last");
        let (status, records) = probe_run("last", &marks, true);
        let _root = Guard(PathBuf::from(&records[0][2]));
        assert!(status.success(), "{status:?}");
        assert_eq!(records[0][3..], ["true", "false"]);
    }

    /// The removal takes a chain of folders deeper than a recursive walk's
    /// stack and a folder of mode 000, which `remove_dir_all` cannot.
    #[test]
    fn remove_tree_takes_a_deep_chain_and_a_closed_folder() {
        use std::os::unix::fs::PermissionsExt;
        let s = Scratch::new("deep");
        let mut deepest = s.to_path_buf();
        for _ in 0..1100 {
            deepest.push("d");
            std::fs::create_dir(&deepest).unwrap();
        }
        std::fs::set_permissions(&deepest, std::fs::Permissions::from_mode(0o000)).unwrap();
        let path = s.to_path_buf();
        remove_tree(&path);
        assert!(!path.exists());
    }

    /// Every test in the workspace makes its scratch here (review round 9:
    /// tests that removed theirs after their asserts left it on a failure).
    #[test]
    fn every_scratch_in_the_workspace_is_made_here() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let found = direct_scratch(&crates);
        assert!(
            found.is_empty(),
            "scratch made outside unityrs_testkit::scratch, which a failure or a \
             dying child leaves behind:\n{}",
            found.join("\n")
        );
    }

    /// The scan finds a temp folder named in code, and passes a comment.
    #[test]
    fn the_scan_finds_each_direct_temp_dir() {
        let s = Scratch::new("scan");
        std::fs::write(
            s.join("a.rs"),
            "// std::env::temp_dir() in a comment\nlet d = std::env::temp_dir().join(\"x\");\n",
        )
        .unwrap();
        std::fs::write(s.join("b.rs"), "let s = Scratch::new(\"x\");\n").unwrap();
        let found = direct_scratch(s.path());
        let at = s.join("a.rs").display().to_string();
        assert_eq!(
            found,
            [format!("{at}:2: let d = std::env::temp_dir().join(\"x\");")]
        );
    }
}
