//! Shared real-filesystem adapter with deterministic startup faults.
use std::{
    ffi::CStr,
    fs::File,
    io,
    os::unix::fs::{MetadataExt, symlink},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use unityrs_batch::{
    manifest::head::HEAD_LIMIT,
    sink::{directory::Directory, fs::FileSystem},
    sys,
};
use unityrs_testkit::scratch::Scratch;

#[derive(Default)]
#[allow(clippy::struct_excessive_bools)] // Independent, combinable fault switches.
pub struct Faults {
    pub fallback: Option<i32>,
    pub release: Option<(u32, u32)>,
    pub missing_mount_at: AtomicUsize,
    pub fail: Option<(&'static str, i32)>,
    /// While set, `fail` is not injected: a test arms it after preflight,
    /// whose own ancestor syncs would otherwise meet the fault first.
    pub disarmed: std::sync::atomic::AtomicBool,
    pub after_rename: bool,
    pub native_temp: bool,
    pub short_write: bool,
    pub interrupt: bool,
    pub missing_mount: bool,
    pub mount_error: bool,
    pub crossing: AtomicUsize,
    pub mounts: AtomicUsize,
    pub calls: Mutex<Vec<&'static str>>,
    pub synced: Mutex<Vec<u64>>,
    pub race: Mutex<Option<(PathBuf, PathBuf, &'static str)>>,
    /// A folding sink on any host: the second name opens and unlinks the
    /// first's entry, so an exclusive create of it fails with `EEXIST`, or,
    /// with `fold_opens`, opens the existing entry (one inode for both).
    pub fold: Option<(&'static CStr, &'static CStr)>,
    pub fold_opens: bool,
    /// Preflight's case and normalization probe runs only when set: the
    /// adapter fakes the kernel release to run on any host, a folding one
    /// included (`FileSystem::probe_sink_semantics`).
    pub semantics: bool,
}
impl Faults {
    fn point(&self, name: &'static str) -> io::Result<()> {
        self.calls.lock().unwrap().push(name);
        if let Some((call, errno)) = self.fail
            && call == name
            && !self.disarmed.load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(io::Error::from_raw_os_error(errno));
        }
        Ok(())
    }
    fn folded<'a>(&self, name: &'a CStr, flags: i32) -> (&'a CStr, i32) {
        match self.fold {
            Some((first, second)) if name == second => (
                first,
                if self.fold_opens {
                    flags & !libc::O_EXCL
                } else {
                    flags
                },
            ),
            _ => (name, flags),
        }
    }
    fn replace_resume(&self, point: &str) -> io::Result<()> {
        let mut race = self.race.lock().unwrap();
        if race.as_ref().is_some_and(|r| r.2 == point) {
            let (from, to, _) = race.take().unwrap();
            std::fs::rename(to, from)?;
        }
        Ok(())
    }
}
impl FileSystem for Faults {
    fn release(&self) -> io::Result<(u32, u32)> {
        self.point("uname")?;
        Ok(self.release.unwrap_or((5, 8)))
    }
    fn probe_sink_semantics(&self) -> bool {
        self.semantics
    }
    fn probe(&self, root: &File) -> io::Result<()> {
        self.point("probe")?;
        self.fallback.map_or_else(
            || sys::beneath_probe(root),
            |n| Err(io::Error::from_raw_os_error(n)),
        )
    }
    fn entry(
        &self,
        parent: &std::fs::File,
        name: &std::ffi::CStr,
    ) -> std::io::Result<unityrs_batch::sys::Entry> {
        let mut entry = unityrs_batch::sys::rooted_entry(parent, name)?;
        entry.mount = self.mount(parent)?;
        self.replace_resume("resume_checked")?;
        Ok(entry)
    }
    fn mount(&self, file: &File) -> io::Result<Option<u64>> {
        self.point("mount")?;
        if self.mount_error {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        let call = self.mounts.fetch_add(1, Ordering::SeqCst);
        if self.missing_mount
            || (self.missing_mount_at.load(Ordering::SeqCst) != 0
                && call == self.missing_mount_at.load(Ordering::SeqCst))
        {
            return Ok(None);
        }
        let id = file.metadata()?.dev();
        Ok(Some(if call == self.crossing.load(Ordering::SeqCst) {
            id + 1
        } else {
            id
        }))
    }
    fn open(&self, dir: &File, name: &CStr, flags: i32, beneath: bool) -> io::Result<File> {
        self.point("open")?;
        let (name, flags) = self.folded(name, flags);
        if name == c"key" && flags & libc::O_CREAT == 0 {
            // A substituted device must never receive a readable pathname open.
            #[cfg(target_os = "linux")]
            assert_eq!(
                flags & (libc::O_PATH | libc::O_NOFOLLOW),
                libc::O_PATH | libc::O_NOFOLLOW
            );
            self.replace_resume("resume_open")?;
        }
        if self.native_temp && name.to_bytes().starts_with(b".unityrs-tmp-") && !beneath {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        let race = self.race.lock().unwrap().take();
        if let Some((from, to, point)) = race {
            if point == "lookup_move" && matches!(name.to_bytes(), b"child" | b"r") {
                std::fs::rename(from, to)?;
            } else if point == "metadata_open" && name == c"_unityrs" {
                let file = sys::rooted_open(dir, name, flags, beneath)?;
                std::fs::rename(from, to)?;
                return Ok(file);
            } else if point == "temp_create" && name.to_bytes().starts_with(b".unityrs-tmp-") {
                symlink(to, from)?;
            } else {
                *self.race.lock().unwrap() = Some((from, to, point));
            }
        }
        #[cfg(target_os = "linux")]
        {
            sys::rooted_open(dir, name, flags, beneath)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = beneath;
            sys::rooted_open(dir, name, flags, false)
        }
    }
    fn reopen(&self, file: &File) -> io::Result<File> {
        self.point("reopen")?;
        let mut race = self.race.lock().unwrap();
        if race.as_ref().is_some_and(|r| r.2 == "resume_wrong_inode") {
            return File::open(race.take().unwrap().1);
        }
        drop(race);
        #[cfg(target_os = "linux")]
        self.replace_resume("resume_checked")?;
        self.replace_resume("resume_reopen")?;
        sys::reopen_pinned(file)
    }
    fn mkdir(&self, dir: &File, name: &CStr) -> io::Result<()> {
        self.point("mkdir")?;
        let race = self.race.lock().unwrap().take();
        if let Some((from, to, point)) = race {
            if point == "mkdir_symlink" {
                symlink(to, from)?;
                return Err(io::Error::from_raw_os_error(libc::EEXIST));
            }
            if point == "mkdir_directory" {
                sys::rooted_mkdir(dir, name)?;
                return Err(io::Error::from_raw_os_error(libc::EEXIST));
            }
            *self.race.lock().unwrap() = Some((from, to, point));
        }
        sys::rooted_mkdir(dir, name)
    }
    fn unlink(&self, dir: &File, name: &CStr) -> io::Result<()> {
        self.point("unlink")?;
        let (name, _) = self.folded(name, 0);
        let race = self.race.lock().unwrap().take();
        if let Some((from, to, point)) = race {
            if point == "unlink_move" {
                std::fs::rename(from, to)?;
            } else {
                *self.race.lock().unwrap() = Some((from, to, point));
            }
        }
        sys::rooted_unlink(dir, name)
    }
    fn rename(&self, dir: &File, from: &CStr, to: &CStr) -> io::Result<()> {
        if self.after_rename {
            sys::rooted_rename(dir, from, to)?;
        }
        self.point("rename")?;
        if self.after_rename {
            Ok(())
        } else {
            sys::rooted_rename(dir, from, to)
        }
    }
    fn write(&self, mut file: &File, bytes: &[u8]) -> io::Result<usize> {
        use std::io::Write;
        self.point("write")?;
        if self.interrupt
            && self
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|p| **p == "write")
                .count()
                == 1
        {
            return Err(io::Error::from_raw_os_error(libc::EINTR));
        }
        file.write(if self.short_write { &bytes[..1] } else { bytes })
    }
    fn read(&self, mut file: &File, bytes: &mut [u8]) -> io::Result<usize> {
        self.point("read")?;
        std::io::Read::read(&mut file, bytes)
    }
    fn sync_file(&self, file: &File) -> io::Result<()> {
        self.point("fsync")?;
        file.sync_all()
    }
    fn sync_directory(&self, file: &File) -> io::Result<()> {
        self.point("fsync_dir")?;
        self.synced.lock().unwrap().push(file.metadata()?.ino());
        file.sync_all()
    }
    fn writeback(&self, _: &File) -> io::Result<()> {
        self.point("writeback")
    }
    fn syncfs(&self, _: &File) -> io::Result<()> {
        self.point("syncfs")
    }
}
pub fn fs(fallback: bool) -> Arc<Faults> {
    Arc::new(Faults {
        fallback: fallback.then_some(libc::ENOSYS),
        crossing: AtomicUsize::new(usize::MAX),
        ..Faults::default()
    })
}
pub fn head(attempt: u32) -> Vec<u8> {
    format!(r#"{{"manifest_version":1,"run_id":"r","attempt":{attempt},"write_seq":0,"binary_version":"test","spec_version":1,"spec_sha256":"{}"}}"#, "a".repeat(64)).into_bytes()
}
pub fn preflight(
    path: &Path,
    attempt: u32,
    fs: Arc<Faults>,
) -> Result<Directory, unityrs_batch::sink::Error> {
    Directory::preflight(path, "r", attempt, fs, &mut vec![0; HEAD_LIMIT + 1])
}
/// Native assertions require the real syscall, including its sandbox policy.
pub fn native_available(path: &Path) -> bool {
    match sys::beneath_probe(&sys::open_directory(path).unwrap()) {
        Ok(()) => true,
        Err(e) if matches!(e.raw_os_error(), Some(libc::ENOSYS | libc::EPERM)) => {
            eprintln!("native openat2 assertions skipped: {e}");
            false
        }
        Err(e) => panic!("unexpected openat2 probe failure: {e}"),
    }
}
pub fn resolvers() -> Vec<bool> {
    let scratch = Scratch::new("directory-native-probe");
    if native_available(scratch.path()) {
        vec![false, true]
    } else {
        vec![true]
    }
}
