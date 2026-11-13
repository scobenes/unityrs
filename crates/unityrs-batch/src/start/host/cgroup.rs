//! Streaming hierarchy discovery, longest visible mount root and ancestor limits.
use super::{Class, Error, Host, invalid, lines, number};
use std::{
    io,
    path::{Path, PathBuf},
};

struct Hierarchy {
    member: String,
    mount: PathBuf,
    root_len: usize,
    v1: bool,
}
fn copy(text: &str) -> io::Result<String> {
    let mut out = String::new();
    out.try_reserve_exact(text.len())
        .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
    out.push_str(text);
    Ok(out)
}
fn hierarchy(host: &dyn Host, controller: &str) -> Result<Option<Hierarchy>, Error> {
    let (mut v1, mut v2) = (None, None);
    lines(
        host,
        "/proc/self/cgroup",
        Class::ProcfsUnreadable,
        &mut |s| {
            let mut parts = s.splitn(3, ':');
            let id = parts.next().ok_or_else(invalid)?;
            let controllers = parts.next().ok_or_else(invalid)?;
            let member = parts.next().ok_or_else(invalid)?;
            if !valid_path(member) {
                return Err(invalid());
            }
            if controllers.split(',').any(|c| c == controller) {
                v1 = Some(copy(member)?);
            } else if id == "0" && controllers.is_empty() {
                v2 = Some(copy(member)?);
            }
            Ok(())
        },
    )?;
    let v1_present = v1.is_some();
    let Some(member) = v1.or(v2) else {
        return Ok(None);
    };
    let mut result = Hierarchy {
        member,
        mount: PathBuf::new(),
        root_len: 0,
        v1: v1_present,
    };
    lines(
        host,
        "/proc/self/mountinfo",
        Class::ProcfsUnreadable,
        &mut |s| {
            let (before, after) = s.split_once(" - ").ok_or_else(invalid)?;
            let mut post = after.split_whitespace();
            let kind = post.next().ok_or_else(invalid)?;
            let _source = post.next().ok_or_else(invalid)?;
            let options = post.next().ok_or_else(invalid)?;
            if (result.v1 && (kind != "cgroup" || !options.split(',').any(|c| c == controller)))
                || (!result.v1 && kind != "cgroup2")
            {
                return Ok(());
            }
            let mut pre = before.split_whitespace();
            let root = decode(pre.nth(3).ok_or_else(invalid)?)?;
            let mount = decode(pre.next().ok_or_else(invalid)?)?;
            if Path::new(&result.member).starts_with(&root) && root.len() > result.root_len {
                result.root_len = root.len();
                result.mount = PathBuf::from(mount);
            }
            Ok(())
        },
    )?;
    if result.root_len == 0 {
        return Ok(None);
    }
    Ok(Some(result))
}
fn valid_path(path: &str) -> bool {
    path.starts_with('/') && !path.contains('\0') && path.split('/').all(|p| p != ".." && p != ".")
}
fn decode(raw: &str) -> io::Result<String> {
    let mut out = Vec::new();
    out.try_reserve_exact(raw.len())
        .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
    let mut bytes = raw.bytes();
    while let Some(b) = bytes.next() {
        if b == b'\\' {
            let mut value = 0_u8;
            for _ in 0..3 {
                let digit = bytes.next().ok_or_else(invalid)?;
                if !(b'0'..=b'7').contains(&digit) {
                    return Err(invalid());
                }
                value = value
                    .checked_mul(8)
                    .and_then(|v| v.checked_add(digit - b'0'))
                    .ok_or_else(invalid)?;
            }
            out.push(value);
        } else {
            out.push(b);
        }
    }
    let text = String::from_utf8(out).map_err(|_| invalid())?;
    if !valid_path(&text) {
        return Err(invalid());
    }
    Ok(text)
}
fn path(h: &Hierarchy) -> Result<PathBuf, Error> {
    let tail = h
        .member
        .get(h.root_len..)
        .ok_or_else(|| Error::new(Class::CgroupUnreadable, "/proc/self/cgroup", None, None))?
        .trim_start_matches('/');
    // A mount rooted at / must retain the first byte of a nonempty tail.
    let tail = if h.root_len == 1 {
        h.member.trim_start_matches('/')
    } else {
        tail
    };
    crate::spool::path::join(&h.mount, Path::new(tail))
        .map_err(|_| Error::new(Class::HostMemory, "", None, None))
}
fn field_path(dir: &Path, field: &str) -> Result<PathBuf, Error> {
    crate::spool::path::join(dir, Path::new(field))
        .map_err(|_| Error::new(Class::HostMemory, "", None, None))
}
fn text(path: &Path) -> Result<&str, Error> {
    path.to_str()
        .ok_or_else(|| Error::new(Class::CgroupUnreadable, "", None, None))
}
fn optional(host: &dyn Host, dir: &Path, name: &str) -> Result<Option<u64>, Error> {
    let path = field_path(dir, name)?;
    let path = text(&path)?;
    let mut found = None;
    match host.lines(path, &mut |s| {
        if found.is_some() {
            return Err(invalid());
        }
        found = Some(if s.trim() == "max" {
            None
        } else {
            Some(s.trim().parse().map_err(|_| invalid())?)
        });
        Ok(())
    }) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(Error::new(
                if e.kind() == io::ErrorKind::OutOfMemory {
                    Class::HostMemory
                } else {
                    Class::CgroupUnreadable
                },
                path,
                None,
                None,
            ));
        }
        Ok(()) => {}
    }
    found.ok_or_else(|| Error::new(Class::CgroupUnreadable, path, None, None))
}
pub(super) fn memory(host: &dyn Host) -> Result<Option<u64>, Error> {
    let Some(h) = hierarchy(host, "memory")? else {
        return Ok(None);
    };
    let mut dir = path(&h)?;
    if h.v1 {
        let file = field_path(&dir, "memory.stat")?;
        let file = text(&file)?;
        let mut limit = None;
        match host.lines(file, &mut |s| {
            if let Some(value) = s.strip_prefix("hierarchical_memory_limit ") {
                limit = Some(value.trim().parse().map_err(|_| invalid())?);
            }
            Ok(())
        }) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Error::new(Class::CgroupUnreadable, file, None, None)),
            Ok(()) => {}
        }
        return limit
            .map(Some)
            .ok_or_else(|| Error::new(Class::CgroupUnreadable, file, None, None));
    }
    let mut smallest: Option<u64> = None;
    loop {
        if let Some(n) = optional(host, &dir, "memory.max")? {
            smallest = Some(smallest.map_or(n, |old| old.min(n)));
        }
        if dir == h.mount || !dir.pop() {
            break;
        }
    }
    Ok(smallest)
}
pub(super) fn pids(host: &dyn Host, added: u64) -> Result<(), Error> {
    let Some(h) = hierarchy(host, "pids")? else {
        return Ok(());
    };
    let mut dir = path(&h)?;
    loop {
        if let Some(max) = optional(host, &dir, "pids.max")? {
            let file = field_path(&dir, "pids.current")?;
            let current = number(host, text(&file)?, Class::CgroupUnreadable)?;
            let room = max.saturating_sub(current);
            if room < added {
                // Public detail uses the hierarchy-relative cgroup path.
                let suffix = dir
                    .strip_prefix(&h.mount)
                    .map_err(|_| Error::new(Class::CgroupUnreadable, "", None, None))?;
                let relative = if h.root_len == 1 {
                    Path::new("/")
                } else {
                    Path::new(
                        h.member
                            .get(..h.root_len)
                            .ok_or_else(|| Error::new(Class::CgroupUnreadable, "", None, None))?,
                    )
                };
                let relative = crate::spool::path::join(relative, suffix)
                    .map_err(|_| Error::new(Class::HostMemory, "", None, None))?;
                return Err(Error::new(
                    Class::RlimitTooLow,
                    text(&relative)?,
                    Some(added),
                    Some(room),
                ));
            }
        }
        if dir == h.mount || !dir.pop() {
            break;
        }
    }
    Ok(())
}
