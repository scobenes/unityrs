use crate::{job::RunFailure, reason::Code, sink, start};
use std::fmt::Write;
#[derive(Debug)]
pub struct Error {
    pub code: Code,
    pub reason: &'static str,
    pub detail: Option<String>,
    pub status: Option<&'static str>,
}
impl Error {
    pub(super) fn usage() -> Self {
        Self::plain(Code::BadSpec, "bad_spec", Some("usage"))
    }
    pub(super) fn spawn() -> Self {
        Self::plain(Code::CouldNotStart, "thread_spawn_failed", None)
    }
    pub(super) fn internal() -> Self {
        Self::plain(Code::Internal, "internal", None)
    }
    pub(super) fn host_io() -> Self {
        Self::plain(Code::Host, "host_io", None)
    }
    pub(super) fn memory() -> Self {
        Self::plain(Code::Host, "host_memory", None)
    }
    /// A stop more severe than a preflight end's code replaces it (9.1).
    pub(super) fn failure_stop(stop: crate::lifetime::Stop) -> Self {
        Self::plain(
            stop.reason.code(),
            stop.reason.name(),
            stop.detail.map(crate::lifetime::SpoolDetail::name),
        )
    }
    fn plain(code: Code, reason: &'static str, detail: Option<&str>) -> Self {
        let detail = detail
            .map(|s| {
                let mut text = String::new();
                text.try_reserve_exact(s.len()).map_err(|_| ())?;
                text.push_str(s);
                Ok::<_, ()>(text)
            })
            .transpose();
        match detail {
            Ok(detail) => Self {
                code,
                reason,
                detail,
                status: None,
            },
            Err(()) => Self {
                code: Code::Host,
                reason: "host_memory",
                detail: None,
                status: None,
            },
        }
    }
    pub(super) fn failure(failure: RunFailure) -> Self {
        Self::plain(
            failure.code(),
            failure.name(),
            failure.detail.map(crate::attempt::DetailCode::name),
        )
    }
    /// A preflight reason (exit 69) and its detail.
    pub(super) fn preflight(reason: crate::reason::Preflight, detail: &str) -> Self {
        Self::plain(Code::CouldNotStart, reason.name(), Some(detail))
    }
}
/// A preflight scratch refusal, or a directory sink's `running` write:
/// any error of that write is 74, with no head (`BATCH_SPEC.md` 7.4).
pub(super) fn running(e: crate::phase::Error) -> Error {
    match e {
        crate::phase::Error::Run(f) => Error::failure(f),
        crate::phase::Error::Stopped => Error::host_io(),
    }
}
impl From<crate::spool::Error> for Error {
    fn from(e: crate::spool::Error) -> Self {
        Self::failure(e.into())
    }
}
pub(super) fn sink(e: sink::Error) -> Error {
    let reason = match e.class {
        sink::Class::SinkRootUnusable => "sink_root_unusable",
        sink::Class::KernelTooOld => "kernel_too_old",
        sink::Class::MountCheckUnavailable => "mount_check_unavailable",
        sink::Class::MountCrossing => "mount_crossing",
        sink::Class::OutputLocked => "output_locked",
        sink::Class::ManifestUnreadable => "manifest_unreadable",
        sink::Class::Superseded => "superseded",
        sink::Class::HostMemory => "host_memory",
        sink::Class::HostIo => "host_io",
        _ => "internal",
    };
    if e.class == sink::Class::ManifestUnreadable {
        return Error::plain(e.code(), reason, Some(e.call));
    }
    if matches!(e.class, sink::Class::SinkRootUnusable | sink::Class::HostIo) {
        return call(e.code(), reason, e.call, e.errno);
    }
    Error::plain(e.code(), reason, None)
}
pub(super) fn spool(e: &start::spool::Error) -> Error {
    use start::spool::Class;
    let (code, reason) = match e.class {
        Class::NotDedicated => (Code::CouldNotStart, "spool_not_dedicated"),
        Class::InMemory => (Code::CouldNotStart, "spool_in_memory"),
        Class::OverlapsSink => (Code::CouldNotStart, "spool_overlaps_sink"),
        Class::Locked => (Code::CouldNotStart, "spool_locked"),
        Class::NoSpace => (Code::CouldNotStart, "spool_no_space"),
        Class::Unusable => (Code::CouldNotStart, "spool_unusable"),
        Class::SinkRootUnusable => (Code::CouldNotStart, "sink_root_unusable"),
        Class::HostIo => (Code::Host, "host_io"),
        Class::HostMemory => (Code::Host, "host_memory"),
        // The drive hands a stopped startup over instead (7.4's first row).
        Class::Stopped => (Code::Internal, "internal"),
    };
    if matches!(e.class, Class::Unusable | Class::SinkRootUnusable) {
        call(code, reason, e.call, e.errno)
    } else {
        Error::plain(code, reason, None)
    }
}
pub(super) fn host(e: &start::host::Error) -> Error {
    use start::host::Class;
    let (code, reason) = match e.class {
        Class::RlimitTooLow => (Code::CouldNotStart, "rlimit_too_low"),
        Class::OvercommitStrict => (Code::CouldNotStart, "overcommit_strict"),
        Class::MapCountTooLow => (Code::CouldNotStart, "map_count_too_low"),
        Class::MemoryTooSmall => (Code::CouldNotStart, "memory_too_small"),
        Class::ProcfsUnreadable => (Code::CouldNotStart, "procfs_unreadable"),
        Class::CgroupUnreadable => (Code::CouldNotStart, "cgroup_unreadable"),
        Class::StartupFailed => (Code::CouldNotStart, "startup_failed"),
        Class::HostMemory => (Code::Host, "host_memory"),
        Class::Arithmetic => (Code::Internal, "internal"),
    };
    if matches!(e.class, Class::RlimitTooLow | Class::MemoryTooSmall) {
        let (name, suffix) = if e.class == Class::RlimitTooLow && e.subject.starts_with('/') {
            ("pids.max", Some(e.subject.as_str()))
        } else {
            (e.subject.as_str(), None)
        };
        let mut detail = String::new();
        if detail
            .try_reserve_exact(name.len() + suffix.map_or(0, str::len) + 64)
            .is_err()
        {
            return Error::plain(Code::Host, "host_memory", None);
        }
        let _ = write!(detail, "{name} ");
        if let Some(needed) = e.needed {
            let _ = write!(detail, "{needed}");
        } else {
            detail.push_str("infinity");
        }
        detail.push(' ');
        if let Some(found) = e.found {
            let _ = write!(detail, "{found}");
        } else {
            detail.push_str("infinity");
        }
        if let Some(suffix) = suffix {
            detail.push(' ');
            detail.push_str(suffix);
        }
        return Error {
            code,
            reason,
            detail: Some(detail),
            status: None,
        };
    }
    if e.class == Class::StartupFailed {
        return call(code, reason, &e.subject, e.errno);
    }
    Error::plain(
        code,
        reason,
        (!e.subject.is_empty()).then_some(e.subject.as_str()),
    )
}

fn call(code: Code, reason: &'static str, name: &str, errno: Option<i32>) -> Error {
    let mut detail = String::new();
    if detail.try_reserve_exact(name.len() + 32).is_err() {
        return Error::plain(Code::Host, "host_memory", None);
    }
    detail.push_str(name);
    if let Some(errno) = errno {
        detail.push(' ');
        let named = [
            (libc::EIO, "EIO"),
            (libc::ENOENT, "ENOENT"),
            (libc::EACCES, "EACCES"),
            (libc::EPERM, "EPERM"),
            (libc::EINVAL, "EINVAL"),
            (libc::ENOSYS, "ENOSYS"),
            (libc::ENOSPC, "ENOSPC"),
            (libc::EDQUOT, "EDQUOT"),
            (libc::ENOTDIR, "ENOTDIR"),
            (libc::ELOOP, "ELOOP"),
            (libc::EMFILE, "EMFILE"),
            (libc::ENFILE, "ENFILE"),
            (libc::ENOMEM, "ENOMEM"),
            (libc::EROFS, "EROFS"),
            (libc::EEXIST, "EEXIST"),
            (libc::EBADF, "EBADF"),
        ]
        .into_iter()
        .find(|(n, _)| *n == errno)
        .map(|(_, name)| name);
        if let Some(name) = named {
            detail.push_str(name);
        } else {
            let _ = write!(detail, "{errno}");
        }
    }
    Error {
        code,
        reason,
        detail: Some(detail),
        status: None,
    }
}
