//! Complete-engine local host checks (`batch.md` 2.7). These functions do not
//! start threads or inspect the spool. B9 owns their final startup ordering.
mod cgroup;
use crate::spec::model::Spec;
use std::{
    io::{self, Read},
    path::Path,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    AddressSpace,
    Data,
    OpenFiles,
    FileSize,
    Processes,
}
impl Resource {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::AddressSpace => "RLIMIT_AS",
            Self::Data => "RLIMIT_DATA",
            Self::OpenFiles => "RLIMIT_NOFILE",
            Self::FileSize => "RLIMIT_FSIZE",
            Self::Processes => "RLIMIT_NPROC",
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Limit {
    pub soft: Option<u64>,
    pub hard: Option<u64>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    RlimitTooLow,
    OvercommitStrict,
    MapCountTooLow,
    MemoryTooSmall,
    ProcfsUnreadable,
    CgroupUnreadable,
    HostMemory,
    Arithmetic,
    StartupFailed,
}
/// Numeric details are kept typed; no host content enters public diagnostics.
/// `path` is used only by the specified procfs/cgroup detail forms.
#[derive(Debug)]
pub struct Error {
    pub class: Class,
    pub subject: String,
    pub needed: Option<u64>,
    pub found: Option<u64>,
    pub errno: Option<i32>,
}
impl Error {
    fn new(class: Class, subject: &str, needed: Option<u64>, found: Option<u64>) -> Self {
        let mut text = String::new();
        if text.try_reserve_exact(subject.len()).is_err() {
            return Self {
                class: Class::HostMemory,
                subject: text,
                needed: None,
                found: None,
                errno: None,
            };
        }
        text.push_str(subject);
        Self {
            class,
            subject: text,
            needed,
            found,
            errno: None,
        }
    }
}
/// All host data is injectable; no environment-based test hooks are added.
pub trait Host {
    fn lines(&self, path: &str, visit: &mut dyn FnMut(&str) -> io::Result<()>) -> io::Result<()>;
    fn limit(&self, resource: Resource) -> io::Result<Limit>;
    fn raise(&self, resource: Resource, limit: Limit) -> io::Result<()>;
    fn privileged(&self) -> io::Result<bool>;
}
pub struct System;
impl Host for System {
    fn lines(&self, path: &str, visit: &mut dyn FnMut(&str) -> io::Result<()>) -> io::Result<()> {
        // Fixed 16 KiB line storage and 1 KiB read storage are C_base scratch;
        // mountinfo/cgroup inventories are streamed, never retained.
        let mut line = Vec::new();
        line.try_reserve_exact(16 * 1024)
            .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
        let mut file = crate::sys::spool_file(Path::new(path), false)?;
        let mut block = [0; 1024];
        loop {
            let n = crate::sys::retry_interrupted(|| file.read(&mut block))?;
            if n == 0 {
                break;
            }
            for b in block
                .get(..n)
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?
            {
                if *b == b'\n' {
                    visit(
                        std::str::from_utf8(&line)
                            .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?,
                    )?;
                    line.clear();
                } else {
                    if line.len() == 16 * 1024 {
                        return Err(io::Error::from(io::ErrorKind::InvalidData));
                    }
                    line.push(*b);
                }
            }
        }
        if !line.is_empty() {
            visit(
                std::str::from_utf8(&line)
                    .map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?,
            )?;
        }
        Ok(())
    }
    fn limit(&self, resource: Resource) -> io::Result<Limit> {
        crate::sys::resource_limit(resource, None)
    }
    fn raise(&self, resource: Resource, limit: Limit) -> io::Result<()> {
        crate::sys::resource_limit(resource, Some(limit)).map(drop)
    }
    fn privileged(&self) -> io::Result<bool> {
        crate::sys::privileged()
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Requirements {
    pub bound: u64,
    pub address: u64,
    pub maps: u64,
    pub files: u64,
    pub threads: u64,
}
impl Requirements {
    pub fn new(spec: &Spec, bound: u64) -> Result<Self, Error> {
        let dc = u64::from(spec.download_concurrency);
        let t = u64::from(spec.threads);
        let address = bound
            .checked_add(bound.div_ceil(2))
            .and_then(|n| n.checked_add((t + 2) * 64 * 1024 * 1024))
            .and_then(|n| n.checked_add((dc + 1) * 3 * 1024 * 1024))
            .ok_or_else(|| Error::new(Class::Arithmetic, "memory_bound", None, None))?;
        Ok(Self {
            bound,
            address,
            maps: spec.max_inflight_uncompressed_bytes / (128 * 1024) + 8_192,
            files: 4 * t + 16 * dc + 72,
            threads: t + 7 + 2 * dc,
        })
    }
}
/// Check in the specified order, including complete-engine threads and network
/// allowances even though the current executable subset is local-only.
pub fn check(host: &dyn Host, needs: Requirements) -> Result<(), Error> {
    for resource in [Resource::AddressSpace, Resource::Data] {
        let limit = get_limit(host, resource)?;
        floor(resource.name(), needs.address, limit.soft)?;
    }
    if number(
        host,
        "/proc/sys/vm/overcommit_memory",
        Class::ProcfsUnreadable,
    )? == 2
    {
        return Err(Error::new(Class::OvercommitStrict, "", None, None));
    }
    let maps = number(host, "/proc/sys/vm/max_map_count", Class::ProcfsUnreadable)?;
    if maps < needs.maps {
        return Err(Error::new(Class::MapCountTooLow, "", None, None));
    }
    let mut total = None;
    lines(host, "/proc/meminfo", Class::ProcfsUnreadable, &mut |s| {
        if let Some(s) = s.strip_prefix("MemTotal:") {
            let mut fields = s.split_whitespace();
            let value: u64 = fields
                .next()
                .ok_or_else(invalid)?
                .parse()
                .map_err(|_| invalid())?;
            if fields.next() != Some("kB") || fields.next().is_some() {
                return Err(invalid());
            }
            total = Some(value.checked_mul(1024).ok_or_else(invalid)?);
        }
        Ok(())
    })?;
    let total =
        total.ok_or_else(|| Error::new(Class::ProcfsUnreadable, "/proc/meminfo", None, None))?;
    // Memory and PID hierarchies are inspected separately to preserve check order.
    let memory = cgroup::memory(host)?;
    let (label, found) = if memory.is_some_and(|m| m < total) {
        ("cgroup", memory.unwrap_or(total))
    } else {
        ("MemTotal", total)
    };
    if found < needs.bound {
        return Err(Error::new(
            Class::MemoryTooSmall,
            label,
            Some(needs.bound),
            Some(found),
        ));
    }
    for resource in [Resource::OpenFiles, Resource::FileSize] {
        let limit = get_limit(host, resource)?;
        match resource {
            Resource::OpenFiles => floor(resource.name(), needs.files, limit.hard)?,
            Resource::FileSize if limit.hard.is_some() => {
                return Err(Error::new(
                    Class::RlimitTooLow,
                    resource.name(),
                    None,
                    limit.hard,
                ));
            }
            _ => {}
        }
        host.raise(resource, limit)
            .map_err(|e| call_error("setrlimit", &e))?;
    }
    let mut initial = false;
    lines(
        host,
        "/proc/self/uid_map",
        Class::ProcfsUnreadable,
        &mut |s| {
            let mut words = s.split_whitespace();
            initial = words.next() == Some("0")
                && words.next() == Some("0")
                && words.next() == Some("4294967295")
                && words.next().is_none();
            Ok(())
        },
    )?;
    let privileged = host
        .privileged()
        .map_err(|_| Error::new(Class::ProcfsUnreadable, "capget", None, None))?;
    let limit = get_limit(host, Resource::Processes)?;
    if !(initial && privileged) {
        floor(Resource::Processes.name(), needs.threads, limit.soft)?;
    }
    cgroup::pids(host, needs.threads.saturating_sub(1))
}
fn get_limit(host: &dyn Host, r: Resource) -> Result<Limit, Error> {
    host.limit(r).map_err(|e| call_error("getrlimit", &e))
}
fn floor(name: &str, needed: u64, found: Option<u64>) -> Result<(), Error> {
    if found.is_some_and(|n| n < needed) {
        return Err(Error::new(Class::RlimitTooLow, name, Some(needed), found));
    }
    Ok(())
}
fn invalid() -> io::Error {
    io::Error::from(io::ErrorKind::InvalidData)
}
fn lines(
    host: &dyn Host,
    path: &str,
    class: Class,
    visit: &mut dyn FnMut(&str) -> io::Result<()>,
) -> Result<(), Error> {
    host.lines(path, visit).map_err(|e| {
        Error::new(
            if e.kind() == io::ErrorKind::OutOfMemory {
                Class::HostMemory
            } else {
                class
            },
            path,
            None,
            None,
        )
    })
}
fn number(host: &dyn Host, path: &str, class: Class) -> Result<u64, Error> {
    let mut value = None;
    lines(host, path, class, &mut |s| {
        if value.is_some() {
            return Err(invalid());
        }
        value = Some(s.trim().parse().map_err(|_| invalid())?);
        Ok(())
    })?;
    value.ok_or_else(|| Error::new(class, path, None, None))
}

fn call_error(call: &str, e: &io::Error) -> Error {
    let mut error = Error::new(Class::StartupFailed, call, None, None);
    error.errno = e.raw_os_error();
    error
}
