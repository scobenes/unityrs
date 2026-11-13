//! Root-wide lock and attempt fence. Keep this owner alive through finishing.
use super::{
    Class, Context, Error,
    fs::FileSystem,
    path::{Name, Root, valid_run},
};
use crate::manifest::head::{self, HEAD_LIMIT, Unreadable};
use std::{fs::File, path::Path, sync::Arc};

pub struct Directory {
    root: Root,
    lock: File,
    run: File,
    run_id: Name,
}
impl Directory {
    /// Caller provides at least `HEAD_LIMIT + 1` bytes, charged to startup's
    /// fixed scratch (`C_base`), reused for reads. Supersession creates no
    /// attempt row directory or output file.
    pub fn preflight(
        root: &Path,
        run_id: &str,
        attempt: u32,
        fs: Arc<dyn FileSystem>,
        scratch: &mut [u8],
    ) -> Result<Self, Error> {
        valid_run(run_id)?;
        if attempt == 0 || attempt > i32::MAX.unsigned_abs() || scratch.len() < HEAD_LIMIT + 1 {
            return Err(Error::plain(Class::InvalidPath, "preflight"));
        }
        let root = Root::open(root, fs)?;
        let cx = Context::Preflight;
        let meta = root.directory(root.file(), "_unityrs", true, cx)?;
        root.revalidate(&meta, "_unityrs/lock", cx)?;
        let lock = root.child(
            &meta,
            c"lock",
            libc::O_RDWR | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            cx,
        )?;
        if !lock
            .metadata()
            .map_err(|e| Error::io(cx, "fstat", &e))?
            .is_file()
        {
            return Err(Error::plain(Class::SinkRootUnusable, "fstat"));
        }
        root.fs().lock(&lock).map_err(|e| {
            if e.kind() == std::io::ErrorKind::WouldBlock {
                Error::plain(Class::OutputLocked, "flock")
            } else {
                Error::io(cx, "flock", &e)
            }
        })?;
        let mut run_key = String::new();
        run_key
            .try_reserve_exact(run_id.len() + 9)
            .map_err(|_| Error::plain(Class::HostMemory, "parent"))?;
        run_key.push_str("_unityrs/");
        run_key.push_str(run_id);
        let run = root.directory(&meta, &run_key, true, cx)?;
        let run = root.readable(&run, cx)?;
        let existing = match root.child(
            &run,
            c"manifest.json",
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            cx,
        ) {
            Ok(file) => Some(file),
            Err(e) if e.errno == Some(libc::ENOENT) => None,
            Err(e) => return Err(e),
        };
        if let Some(file) = existing {
            if !file
                .metadata()
                .map_err(|e| Error::io(cx, "fstat", &e))?
                .is_file()
            {
                return Err(Error::plain(Class::SinkRootUnusable, "fstat"));
            }
            let scratch = scratch
                .get_mut(..HEAD_LIMIT + 1)
                .ok_or_else(|| Error::plain(Class::InvalidPath, "scratch"))?;
            let mut used = 0;
            while used < scratch.len() {
                let bytes = scratch
                    .get_mut(used..)
                    .ok_or_else(|| Error::plain(Class::InvalidPath, "scratch"))?;
                let n = crate::sys::retry_interrupted(|| root.fs().read(&file, bytes))
                    .map_err(|e| Error::io(cx, "read", &e))?;
                if n == 0 {
                    break;
                }
                if n > bytes.len() {
                    return Err(Error::plain(Class::SinkRootUnusable, "read"));
                }
                used += n;
            }
            let identity = head::recognize(
                scratch
                    .get(..used)
                    .ok_or_else(|| Error::plain(Class::InvalidPath, "scratch"))?,
                run_id,
            )
            .map_err(|e| {
                Error::plain(
                    Class::ManifestUnreadable,
                    match e {
                        Unreadable::TooLarge => "too_large",
                        Unreadable::NotAManifest => "not_a_manifest",
                    },
                )
            })?;
            if identity.attempt >= attempt {
                return Err(Error::plain(Class::Superseded, "head"));
            }
        }
        Ok(Self {
            root,
            lock,
            run,
            run_id: Name::new(run_id)?,
        })
    }
    /// Writes already encoded head bytes. B7b supplies the typed encoder.
    pub fn write_head(&self, bytes: &[u8], running: bool) -> Result<(), Error> {
        let cx = if running {
            Context::RunningHead
        } else {
            Context::Finish
        };
        if bytes.len() >= 64 * 1024 {
            return Err(Error::plain(Class::InvalidPath, "head_size"));
        }
        let run_id = self
            .run_id
            .cstr()
            .to_str()
            .map_err(|_| Error::plain(Class::InvalidPath, "run_id"))?;
        let mut key = String::new();
        key.try_reserve_exact(run_id.len() + 23)
            .map_err(|_| Error::plain(Class::HostMemory, "parent"))?;
        key.push_str("_unityrs/");
        key.push_str(run_id);
        key.push_str("/manifest.json");
        let parent = super::path::Parent::new(
            self.run.try_clone().map_err(|e| Error::io(cx, "dup", &e))?,
            &self.root,
            &key,
        )?;
        let temp = parent.temporary(run_id, 0, cx)?;
        temp.write_all(bytes)?;
        temp.head_commit("manifest.json")
    }
    #[must_use]
    pub fn root(&self) -> &Root {
        &self.root
    }
    /// Use the original lock descriptor, preserving its writeback-error cursor.
    pub fn syncfs(&self) -> Result<(), Error> {
        self.root
            .fs()
            .syncfs(&self.lock)
            .map_err(|e| Error::io(Context::Output, "syncfs", &e))
    }
}
