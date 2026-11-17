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
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_export::extractor::{Error, Sink, paths};
use unityrs_mesh::obj::{ChargedBytes, MeteredWrite};
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
    /// Reach a request's subdirectory from the server's held root, then
    /// use the original writer with the request's own finite meter.
    pub(crate) fn subdirectory(&self, path: &[u8], meter: &Arc<Meter>) -> Result<Self, Error> {
        let request = Self {
            root: self.root.try_clone()?,
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
        let mut directory = self.root.try_clone()?;
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
