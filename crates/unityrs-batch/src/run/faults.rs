//! The `files` and `host_root` test hooks (`docs/plans/batch.md` 2.11's
//! *Test hooks*), compiled only under `test-hooks`.
//!
//! `UNITYRS_TEST_FILES` names a fault plan for the file calls on the spool
//! at its start (`spool.<call>`), the sink (`sink.<call>`) and the started
//! spool (`started.<call>`). Each call is appended to `<plan>.log` as its
//! name. A plan line is `<fs>.<call> <n | *> <action>`, the rule applying
//! to that call's `n`th time (from 1) or every time:
//!
//! - `fail <ERRNO>`: the call is not made and returns that error;
//! - `after <ERRNO>`: the call is made, then returns that error (a rename
//!   into `done/` that took effect and reported a failure);
//! - `delay <ms>`: the call is made after the delay;
//! - `figure <n>`: `available` answers `n` bytes (`statvfs`'s figure).
//!
//! `UNITYRS_TEST_HOST_ROOT` names a directory at which step 6 reads the
//! procfs and cgroup files, a `capget` file there failing `capget`
//! (`EPERM`), and at which preflight reads the kernel's `release`.
use crate::{
    sink::fs::Host as SinkHost,
    spool::{self, Host as SpoolHost},
    start::{
        host::{Host, Limit, Resource, System},
        spool::Host as StartHost,
    },
};
use std::{
    ffi::CStr,
    fs::File,
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering::SeqCst},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Fail(i32),
    After(i32),
    Delay(u64),
    Figure(u64),
}
struct Rule {
    call: String,
    nth: Option<u64>,
    action: Action,
    seen: AtomicU64,
}
/// A parsed fault plan and its call log.
pub struct Plan {
    rules: Vec<Rule>,
    log: Option<super::hooks::Log>,
}
fn errno(name: &str) -> Option<i32> {
    let named = [
        ("EIO", libc::EIO),
        ("ENOSPC", libc::ENOSPC),
        ("EDQUOT", libc::EDQUOT),
        ("EACCES", libc::EACCES),
        ("EPERM", libc::EPERM),
        ("ENOENT", libc::ENOENT),
        ("EEXIST", libc::EEXIST),
        ("EROFS", libc::EROFS),
        ("EMFILE", libc::EMFILE),
        ("ENOMEM", libc::ENOMEM),
        ("EXDEV", libc::EXDEV),
        ("ENOSYS", libc::ENOSYS),
        ("EAGAIN", libc::EAGAIN),
        ("EINVAL", libc::EINVAL),
    ];
    named
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, e)| *e)
        .or_else(|| name.parse().ok())
}
impl Plan {
    /// The plan at `path`; `None` for a malformed line.
    #[must_use]
    pub fn read(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut rules = Vec::new();
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            let mut words = line.split_whitespace();
            let (call, nth, action, value) =
                (words.next()?, words.next()?, words.next()?, words.next()?);
            let nth = if nth == "*" {
                None
            } else {
                Some(nth.parse().ok()?)
            };
            let action = match action {
                "fail" => Action::Fail(errno(value)?),
                "after" => Action::After(errno(value)?),
                "delay" => Action::Delay(value.parse().ok()?),
                "figure" => Action::Figure(value.parse().ok()?),
                _ => return None,
            };
            rules.push(Rule {
                call: call.to_owned(),
                nth,
                action,
                seen: AtomicU64::new(0),
            });
        }
        let mut log = path.as_os_str().to_owned();
        log.push(".log");
        let log = super::hooks::Log::append(Path::new(&log)).ok();
        Some(Self { rules, log })
    }
    /// Log `call` and find the rule that applies to this one.
    fn hit(&self, call: &str) -> Option<Action> {
        if let Some(log) = &self.log {
            log.line(format_args!("{call}"));
        }
        let mut found = None;
        for rule in self.rules.iter().filter(|r| r.call == call) {
            let n = rule.seen.fetch_add(1, SeqCst) + 1;
            if found.is_none() && rule.nth.is_none_or(|nth| nth == n) {
                found = Some(rule.action);
            }
        }
        found
    }
    fn call<T>(&self, call: &str, f: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        match self.hit(call) {
            Some(Action::Fail(e)) => Err(io::Error::from_raw_os_error(e)),
            Some(Action::After(e)) => f().and(Err(io::Error::from_raw_os_error(e))),
            Some(Action::Delay(ms)) => {
                std::thread::sleep(std::time::Duration::from_millis(ms));
                f()
            }
            _ => f(),
        }
    }
    fn figure(&self, call: &str, f: impl FnOnce() -> io::Result<u64>) -> io::Result<u64> {
        match self.hit(call) {
            Some(Action::Figure(n)) => Ok(n),
            Some(Action::Fail(e)) => Err(io::Error::from_raw_os_error(e)),
            _ => f(),
        }
    }
}
/// The hosts' filesystems under a plan, a host root, or both.
pub struct Files {
    pub plan: Option<Plan>,
    pub root: Option<PathBuf>,
}
impl Files {
    fn call<T>(&self, call: &str, f: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        match &self.plan {
            Some(plan) => plan.call(call, f),
            None => f(),
        }
    }
    fn figure(&self, call: &str, f: impl FnOnce() -> io::Result<u64>) -> io::Result<u64> {
        match &self.plan {
            Some(plan) => plan.figure(call, f),
            None => f(),
        }
    }
}
impl crate::start::spool::FileSystem for Files {
    fn open(&self, root: &File, name: &CStr, flags: i32) -> io::Result<File> {
        self.call("spool.open", || StartHost.open(root, name, flags))
    }
    fn in_memory(&self, root: &File) -> io::Result<bool> {
        self.call("spool.in_memory", || StartHost.in_memory(root))
    }
    fn available(&self, root: &File) -> io::Result<u64> {
        self.figure("spool.available", || StartHost.available(root))
    }
    fn cleanup(&self, root: &File, stopped: &dyn Fn() -> bool) -> Result<(), spool::Error> {
        self.call("spool.cleanup", || Ok(()))
            .map_err(spool::Error::HostIo)?;
        StartHost.cleanup(root, stopped)
    }
}
impl crate::sink::fs::FileSystem for Files {
    fn release(&self) -> io::Result<(u32, u32)> {
        let Some(root) = &self.root else {
            return SinkHost.release();
        };
        let text = std::fs::read_to_string(root.join("release"))?;
        let mut parts = text.trim().split('.').map(str::parse::<u32>);
        match (parts.next(), parts.next()) {
            (Some(Ok(major)), Some(Ok(minor))) => Ok((major, minor)),
            _ => Err(io::ErrorKind::InvalidData.into()),
        }
    }
    fn probe(&self, root: &File) -> io::Result<()> {
        self.call("sink.probe", || SinkHost.probe(root))
    }
    fn mount(&self, file: &File) -> io::Result<Option<u64>> {
        self.call("sink.mount", || SinkHost.mount(file))
    }
    fn entry(&self, parent: &File, name: &CStr) -> io::Result<crate::sys::Entry> {
        self.call("sink.entry", || SinkHost.entry(parent, name))
    }
    fn open(&self, parent: &File, name: &CStr, flags: i32, beneath: bool) -> io::Result<File> {
        self.call("sink.open", || SinkHost.open(parent, name, flags, beneath))
    }
    fn reopen(&self, file: &File) -> io::Result<File> {
        self.call("sink.reopen", || SinkHost.reopen(file))
    }
    fn mkdir(&self, parent: &File, name: &CStr) -> io::Result<()> {
        self.call("sink.mkdir", || SinkHost.mkdir(parent, name))
    }
    fn unlink(&self, parent: &File, name: &CStr) -> io::Result<()> {
        self.call("sink.unlink", || SinkHost.unlink(parent, name))
    }
    fn rename(&self, parent: &File, from: &CStr, to: &CStr) -> io::Result<()> {
        self.call("sink.rename", || SinkHost.rename(parent, from, to))
    }
    fn lock(&self, file: File) -> io::Result<crate::sys::FlockLease> {
        self.call("sink.lock", || SinkHost.lock(file))
    }
    fn write(&self, file: &File, bytes: &[u8]) -> io::Result<usize> {
        self.call("sink.write", || SinkHost.write(file, bytes))
    }
    fn read(&self, file: &File, bytes: &mut [u8]) -> io::Result<usize> {
        self.call("sink.read", || SinkHost.read(file, bytes))
    }
    fn sync_file(&self, file: &File) -> io::Result<()> {
        self.call("sink.sync_file", || SinkHost.sync_file(file))
    }
    fn sync_directory(&self, file: &File) -> io::Result<()> {
        self.call("sink.sync_directory", || SinkHost.sync_directory(file))
    }
    fn writeback(&self, file: &File) -> io::Result<()> {
        self.call("sink.writeback", || SinkHost.writeback(file))
    }
    fn syncfs(&self, lock: &File) -> io::Result<()> {
        self.call("sink.syncfs", || SinkHost.syncfs(lock))
    }
}
impl spool::FileSystem for Files {
    fn create_random(&self, path: &Path) -> io::Result<Box<dyn spool::random::Random>> {
        self.call("started.create_random", || SpoolHost.create_random(path))
    }
    fn open_random(&self, path: &Path) -> io::Result<Box<dyn spool::random::Random>> {
        self.call("started.open_random", || SpoolHost.open_random(path))
    }
    fn stat_directory(&self, path: &Path) -> io::Result<()> {
        self.call("started.stat_directory", || SpoolHost.stat_directory(path))
    }
    fn open(&self, path: &Path) -> io::Result<Box<dyn std::io::Read + Send>> {
        self.call("started.open", || SpoolHost.open(path))
    }
    fn length(&self, path: &Path) -> io::Result<u64> {
        self.call("started.length", || SpoolHost.length(path))
    }
    fn available(&self, directory: &Path) -> io::Result<u64> {
        self.figure("started.available", || SpoolHost.available(directory))
    }
    fn create(&self, path: &Path) -> io::Result<Box<dyn spool::Writer>> {
        self.call("started.create", || SpoolHost.create(path))
    }
    fn mkdir(&self, path: &Path) -> io::Result<()> {
        self.call("started.mkdir", || SpoolHost.mkdir(path))
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        self.call("started.rename", || SpoolHost.rename(from, to))
    }
    fn remove(&self, path: &Path) -> io::Result<()> {
        self.call("started.remove", || SpoolHost.remove(path))
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.call("started.remove_file", || SpoolHost.remove_file(path))
    }
}
/// Step 6's host facts read at a root of the test's.
pub struct Rooted(pub PathBuf);
impl Host for Rooted {
    fn lines(&self, path: &str, visit: &mut dyn FnMut(&str) -> io::Result<()>) -> io::Result<()> {
        let text = std::fs::read_to_string(self.0.join(path.trim_start_matches('/')))?;
        for line in text.lines() {
            visit(line)?;
        }
        Ok(())
    }
    fn limit(&self, resource: Resource) -> io::Result<Limit> {
        System.limit(resource)
    }
    fn raise(&self, resource: Resource, limit: Limit) -> io::Result<()> {
        System.raise(resource, limit)
    }
    fn privileged(&self) -> io::Result<bool> {
        if self.0.join("capget").exists() {
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        }
        System.privileged()
    }
}
