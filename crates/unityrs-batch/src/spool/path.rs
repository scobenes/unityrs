//! Transient generated paths, reserved fallibly before any push.
use super::Error;
use std::path::{Path, PathBuf};

pub(crate) fn join(root: &Path, name: &Path) -> Result<PathBuf, Error> {
    let capacity = root
        .as_os_str()
        .len()
        .checked_add(name.as_os_str().len())
        .and_then(|n| n.checked_add(1))
        .ok_or(Error::Internal)?;
    let mut path = PathBuf::new();
    path.try_reserve_exact(capacity)
        .map_err(|_| Error::HostMemory)?;
    path.push(root);
    path.push(name);
    Ok(path)
}
pub(crate) fn indexed(root: &Path, prefix: &str, mut id: u64) -> Result<PathBuf, Error> {
    let capacity = root
        .as_os_str()
        .len()
        .checked_add(prefix.len())
        .and_then(|n| n.checked_add(21))
        .ok_or(Error::Internal)?;
    let mut path = PathBuf::new();
    path.try_reserve_exact(capacity)
        .map_err(|_| Error::HostMemory)?;
    let mut digits = [b'0'; 20];
    let mut start = digits.len();
    loop {
        start -= 1;
        *digits.get_mut(start).ok_or(Error::Internal)? =
            b'0' + u8::try_from(id % 10).map_err(|_| Error::Internal)?;
        id /= 10;
        if id == 0 {
            break;
        }
    }
    let suffix = std::str::from_utf8(digits.get(start..).ok_or(Error::Internal)?)
        .map_err(|_| Error::Internal)?;
    path.push(root);
    // Push the prefix as a component, then append the decimal suffix to it.
    // The complete capacity was reserved before either operation.
    path.push(prefix);
    if prefix.is_empty() {
        path.push(suffix);
    } else {
        path.as_mut_os_string().push(suffix);
    }
    Ok(path)
}
