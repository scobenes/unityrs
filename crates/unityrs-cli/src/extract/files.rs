//! Descriptor-relative output. Never follow an output symlink, and replace
//! leaves by rename so a preexisting hard link cannot change an outside file.
//! All untrusted C strings use a charged, fallibly allocated byte buffer.
use std::{
    fs::File,
    io,
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::ffi::OsStrExt,
    },
    path::{Component, Path},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_export::extractor::{Error, Sink, paths};
use unityrs_mesh::obj::{ChargedBytes, MeteredWrite};
pub(crate) const MAX_OUTPUT_BYTES: usize = 4096;
pub(crate) const MAX_OUTPUT_COMPONENTS: usize = 64;
const DIRECTORY_WORK: u64 = 64;

pub(crate) fn output_limit(path: &[u8]) -> Result<(), &'static str> {
    if path.len() > MAX_OUTPUT_BYTES {
        return Err("output limit: 4096 bytes");
    }
    if path
        .split(|b| *b == b'/')
        .filter(|s| !s.is_empty() && *s != b".")
        .count()
        > MAX_OUTPUT_COMPONENTS
    {
        return Err("output limit: 64 components");
    }
    Ok(())
}
fn clone_directory(file: &File, meter: &Meter) -> Result<File, Error> {
    meter.count_decode(DIRECTORY_WORK)?;
    Ok(file.try_clone()?)
}
static NEXT: AtomicU64 = AtomicU64::new(0);
pub(crate) struct Files {
    root: File,
    meter: Arc<Meter>,
}
fn nul(name: &[u8], meter: &Arc<Meter>) -> Result<MeteredVec<u8>, Error> {
    if name.contains(&0) {
        return Err(Error::UnsafePath);
    }
    let mut out = MeteredVec::with_capacity(meter, name.len().saturating_add(1))?;
    out.extend_from_slice(name)?;
    out.push(0)?;
    Ok(out)
}
fn open(parent: RawFd, name: &[u8], flags: i32, meter: &Arc<Meter>) -> Result<File, Error> {
    if flags & libc::O_DIRECTORY != 0 {
        meter.count_decode(DIRECTORY_WORK)?;
    }
    let name = nul(name, meter)?;
    // SAFETY: a NUL-terminated string lives through the syscall; the result
    // is either a negative errno or a newly owned descriptor.
    let fd = unsafe {
        libc::openat(
            parent,
            name.as_slice().as_ptr().cast(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            0o644 as libc::c_uint,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: successful openat transfers this single owned descriptor.
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn mkdir(parent: RawFd, name: &[u8], meter: &Arc<Meter>) -> Result<File, Error> {
    meter.count_decode(DIRECTORY_WORK)?;
    let terminated = nul(name, meter)?;
    // SAFETY: valid parent fd and live NUL-terminated name.
    let status = unsafe { libc::mkdirat(parent, terminated.as_slice().as_ptr().cast(), 0o755) };
    if status != 0 && io::Error::last_os_error().kind() != io::ErrorKind::AlreadyExists {
        return Err(io::Error::last_os_error().into());
    }
    open(parent, name, libc::O_RDONLY | libc::O_DIRECTORY, meter)
}
impl Files {
    pub fn new(root: &Path, meter: &Arc<Meter>) -> Result<Self, Error> {
        meter.count_decode((root.components().count() as u64).saturating_mul(DIRECTORY_WORK))?;
        std::fs::create_dir_all(root)?;
        let root = open(
            libc::AT_FDCWD,
            root.as_os_str().as_bytes(),
            libc::O_RDONLY | libc::O_DIRECTORY,
            meter,
        )?;
        Ok(Self {
            root,
            meter: meter.clone(),
        })
    }
    /// Open an MCP output root one component at a time. Startup paths must
    /// contain neither parent components nor symlinks, including ancestors.
    /// Relative paths attach to the held working directory; absolute paths
    /// attach to `/`. Keep the ordinary CLI constructor's path semantics.
    pub(crate) fn confined(root: &Path, meter: &Arc<Meter>) -> Result<Self, Error> {
        if root.as_os_str().as_bytes().contains(&0)
            || root.components().any(|part| part == Component::ParentDir)
        {
            return Err(Error::UnsafePath);
        }
        let mut held = open(
            libc::AT_FDCWD,
            if root.is_absolute() { b"/" } else { b"." },
            libc::O_RDONLY | libc::O_DIRECTORY,
            meter,
        )?;
        for part in root.components() {
            if let Component::Normal(name) = part {
                held = mkdir(held.as_raw_fd(), name.as_bytes(), meter)?;
            }
        }
        Ok(Self {
            root: held,
            meter: meter.clone(),
        })
    }
    /// Reach a request's subdirectory from the server's held root, then
    /// use the original writer with the request's own finite meter.
    pub(crate) fn subdirectory(&self, path: &[u8], meter: &Arc<Meter>) -> Result<Self, Error> {
        output_limit(path).map_err(Error::Field)?;
        let request = Self {
            root: clone_directory(&self.root, meter)?,
            meter: meter.clone(),
        };
        let root = request.parent(path)?;
        Ok(Self {
            root,
            meter: meter.clone(),
        })
    }
    fn parent(&self, path: &[u8]) -> Result<File, Error> {
        paths::validate(path)?;
        let mut directory = clone_directory(&self.root, &self.meter)?;
        for segment in path
            .split(|b| *b == b'/')
            .filter(|s| !s.is_empty() && *s != b".")
        {
            directory = mkdir(directory.as_raw_fd(), segment, &self.meter)?;
        }
        Ok(directory)
    }
    fn publish(&self, path: &[u8], bytes: &[u8]) -> Result<(), Error> {
        use std::io::Write;
        paths::validate(path)?;
        let slash = path.iter().rposition(|b| *b == b'/');
        let parent = self.parent(slash.and_then(|i| path.get(..i)).unwrap_or_default())?;
        let leaf = slash.and_then(|i| path.get(i + 1..)).unwrap_or(path);
        if leaf.is_empty() || leaf == b"." {
            return Err(Error::UnsafePath);
        }
        let mut temp = ChargedBytes::new(&self.meter);
        temp.write_args(format_args!(
            ".unityrs-extract-{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ))?;
        let temporary = nul(temp.as_bytes(), &self.meter)?;
        let leaf = nul(leaf, &self.meter)?;
        let mut file = open(
            parent.as_raw_fd(),
            temp.as_bytes(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            &self.meter,
        )?;
        let result = (|| {
            self.meter.count_decode(bytes.len() as u64)?;
            file.write_all(bytes)?;
            file.flush()?;
            // SAFETY: both names are live terminated strings; rename replaces
            // a leaf without following it, beneath the same held directory.
            let status = unsafe {
                libc::renameat(
                    parent.as_raw_fd(),
                    temporary.as_slice().as_ptr().cast(),
                    parent.as_raw_fd(),
                    leaf.as_slice().as_ptr().cast(),
                )
            };
            if status != 0 {
                return Err(io::Error::last_os_error().into());
            }
            Ok(())
        })();
        if result.is_err() {
            // SAFETY: the unique temporary leaf belongs to this call.
            unsafe {
                libc::unlinkat(parent.as_raw_fd(), temporary.as_slice().as_ptr().cast(), 0);
            }
        }
        result
    }
}
impl Sink for Files {
    fn directory(&mut self, relative: &[u8]) -> Result<(), Error> {
        self.parent(relative).map(|_| ())
    }
    fn file(&mut self, relative: &[u8], bytes: &[u8]) -> Result<(), Error> {
        self.publish(relative, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn planted_symlinks_and_parent_components_are_never_followed() {
        use std::os::unix::fs::symlink;
        let tmp = unityrs_testkit::scratch::Scratch::new("extract-planted");
        let root = tmp.join("out");
        let outside = tmp.join("outside");
        std::fs::create_dir(&outside).unwrap();
        let mut files = Files::new(&root, &Meter::unlimited()).unwrap();
        // Plant these after the output descriptor has been opened.
        symlink(&outside, root.join("planted")).unwrap();
        std::fs::create_dir(root.join("inside")).unwrap();
        for path in [
            b"planted/file".as_slice(),
            b"inside/../../outside/file",
            b"inside/../file",
        ] {
            assert!(files.file(path, b"payload").is_err(), "{path:?}");
            assert!(files.directory(path).is_err(), "{path:?}");
        }
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        assert!(!root.join("file").exists());
    }
}

#[cfg(test)]
mod work_tests {
    use super::*;
    #[test]
    fn directory_creation_and_opening_refuse_exhausted_work_before_syscalls() {
        let tmp = unityrs_testkit::scratch::Scratch::new("extract-directory-work");
        let files = Files::new(tmp.path(), &Meter::unlimited()).unwrap();
        for existing in [false, true] {
            let limits = unityrs_core::limits::Limits::default();
            let meter = crate::extract::limits::extraction(&limits);
            // Leave enough for all legacy name-buffer work, but no directory operations.
            meter.count_decode(limits.max_decode_work - 8).unwrap();
            if existing {
                std::fs::create_dir(tmp.join("a")).unwrap();
            }
            let result = if existing {
                open(
                    files.root.as_raw_fd(),
                    b"a",
                    libc::O_RDONLY | libc::O_DIRECTORY,
                    &meter,
                )
            } else {
                mkdir(files.root.as_raw_fd(), b"a", &meter)
            };
            assert!(matches!(result, Err(ref e) if e.resource_refusal()));
            assert!(!tmp.join("a/b").exists());
            if !existing {
                assert!(!tmp.join("a").exists());
            }
        }
    }
}
