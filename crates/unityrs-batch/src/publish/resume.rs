//! Whole-file MD5 under the same serial admission as the following transfer.
use super::{Context, Point, sink_error};
use crate::{
    phase::{Error, check},
    sink::{self, path::Name},
    winners::Row,
};
use md5::{Digest, Md5};
use std::{fs::File, io, os::unix::fs::MetadataExt};

pub(super) fn present(cx: &Context<'_>, row: Row<'_>, bytes: &mut [u8]) -> Result<bool, Error> {
    cx.at(Point::Resume)?;
    let root = cx.directory.root();
    let parent = match root.parent(row.key, false, sink::Context::Output) {
        Ok(p) => p,
        Err(e) if e.errno == Some(libc::ENOENT) || e.class == sink::Class::PublishFailed => {
            return Ok(false);
        }
        Err(e) => return Err(sink_error(e)),
    };
    let name =
        Name::new(row.key.rsplit('/').next().ok_or_else(Error::internal)?).map_err(sink_error)?;
    // macOS cannot pin with O_PATH: compare the no-follow check with fstat
    // after a nonblocking open. Linux validates an O_PATH descriptor instead.
    #[cfg(not(target_os = "linux"))]
    let checked = match root.fs().entry(&parent.file, name.cstr()) {
        Ok(entry) if entry.regular && entry.size == row.size && root.matches_mount(entry.mount) => {
            entry
        }
        Ok(_) => return Ok(false),
        Err(e) if miss(&e) => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let pinned = match root.child(
        &parent.file,
        name.cstr(),
        crate::sys::resume_flags(),
        sink::Context::Output,
    ) {
        Ok(file) => file,
        Err(e) if e.errno.is_some_and(miss_errno) => return Ok(false),
        Err(e) => return Err(sink_error(e)),
    };
    let meta = pinned.metadata()?;
    if !meta.is_file() || meta.len() != row.size || !root.matches_mount(root.fs().mount(&pinned)?) {
        return Ok(false);
    }
    #[cfg(not(target_os = "linux"))]
    if checked.identity != (meta.dev(), meta.ino()) {
        return Ok(false);
    }
    let file: File = match root.fs().reopen(&pinned) {
        Ok(file) => file,
        // A missing/inaccessible procfs is a miss too. Never fall back to a
        // Linux pathname open that could invoke a substituted device handler.
        Err(e) if miss(&e) => return Ok(false),
        Err(e) => return Err(e.into()),
    };
    let opened = file.metadata()?;
    if !opened.is_file()
        || opened.len() != row.size
        || (opened.dev(), opened.ino()) != (meta.dev(), meta.ino())
        || !root.matches_mount(root.fs().mount(&file)?)
    {
        return Ok(false);
    }
    let mut md5 = Md5::new();
    let mut size = 0_u64;
    loop {
        cx.at(Point::Buffer)?;
        let n = crate::sys::retry_interrupted(|| root.fs().read(&file, bytes))?;
        if n == 0 {
            break;
        }
        let bytes = bytes.get(..n).ok_or_else(Error::internal)?;
        size = size
            .checked_add(u64::try_from(n).map_err(|_| Error::internal())?)
            .ok_or_else(Error::internal)?;
        if size > row.size {
            return Ok(false);
        }
        md5.update(bytes);
    }
    check(cx.stopped)?;
    let md5: [u8; 16] = md5.finalize().into();
    if size != row.size || md5 != row.md5 {
        return Ok(false);
    }
    // A pinned inode can outlive its directory entry. Do not call a replaced
    // pathname present just because the old inode still has the expected hash.
    match root.fs().entry(&parent.file, name.cstr()) {
        Ok(entry) => Ok(entry.regular
            && entry.size == row.size
            && entry.identity == (meta.dev(), meta.ino())
            && root.matches_mount(entry.mount)),
        Err(e) if miss(&e) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

fn miss(error: &io::Error) -> bool {
    error.raw_os_error().is_some_and(miss_errno)
}
fn miss_errno(errno: i32) -> bool {
    matches!(
        errno,
        libc::ENOENT
            | libc::ELOOP
            | libc::EACCES
            | libc::EPERM
            | libc::EXDEV
            | libc::ENXIO
            | libc::EOPNOTSUPP
    )
}
