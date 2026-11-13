//! Decoded leaf facts, resolver probes and descriptor-anchored visits.
use super::{Cursor, cvt, directory_flags, rooted_open};
use std::{fs::File, io, os::fd::AsRawFd};

/// No-follow leaf facts, obtained before opening a resume destination.
#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub regular: bool,
    pub size: u64,
    pub mount: Option<u64>,
    pub identity: (u64, u64),
}
impl Entry {
    #[cfg(target_os = "linux")]
    pub(super) fn from_linux(stat: libc::statx) -> io::Result<Self> {
        if stat.stx_mask & (libc::STATX_TYPE | libc::STATX_SIZE | libc::STATX_INO)
            != libc::STATX_TYPE | libc::STATX_SIZE | libc::STATX_INO
        {
            return Err(io::Error::from(io::ErrorKind::Unsupported));
        }
        Ok(Self {
            identity: (
                libc::makedev(stat.stx_dev_major, stat.stx_dev_minor),
                stat.stx_ino,
            ),
            regular: u32::from(stat.stx_mode) & libc::S_IFMT == libc::S_IFREG,
            size: stat.stx_size,
            mount: (stat.stx_mask & libc::STATX_MNT_ID != 0).then_some(stat.stx_mnt_id),
        })
    }
    #[cfg(not(target_os = "linux"))]
    pub(super) fn from_unix(stat: libc::stat) -> io::Result<Self> {
        Ok(Self {
            identity: (u64::from(stat.st_dev.cast_unsigned()), stat.st_ino),
            regular: stat.st_mode & libc::S_IFMT == libc::S_IFREG,
            size: u64::try_from(stat.st_size)
                .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?,
            mount: Some(
                u64::try_from(stat.st_dev)
                    .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?,
            ),
        })
    }
}
/// Visit entries using an independent directory description (not dup's shared
/// directory offset). The callback consumes each name before the next read.
pub fn visit_directory(
    root: &File,
    visit: &mut dyn FnMut(&std::ffi::CStr) -> io::Result<()>,
) -> io::Result<()> {
    let file = rooted_open(root, c".", directory_flags(), false)?;
    let cursor = Cursor::new(file, None)?;
    while let Some(name) = cursor.next_name()? {
        if name != c"." && name != c".." {
            visit(name)?;
        }
    }
    Ok(())
}

pub fn beneath_probe(root: &File) -> io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        rooted_open(root, c".", libc::O_RDONLY | libc::O_DIRECTORY, true).map(drop)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = root;
        Err(io::Error::from_raw_os_error(libc::ENOSYS))
    }
}

#[must_use]
pub fn traversal_flags() -> i32 {
    #[cfg(target_os = "linux")]
    {
        libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW
    }
    #[cfg(not(target_os = "linux"))]
    {
        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK
    }
}

pub fn rooted_entry(parent: &File, name: &std::ffi::CStr) -> io::Result<Entry> {
    #[cfg(target_os = "linux")]
    {
        let mut stat = std::mem::MaybeUninit::<libc::statx>::uninit();
        // SAFETY: live parent, terminated name, writable complete statx;
        // AT_SYMLINK_NOFOLLOW inspects the leaf without opening or following it.
        cvt(unsafe {
            libc::statx(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
                libc::STATX_TYPE | libc::STATX_SIZE | libc::STATX_INO | libc::STATX_MNT_ID,
                stat.as_mut_ptr(),
            )
        })?;
        // SAFETY: successful statx initialized the output structure.
        let stat = unsafe { stat.assume_init() };
        Entry::from_linux(stat)
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Test adapters on other Unix hosts use device IDs; production startup
        // still requires Linux and its mount-ID checks.
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: live parent, terminated name and complete writable stat.
        cvt(unsafe {
            libc::fstatat(
                parent.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        })?;
        // SAFETY: successful fstatat initialized the output structure.
        let stat = unsafe { stat.assume_init() };
        Entry::from_unix(stat)
    }
}

/// Pin a leaf without invoking a device's open handler on Linux.
#[must_use]
pub fn resume_flags() -> i32 {
    #[cfg(target_os = "linux")]
    {
        libc::O_PATH | libc::O_NOFOLLOW
    }
    #[cfg(not(target_os = "linux"))]
    {
        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_NOCTTY
    }
}
/// Reopen the already validated regular inode, never its mutable pathname.
pub fn reopen_pinned(file: &File) -> io::Result<File> {
    #[cfg(target_os = "linux")]
    {
        use std::{io::Write, os::fd::FromRawFd};
        let mut path = [0; 64];
        write!(&mut path[..], "/proc/self/fd/{}", file.as_raw_fd())?;
        // SAFETY: stack path is NUL-terminated and lives across open. Following
        // this proc magic link is intentional: file owns the pinned descriptor.
        let fd = unsafe {
            libc::open(
                path.as_ptr().cast(),
                libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK | libc::O_NOCTTY,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful open transferred exactly one new descriptor.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    #[cfg(not(target_os = "linux"))]
    {
        file.try_clone()
    }
}
