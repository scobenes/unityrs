//! Dedicated local spool startup, in `BATCH_SPEC` 6.2 order. No old file is
//! adopted into the new claim counter. Directory sinks have no matching store
//! endpoint/bucket, so old journals require no network abort work.
use super::Local;
use crate::{sink::path::Name, spool::Counter};
use std::{
    ffi::CStr,
    fs::File,
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    NotDedicated,
    InMemory,
    OverlapsSink,
    Locked,
    NoSpace,
    Unusable,
    SinkRootUnusable,
    HostIo,
    HostMemory,
    Stopped,
}
#[derive(Debug)]
pub struct Error {
    pub class: Class,
    pub call: &'static str,
    pub errno: Option<i32>,
}
impl Error {
    fn plain(class: Class, call: &'static str) -> Self {
        Self {
            class,
            call,
            errno: None,
        }
    }
    fn io(class: Class, call: &'static str, e: &io::Error) -> Self {
        Self {
            class: if e.kind() == io::ErrorKind::OutOfMemory {
                Class::HostMemory
            } else {
                class
            },
            call,
            errno: e.raw_os_error(),
        }
    }
}
pub trait FileSystem {
    fn open(&self, root: &File, name: &CStr, flags: i32) -> io::Result<File> {
        crate::sys::rooted_open(root, name, flags, false)
    }
    fn in_memory(&self, root: &File) -> io::Result<bool> {
        crate::sys::in_memory(root)
    }
    fn available(&self, root: &File) -> io::Result<u64> {
        crate::sys::available_at(root)
    }
    fn cleanup(&self, root: &File, stopped: &dyn Fn() -> bool) -> Result<(), crate::spool::Error> {
        crate::sys::remove_tree(root, Path::new("runs"), stopped)
    }
}
pub struct Host;
impl FileSystem for Host {}

/// The permanent lock is retained separately from all run artifacts.
pub struct Started {
    pub counter: Arc<Counter>,
    pub run: PathBuf,
    root: File,
    lock: File,
}
impl Started {
    #[must_use]
    pub fn root(&self) -> &File {
        &self.root
    }
    #[must_use]
    pub fn lock(&self) -> &File {
        &self.lock
    }
}
/// The locked descriptor `held` must be the directory `canonical` names,
/// checked without following a final symlink and with fallible path storage.
fn same_directory(held: &File, canonical: &Path) -> Result<(), Error> {
    let held = held
        .metadata()
        .map_err(|e| Error::io(Class::Unusable, "fstat", &e))?;
    let named = match crate::sys::directory_identity(canonical) {
        Ok(named) => named,
        Err(e) if e.kind() == io::ErrorKind::OutOfMemory => {
            return Err(Error::plain(Class::HostMemory, "lstat"));
        }
        Err(e) if e.kind() == io::ErrorKind::InvalidData => {
            return Err(Error::plain(Class::Unusable, "lstat"));
        }
        Err(e) => return Err(Error::io(Class::Unusable, "lstat", &e)),
    };
    if (held.dev(), held.ino()) == named {
        Ok(())
    } else {
        Err(Error::plain(Class::Unusable, "lstat"))
    }
}

/// Perform only local spool startup; create execution resources before calling
/// this function. `Local` proves unsupported capabilities were refused first.
pub fn start(
    local: &Local<'_>,
    fs: &dyn FileSystem,
    stopped: &dyn Fn() -> bool,
) -> Result<Started, Error> {
    let spec = local.spec;
    crate::sink::path::valid_run(&spec.run_id)
        .map_err(|_| Error::plain(Class::Unusable, "run_id"))?;
    let path = Path::new(&spec.spool.dir);
    let canonical_spool =
        crate::sys::canonical(path).map_err(|e| Error::io(Class::Unusable, "realpath", &e))?;
    let root = crate::sys::open_directory(&canonical_spool)
        .map_err(|e| Error::io(Class::Unusable, "open", &e))?;
    dedicated(&root)?;
    if fs
        .in_memory(&root)
        .map_err(|e| Error::io(Class::Unusable, "fstatfs", &e))?
    {
        return Err(Error::plain(Class::InMemory, "fstatfs"));
    }
    let canonical_sink = crate::sys::canonical(local.root())
        .map_err(|e| Error::io(Class::SinkRootUnusable, "realpath", &e))?;
    if canonical_spool.starts_with(&canonical_sink) || canonical_sink.starts_with(&canonical_spool)
    {
        return Err(Error::plain(Class::OverlapsSink, "realpath"));
    }
    // Dedicate before creating the permanent lock: a first-use crash may leave
    // a lone marker, which is safe to restart. A lone lock is never adopted.
    match fs.open(
        &root,
        c".unityrs-spool",
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
    ) {
        Ok(_) => {}
        Err(e) if e.raw_os_error() == Some(libc::EEXIST) => {}
        Err(e) => return Err(Error::io(Class::Unusable, "openat", &e)),
    }
    let lock = fs
        .open(
            &root,
            c"lock",
            libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
        .map_err(|e| Error::io(Class::Unusable, "openat", &e))?;
    if !lock
        .metadata()
        .map_err(|e| Error::io(Class::Unusable, "fstat", &e))?
        .is_file()
    {
        return Err(Error::plain(Class::NotDedicated, "lock"));
    }
    crate::sys::lock_exclusive(&lock).map_err(|e| {
        Error::io(
            if e.kind() == io::ErrorKind::WouldBlock {
                Class::Locked
            } else {
                Class::Unusable
            },
            "flock",
            &e,
        )
    })?;
    // Check the canonical pathname against the directory we actually locked.
    // Do this before cleanup; all later filesystem work uses the held root.
    same_directory(&root, &canonical_spool)?;
    // Recheck after acquiring the lock: another startup may have populated it.
    let has_runs = dedicated(&root)?;
    if stopped() {
        return Err(Error::plain(Class::Stopped, "cleanup"));
    }
    match if has_runs {
        fs.cleanup(&root, stopped)
    } else {
        Ok(())
    } {
        Ok(()) => {}
        Err(crate::spool::Error::Stopped) => return Err(Error::plain(Class::Stopped, "cleanup")),
        Err(crate::spool::Error::HostMemory) => {
            return Err(Error::plain(Class::HostMemory, "cleanup"));
        }
        Err(_) => return Err(Error::plain(Class::HostIo, "cleanup")),
    }
    if fs
        .available(&root)
        .map_err(|e| Error::io(Class::Unusable, "fstatvfs", &e))?
        < spec.spool.reserve_bytes
    {
        return Err(Error::plain(Class::NoSpace, "fstatvfs"));
    }
    let block = root
        .metadata()
        .map_err(|e| Error::io(Class::Unusable, "fstat", &e))?
        .blksize();
    let counter = Counter::new(spec.spool.size_bytes, spec.spool.reserve_bytes, block)
        .map_err(|_| Error::plain(Class::HostMemory, "counter"))?;
    let run = prepare_run(&root, &canonical_spool, &spec.run_id)?;
    Ok(Started {
        counter,
        run,
        root,
        lock,
    })
}
fn prepare_run(root: &File, canonical_spool: &Path, run_id: &str) -> Result<PathBuf, Error> {
    mkdir(root, c"runs")?;
    let runs = crate::sys::rooted_open(
        root,
        c"runs",
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        false,
    )
    .map_err(|e| Error::io(Class::Unusable, "openat", &e))?;
    let name = Name::new(run_id).map_err(|_| Error::plain(Class::Unusable, "run_id"))?;
    mkdir(&runs, name.cstr())?;
    let run = crate::sys::rooted_open(
        &runs,
        name.cstr(),
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
        false,
    )
    .map_err(|e| Error::io(Class::Unusable, "openat", &e))?;
    for name in [c"in", c"attempts", c"fail", c"done", c"winners"] {
        mkdir(&run, name)?;
    }
    let base = crate::spool::path::join(canonical_spool, Path::new("runs"))
        .map_err(|_| Error::plain(Class::HostMemory, "path"))?;
    let path = crate::spool::path::join(&base, Path::new(run_id))
        .map_err(|_| Error::plain(Class::HostMemory, "path"))?;
    Ok(path)
}
fn mkdir(root: &File, name: &CStr) -> Result<(), Error> {
    crate::sys::rooted_mkdir(root, name).map_err(|e| Error::io(Class::Unusable, "mkdirat", &e))
}
fn dedicated(root: &File) -> Result<bool, Error> {
    let (mut marker, mut entries, mut runs) = (false, 0, false);
    crate::sys::visit_directory(root, &mut |name| {
        entries += 1;
        let flags = match name.to_bytes() {
            b".unityrs-spool" => {
                marker = true;
                libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW
            }
            b"lock" => libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOFOLLOW,
            b"runs" => {
                runs = true;
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW
            }
            _ => return Err(io::Error::from(io::ErrorKind::InvalidData)),
        };
        let file = crate::sys::rooted_open(root, name, flags, false)?;
        let metadata = file.metadata()?;
        if name != c"runs" && !metadata.is_file() {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        Ok(())
    })
    .map_err(|e| {
        if matches!(e.kind(), io::ErrorKind::InvalidData)
            || matches!(e.raw_os_error(), Some(libc::ELOOP | libc::ENOTDIR))
        {
            Error::plain(Class::NotDedicated, "entries")
        } else {
            Error::io(Class::Unusable, "readdir", &e)
        }
    })?;
    if entries != 0 && !marker {
        return Err(Error::plain(Class::NotDedicated, "marker"));
    }
    Ok(runs)
}
