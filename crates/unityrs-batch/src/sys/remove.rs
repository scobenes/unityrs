use super::{Cursor, REMOVE_DEPTH, directory_at, terminated_path, unlink_at};
/// Cooperative deletion anchored to the spool's held root. Every relative
/// component is opened with `openat`; even the top removal uses its parent fd.
pub fn remove_tree(
    root: &std::fs::File,
    relative: &std::path::Path,
    stopped: &dyn Fn() -> bool,
) -> Result<(), crate::spool::Error> {
    use crate::spool::Error;
    use std::{os::fd::AsRawFd, path::Component};
    if stopped() {
        return Err(Error::Stopped);
    }
    let mut components = relative.components().peekable();
    let mut parent = root.try_clone()?;
    let top = loop {
        let Some(Component::Normal(name)) = components.next() else {
            return Err(Error::Internal);
        };
        let name = terminated_path(std::path::Path::new(name))?;
        if components.peek().is_none() {
            break name;
        }
        parent = directory_at(parent.as_raw_fd(), &name)?;
    };
    let file = directory_at(parent.as_raw_fd(), &top)?;
    let mut stack = Vec::new();
    stack
        .try_reserve_exact(REMOVE_DEPTH)
        .map_err(|_| Error::HostMemory)?;
    stack.push(Cursor::new(file, None)?);
    let mut entries = 0_u64;
    while let Some(cursor) = stack.last() {
        let Some(name) = cursor.next_name()? else {
            let name = stack.pop().ok_or(Error::Internal)?.name.take();
            if let Some(ancestor) = stack.last() {
                unlink_at(
                    ancestor.fd(),
                    &name.ok_or(Error::Internal)?,
                    libc::AT_REMOVEDIR,
                )?;
            }
            continue;
        };
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        if entries.is_multiple_of(1_000) && stopped() {
            return Err(Error::Stopped);
        }
        entries += 1;
        // Fallible copy, including the trailing NUL, before advancing DIR.
        let mut owned = Vec::new();
        owned
            .try_reserve_exact(name.to_bytes_with_nul().len())
            .map_err(|_| Error::HostMemory)?;
        owned.extend_from_slice(name.to_bytes_with_nul());
        match directory_at(cursor.fd(), &owned) {
            Ok(file) => {
                if stack.len() == REMOVE_DEPTH {
                    return Err(Error::Internal);
                }
                stack.push(Cursor::new(file, Some(owned))?);
            }
            Err(error) => {
                if !matches!(error.raw_os_error(), Some(libc::ENOTDIR | libc::ELOOP)) {
                    return Err(error.into());
                }
                unlink_at(cursor.fd(), &owned, 0)?;
            }
        }
    }
    if stopped() {
        return Err(Error::Stopped);
    }
    unlink_at(parent.as_raw_fd(), &top, libc::AT_REMOVEDIR)?;
    Ok(())
}
