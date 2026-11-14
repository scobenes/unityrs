//! One closure owns the WHOLE scene: servers, FIFOs, scratch and engine. An
//! outside SIGKILL restarts that closure, even when the killed child caused an
//! assertion or a broken pipe. Deliberate kills are explicit and never retried.
//! Spawn and reap on the calling, long-lived thread: Linux PDEATHSIG follows
//! the spawning thread. Every child has its own process group; cleanup kills
//! the group before reaping the child, including namespace wrappers/services.
use crate::{
    guard::{self, DEADLOCK_GUARD},
    scratch::Scratch,
};
use std::{
    fs::{File, OpenOptions},
    io::{self, Read},
    os::unix::{fs::OpenOptionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus},
    sync::mpsc::{self, Receiver},
    thread::JoinHandle,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
pub struct Live(usize);
struct Owned {
    /// `None` for an adopted descendant ([`Scene::adopt`]), reaped by pid.
    child: Option<Child>,
    pid: u32,
    status: Option<ExitStatus>,
    deliberate: bool,
    service: bool,
    group: bool,
}
pub struct Scene {
    pub scratch: Scratch,
    children: Vec<Owned>,
    nested: bool,
}
/// Rebuild the complete scene after outside kills. Cleanup also runs on panic.
///
/// # Panics
/// Repeated outside SIGKILL or a scene assertion failure.
pub fn scene<T>(mut run: impl FnMut(&mut Scene) -> io::Result<T>) -> io::Result<T> {
    for attempt in 1..=super::KILLED_RUNS {
        let mut scene = Scene {
            scratch: Scratch::new("scene"),
            children: Vec::new(),
            nested: std::env::var_os("UNITYRS_TEST_INHERIT_GROUP").is_some(),
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&mut scene)));
        // Observe endings before deliberate cleanup can disguise an outside kill.
        let (outside, unfinished) = scene.finish()?;
        if outside {
            assert!(
                attempt < super::KILLED_RUNS,
                "scene killed by SIGKILL on every run: the system ended it from outside"
            );
            continue;
        }
        return match result {
            Ok(Ok(_)) if unfinished => Err(io::Error::other(
                "scene returned while its engine was still alive",
            )),
            Ok(value) => value,
            Err(panic) => std::panic::resume_unwind(panic),
        };
    }
    unreachable!("the final killed scene fails above")
}
/// Spawn an engine or service in the current scene. Use `service=true` only
/// for a process intentionally retained until scene cleanup. All streams must
/// be drained by the caller when piped; `drain` supplies a continuous reader.
pub fn live(scene: &mut Scene, cmd: &mut Command, service: bool) -> io::Result<Live> {
    if !cmd.get_envs().any(|(k, _)| k == crate::scratch::VAR) {
        cmd.env(crate::scratch::VAR, scene.scratch.path());
    }
    if !scene.nested {
        cmd.process_group(0);
    }
    cmd.env("UNITYRS_TEST_INHERIT_GROUP", "1");
    let child = cmd
        .spawn()
        .map_err(|e| io::Error::new(e.kind(), format!("scene spawn: {e}")))?;
    let token = Live(scene.children.len());
    scene.children.push(Owned {
        pid: child.id(),
        child: Some(child),
        status: None,
        deliberate: false,
        service,
        group: !scene.nested,
    });
    Ok(token)
}
impl Scene {
    #[must_use]
    pub fn id(&self, live: Live) -> u32 {
        self.children[live.0].pid
    }
    /// The child's piped stdin, to write to. The scene hands out only stdio
    /// handles, never the `Child`: observing, killing and reaping go through
    /// the scene, so no test can reap a root whose group cleanup then kills.
    pub fn stdin(&mut self, live: Live) -> Option<&mut ChildStdin> {
        self.children[live.0].child.as_mut()?.stdin.as_mut()
    }
    /// Take the piped stdin (dropping it closes the child's stdin).
    pub fn take_stdin(&mut self, live: Live) -> Option<ChildStdin> {
        self.children[live.0].child.as_mut()?.stdin.take()
    }
    /// Take the piped stdout, for [`drain`].
    pub fn take_stdout(&mut self, live: Live) -> Option<ChildStdout> {
        self.children[live.0].child.as_mut()?.stdout.take()
    }
    /// Take the piped stderr, for [`drain`].
    pub fn take_stderr(&mut self, live: Live) -> Option<ChildStderr> {
        self.children[live.0].child.as_mut()?.stderr.take()
    }
    /// Adopt a descendant re-parented to this process (a Linux subreaper's
    /// orphan): the scene observes, reaps and on cleanup kills it as one of
    /// its own, so its outside SIGKILL is the scene's retry case. Its group
    /// is never signalled, since it may be this process's own.
    ///
    /// # Errors
    /// `ECHILD` when `pid` is not (yet) this process's child.
    pub fn adopt(&mut self, pid: u32, service: bool) -> io::Result<Live> {
        sys::exited(pid).map_err(|e| io::Error::new(e.kind(), format!("scene adopt: {e}")))?;
        let token = Live(self.children.len());
        self.children.push(Owned {
            child: None,
            pid,
            status: None,
            deliberate: false,
            service,
            group: false,
        });
        Ok(token)
    }
    /// Observe/reap through the scene so descendants are killed before the
    /// root pid can be recycled.
    pub fn running(&mut self, live: Live) -> io::Result<bool> {
        let owned = &mut self.children[live.0];
        if owned.status.is_none()
            && sys::exited(owned.pid)
                .map_err(|e| io::Error::new(e.kind(), format!("scene waitid: {e}")))?
        {
            if owned.group {
                sys::cleanup_exited_group(owned.pid)?;
            }
            owned.status = Some(match &mut owned.child {
                Some(child) => child.wait()?,
                None => sys::reap(owned.pid)?,
            });
        }
        Ok(owned.status.is_none())
    }
    /// Wait for a filesystem mark (a file another process creates) while
    /// watching `watched`. One that ends first returns `BrokenPipe` at once,
    /// so an outside SIGKILL reaches scene arbitration, not the guard.
    ///
    /// # Panics
    /// When the mark outlasts the testkit deadlock guard.
    pub fn mark(&mut self, path: &Path, watched: &[Live]) -> io::Result<()> {
        let deadline = Instant::now() + DEADLOCK_GUARD;
        while !path.exists() {
            for live in watched {
                if !self.running(*live)? {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        format!("scene child ended before mark {}", path.display()),
                    ));
                }
            }
            assert!(
                Instant::now() < deadline,
                "deadlock guard: scene mark {} missing",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }
    pub fn signal(&mut self, live: Live, signal: i32) -> io::Result<()> {
        if signal == libc::SIGKILL {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        if !self.running(live)? {
            return Err(io::ErrorKind::NotFound.into());
        }
        sys::signal(self.id(live), signal, false)
    }
    /// A test's deliberate SIGKILL never counts as an outside kill.
    pub fn kill(&mut self, live: Live) -> io::Result<()> {
        if !self.running(live)? {
            return Ok(());
        }
        let owned = &mut self.children[live.0];
        owned.deliberate = true;
        sys::signal(owned.pid, libc::SIGKILL, owned.group)
    }
    /// # Panics
    /// When the child outlasts the testkit deadlock guard.
    pub fn wait(&mut self, live: Live) -> io::Result<ExitStatus> {
        let deadline = Instant::now() + DEADLOCK_GUARD;
        loop {
            if let Some(status) = self.children[live.0].status {
                return Ok(status);
            }
            if !self.running(live)? {
                continue;
            }
            assert!(
                Instant::now() < deadline,
                "deadlock guard: child did not exit"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn finish(&mut self) -> io::Result<(bool, bool)> {
        let mut outside = false;
        let mut unfinished = false;
        for i in 0..self.children.len() {
            let _ = self.running(Live(i))?;
            if self.children[i].status.is_none() {
                unfinished |= !self.children[i].service;
                // Failure paths must not wait ten minutes for a held engine.
                self.kill(Live(i))?;
            }
            let status = self.wait(Live(i))?;
            outside |= super::killed(status) && !self.children[i].deliberate;
        }
        Ok((outside, unfinished))
    }
}
impl Drop for Scene {
    fn drop(&mut self) {
        for owned in &mut self.children {
            if owned.status.is_none() {
                // Before reaping, this pid/group cannot name an unrelated run.
                let _ = sys::signal(owned.pid, libc::SIGKILL, owned.group);
                let _ = match &mut owned.child {
                    Some(child) => child.wait(),
                    None => sys::reap(owned.pid),
                };
            }
        }
    }
}
/// Continuously drain a pipe before waiting for the next protocol event.
#[must_use]
pub fn drain(mut input: impl Read + Send + 'static) -> (Receiver<Vec<u8>>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        loop {
            let mut bytes = vec![0; 4096];
            match input.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => {
                    bytes.truncate(n);
                    // Dropping the observer must not close the child's pipe or
                    // turn its later diagnostics into EPIPE. Drain until EOF even
                    // when the scene only wanted an early protocol observation.
                    let _ = tx.send(bytes);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    });
    (rx, thread)
}
/// A fresh scene-local FIFO opened before engine spawn. Its dedicated reader
/// drains continuously, preserving partial lines; no stale file can satisfy a
/// later scene's wait. Drop ends the reader without waiting for engine exit.
pub struct Fifo {
    path: PathBuf,
    rx: Receiver<String>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Fifo {
    pub fn new(scene: &Scene, name: &str) -> io::Result<Self> {
        let path = scene.scratch.path().join(name);
        sys::fifo(&path).map_err(|e| io::Error::new(e.kind(), format!("scene mkfifo: {e}")))?;
        let mut file: File = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&path)?;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopping = stop.clone();
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let mut pending = Vec::new();
            let mut bytes = [0; 4096];
            while !stopping.load(std::sync::atomic::Ordering::Acquire) {
                match file.read(&mut bytes) {
                    Ok(n) => {
                        for b in &bytes[..n] {
                            if *b == b'\n' {
                                if tx
                                    .send(String::from_utf8_lossy(&pending).into_owned())
                                    .is_err()
                                {
                                    return;
                                }
                                pending.clear();
                            } else {
                                pending.push(*b);
                            }
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(_) => return,
                }
            }
        });
        Ok(Self {
            path,
            rx,
            stop,
            thread: Some(thread),
        })
    }
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
    #[must_use]
    pub fn line(&self) -> String {
        guard::recv(&self.rx, "fresh lifecycle FIFO line")
    }
    /// Wait for a trace while also observing child death. An outside SIGKILL
    /// returns to scene arbitration immediately instead of spending the guard
    /// waiting on a FIFO whose read/write keeper intentionally prevents EOF.
    ///
    /// # Panics
    /// When no line arrives before the testkit deadlock guard.
    pub fn line_while(&self, scene: &mut Scene, live: Live) -> io::Result<String> {
        let deadline = Instant::now() + DEADLOCK_GUARD;
        loop {
            match self.rx.recv_timeout(Duration::from_millis(1)) {
                Ok(text) => return Ok(text),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if !scene.running(live)? {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            assert!(
                Instant::now() < deadline,
                "deadlock guard: lifecycle FIFO line missing"
            );
        }
    }
}
impl Drop for Fifo {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
#[path = "sys.rs"]
mod sys;

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
