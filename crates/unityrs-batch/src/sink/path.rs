//! Pinned parents, revalidated from the root before create and commit.
use super::{Class, Context, Error, fs::FileSystem};
use std::{ffi::CStr, fs::File, os::unix::fs::MetadataExt, path::Path, sync::Arc};

pub struct Root {
    file: File,
    mount: Option<u64>,
    beneath: bool,
    fs: Arc<dyn FileSystem>,
}
impl Root {
    /// The root must already exist. Fallback is selected only by the probe's
    /// `ENOSYS` or `EPERM`, never by a subsequent ordinary open failure.
    pub fn open(path: &Path, fs: Arc<dyn FileSystem>) -> Result<Self, Error> {
        let cx = Context::Preflight;
        if fs.release().map_err(|e| Error::io(cx, "uname", &e))? < (5, 8) {
            return Err(Error::plain(Class::KernelTooOld, "uname"));
        }
        let file = crate::sys::open_directory(path).map_err(|e| Error::io(cx, "open", &e))?;
        let beneath = match fs.probe(&file) {
            Ok(()) => true,
            Err(e) if matches!(e.raw_os_error(), Some(libc::ENOSYS | libc::EPERM)) => false,
            Err(e) => return Err(Error::io(cx, "openat2", &e)),
        };
        let mount = match fs.mount(&file) {
            Ok(id) if beneath || id.is_some() => id,
            _ => return Err(Error::plain(Class::MountCheckUnavailable, "statx")),
        };
        Ok(Self {
            file,
            mount,
            beneath,
            fs,
        })
    }
    pub fn parent(&self, key: &str, create: bool, cx: Context) -> Result<Parent<'_>, Error> {
        validate(key)?;
        let file = self.resolve_parent(key, create, cx)?;
        Parent::new(file, self, key)
    }
    fn resolve_parent(&self, key: &str, create: bool, cx: Context) -> Result<File, Error> {
        let mut parts = key.split('/').peekable();
        let mut end = 0;
        let mut current = self
            .file
            .try_clone()
            .map_err(|e| Error::io(cx, "dup", &e))?;
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                return Ok(current);
            }
            end += part.len();
            let prefix = key
                .get(..end)
                .ok_or_else(|| Error::plain(Class::InvalidPath, "parent"))?;
            current = self.directory(&current, prefix, create, cx)?;
            end += 1;
        }
        Err(Error::plain(Class::InvalidPath, "key"))
    }
    pub(crate) fn directory(
        &self,
        parent: &File,
        key: &str,
        create: bool,
        cx: Context,
    ) -> Result<File, Error> {
        let part = key
            .rsplit('/')
            .next()
            .ok_or_else(|| Error::plain(Class::InvalidPath, "parent"))?;
        let name = Name::new(part)?;
        let flags = crate::sys::traversal_flags();
        let open = || self.fs.open(parent, name.cstr(), flags, self.beneath);
        let file = match open() {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                self.revalidate(parent, key, cx)?;
                match self.fs.mkdir(parent, name.cstr()) {
                    Ok(()) => {}
                    Err(e) if e.raw_os_error() == Some(libc::EEXIST) => {}
                    Err(e) => return Err(Error::io(cx, "mkdirat", &e)),
                }
                open().map_err(|e| Error::io(cx, "openat", &e))?
            }
            Err(e) => return Err(Error::io(cx, "openat", &e)),
        };
        // Preflight syncs the directory holding each head ancestor every
        // time, found or made: an earlier preflight may have created the
        // entry and stopped before its sync succeeded.
        if create && cx == Context::Preflight {
            let readable = self.readable(parent, cx)?;
            self.fs
                .sync_directory(&readable)
                .map_err(|e| Error::io(cx, "fsync_dir", &e))?;
        }
        if !self.beneath
            && (self
                .fs
                .mount(&file)
                .map_err(|e| Error::io(cx, "statx", &e))?
                != self.mount)
        {
            return Err(Error::io(
                cx,
                "statx",
                &std::io::Error::from_raw_os_error(libc::EXDEV),
            ));
        }
        Ok(file)
    }
    pub(crate) fn readable(&self, dir: &File, cx: Context) -> Result<File, Error> {
        self.child(
            dir,
            c".",
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            cx,
        )
    }
    pub(crate) fn child(
        &self,
        parent: &File,
        name: &CStr,
        flags: i32,
        cx: Context,
    ) -> Result<File, Error> {
        let file = self
            .fs
            .open(parent, name, flags, self.beneath)
            .map_err(|e| Error::io(cx, "openat", &e))?;
        if !self.beneath
            && self
                .fs
                .mount(&file)
                .map_err(|e| Error::io(cx, "statx", &e))?
                != self.mount
        {
            return Err(Error::io(
                cx,
                "statx",
                &std::io::Error::from_raw_os_error(libc::EXDEV),
            ));
        }
        Ok(file)
    }
    pub(crate) fn matches_mount(&self, mount: Option<u64>) -> bool {
        mount == self.mount
    }
    pub(crate) fn file(&self) -> &File {
        &self.file
    }
    pub(crate) fn fs(&self) -> &dyn FileSystem {
        &*self.fs
    }
    pub(crate) fn revalidate(&self, parent: &File, key: &str, cx: Context) -> Result<(), Error> {
        let current = if self.beneath {
            if let Some((path, _)) = key.rsplit_once('/') {
                let mut bytes = Vec::new();
                bytes
                    .try_reserve_exact(path.len() + 1)
                    .map_err(|_| Error::plain(Class::HostMemory, "parent"))?;
                bytes.extend_from_slice(path.as_bytes());
                bytes.push(0);
                let name = CStr::from_bytes_with_nul(&bytes)
                    .map_err(|_| Error::plain(Class::InvalidPath, "parent"))?;
                self.child(self.file(), name, crate::sys::traversal_flags(), cx)
            } else {
                self.file.try_clone().map_err(|e| Error::io(cx, "dup", &e))
            }
        } else {
            self.resolve_parent(key, false, cx)
        }
        .map_err(|e| {
            if e.errno == Some(libc::ENOENT) {
                moved(cx)
            } else {
                e
            }
        })?;
        let held = parent.metadata().map_err(|e| Error::io(cx, "fstat", &e))?;
        let found = current.metadata().map_err(|e| Error::io(cx, "fstat", &e))?;
        if (held.dev(), held.ino()) != (found.dev(), found.ino()) {
            return Err(moved(cx));
        }
        Ok(())
    }
}
/// A pinned parent must still match its root-relative path before each create
/// and commit rename. Concurrent external renames inside the owned root are
/// detected at these points only; callers must retain exclusive ownership.
pub struct Parent<'a> {
    pub(crate) file: File,
    pub(crate) root: &'a Root,
    key: String,
}
impl<'a> Parent<'a> {
    pub(crate) fn new(file: File, root: &'a Root, key: &str) -> Result<Self, Error> {
        let mut owned = String::new();
        owned
            .try_reserve_exact(key.len())
            .map_err(|_| Error::plain(Class::HostMemory, "parent"))?;
        owned.push_str(key);
        Ok(Self {
            file,
            root,
            key: owned,
        })
    }
    fn revalidate(&self, cx: Context) -> Result<(), Error> {
        self.root.revalidate(&self.file, &self.key, cx)
    }
    pub fn temporary(
        &self,
        run_id: &str,
        ordinal: u64,
        cx: Context,
    ) -> Result<Temporary<'_>, Error> {
        valid_run(run_id)?;
        let mut name = Name::new(".unityrs-tmp-")?;
        name.append(run_id.as_bytes())?;
        name.append(b"-")?;
        let mut digits = [b'0'; 20];
        let mut n = ordinal;
        let mut at = digits.len();
        loop {
            at -= 1;
            *digits
                .get_mut(at)
                .ok_or_else(|| Error::plain(Class::InvalidPath, "ordinal"))? = b'0'
                + u8::try_from(n % 10).map_err(|_| Error::plain(Class::InvalidPath, "ordinal"))?;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        name.append(
            digits
                .get(at..)
                .ok_or_else(|| Error::plain(Class::InvalidPath, "ordinal"))?,
        )?;
        self.revalidate(cx)?;
        match self.root.fs().unlink(&self.file, name.cstr()) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Error::io(cx, "unlinkat", &e)),
        }
        self.revalidate(cx)?;
        let file = self.root.child(
            &self.file,
            name.cstr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
            cx,
        )?;
        Ok(Temporary {
            file,
            parent: self,
            name,
            cx,
        })
    }
}
pub struct Temporary<'a> {
    file: File,
    parent: &'a Parent<'a>,
    name: Name,
    cx: Context,
}
impl Temporary<'_> {
    pub fn write_all(&self, mut bytes: &[u8]) -> Result<(), Error> {
        while !bytes.is_empty() {
            let n =
                crate::sys::retry_interrupted(|| self.parent.root.fs().write(&self.file, bytes))
                    .map_err(|e| Error::io(self.cx, "write", &e))?;
            if n == 0 || n > bytes.len() {
                return Err(Error::io(
                    self.cx,
                    "write",
                    &std::io::Error::from(std::io::ErrorKind::WriteZero),
                ));
            }
            bytes = bytes
                .get(n..)
                .ok_or_else(|| Error::plain(Class::InvalidPath, "write"))?;
        }
        Ok(())
    }
    /// Head protocol: temp `fsync`, rename, parent-directory `fsync`.
    /// Any failure leaves acknowledgment to the caller; a rename may already
    /// have happened even when its adapter reports failure.
    // Only Directory::write_head uses this helper. Its parent descriptor
    // duplicates the readable run directory retained since preflight, sharing
    // its writeback-error cursor. Never reopen that directory after rename.
    pub(crate) fn head_commit(self, basename: &str) -> Result<(), Error> {
        self.parent
            .root
            .fs()
            .sync_file(&self.file)
            .map_err(|e| Error::io(self.cx, "fsync", &e))?;
        self.rename(basename)?;
        self.parent
            .root
            .fs()
            .sync_directory(&self.parent.file)
            .map_err(|e| Error::io(self.cx, "fsync_dir", &e))
    }
    /// Loose-output protocol starts writeback; the publisher must hold its
    /// commit gate around `rename` and the outcome write. Final `syncfs` is
    /// separate. There is no per-output `fsync`.
    pub fn writeback(&self) -> Result<(), Error> {
        self.parent
            .root
            .fs()
            .writeback(&self.file)
            .map_err(|e| Error::io(self.cx, "sync_file_range", &e))
    }
    /// Best-effort removal of this temp after a sink failure. Errors are
    /// ignored: the name may already be gone (an uncertain rename), and a
    /// retry's create of the same ordinal unlinks it first.
    pub fn discard(&self) {
        let _ = self
            .parent
            .root
            .fs()
            .unlink(&self.parent.file, self.name.cstr());
    }
    pub fn rename(&self, basename: &str) -> Result<(), Error> {
        let to = Name::new(basename)?;
        self.parent.revalidate(self.cx)?;
        self.parent
            .root
            .fs()
            .rename(&self.parent.file, self.name.cstr(), to.cstr())
            .map_err(|e| Error::io(self.cx, "renameat", &e))
    }
}
fn moved(cx: Context) -> Error {
    Error::io(
        cx,
        "openat",
        &std::io::Error::from_raw_os_error(libc::ENOTDIR),
    )
}
fn validate(key: &str) -> Result<(), Error> {
    if key.is_empty()
        || key.len() > 1024
        || key
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == ".." || s.len() > 255 || s.contains('\0'))
    {
        return Err(Error::plain(Class::InvalidPath, "key"));
    }
    Ok(())
}
pub(crate) fn valid_run(run: &str) -> Result<(), Error> {
    if run.is_empty()
        || run.len() > 64
        || run == "."
        || run == ".."
        || !run
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err(Error::plain(Class::InvalidPath, "run_id"));
    }
    Ok(())
}
pub(crate) struct Name {
    bytes: [u8; 256],
    len: usize,
}
impl Name {
    pub(crate) fn new(name: &str) -> Result<Self, Error> {
        let mut out = Self {
            bytes: [0; 256],
            len: 0,
        };
        out.append(name.as_bytes())?;
        if name.is_empty() || name == "." || name == ".." || name.contains('/') {
            return Err(Error::plain(Class::InvalidPath, "basename"));
        }
        Ok(out)
    }
    fn append(&mut self, bytes: &[u8]) -> Result<(), Error> {
        let end = self.len + bytes.len();
        if end >= self.bytes.len() || bytes.contains(&0) {
            return Err(Error::plain(Class::InvalidPath, "basename"));
        }
        self.bytes
            .get_mut(self.len..end)
            .ok_or_else(|| Error::plain(Class::InvalidPath, "basename"))?
            .copy_from_slice(bytes);
        self.len = end;
        Ok(())
    }
    pub(crate) fn cstr(&self) -> &CStr {
        CStr::from_bytes_until_nul(&self.bytes).unwrap_or(c"invalid")
    }
}
