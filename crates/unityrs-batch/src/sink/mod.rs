//! Descriptor-rooted directory safety. No output inventory is retained.
pub mod directory;
pub mod fs;
pub mod path;
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context {
    Preflight,
    Output,
    RunningHead,
    Finish,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    SinkRootUnusable,
    KernelTooOld,
    MountCheckUnavailable,
    MountCrossing,
    OutputLocked,
    ManifestUnreadable,
    Superseded,
    PublishFailed,
    HostIo,
    HostMemory,
    InvalidPath,
}
#[derive(Debug)]
pub struct Error {
    pub class: Class,
    pub call: &'static str,
    pub errno: Option<i32>,
}
impl Error {
    pub(crate) const fn plain(class: Class, call: &'static str) -> Self {
        Self {
            class,
            call,
            errno: None,
        }
    }
    pub(crate) fn io(context: Context, call: &'static str, e: &io::Error) -> Self {
        let class = if e.kind() == io::ErrorKind::OutOfMemory {
            Class::HostMemory
        } else if e.raw_os_error() == Some(libc::EXDEV) {
            match context {
                Context::Preflight => Class::MountCrossing,
                Context::Finish => Class::PublishFailed,
                _ => Class::HostIo,
            }
        } else {
            match context {
                Context::Preflight => Class::SinkRootUnusable,
                Context::RunningHead => Class::HostIo,
                Context::Finish => Class::PublishFailed,
                Context::Output => {
                    let kind = match call {
                        "openat" => matches!(
                            e.raw_os_error(),
                            Some(libc::ELOOP | libc::ENOTDIR | libc::EISDIR | libc::EEXIST)
                        ),
                        "renameat" => matches!(
                            e.raw_os_error(),
                            Some(libc::EISDIR | libc::ENOTEMPTY | libc::ENOTDIR | libc::EBUSY)
                        ),
                        "unlinkat" => {
                            (cfg!(target_os = "macos") && e.raw_os_error() == Some(libc::EPERM))
                                || matches!(e.raw_os_error(), Some(libc::EISDIR | libc::ENOTDIR))
                        }
                        "mkdirat" => matches!(e.raw_os_error(), Some(libc::EEXIST | libc::ENOTDIR)),
                        _ => false,
                    };
                    if kind {
                        Class::PublishFailed
                    } else {
                        Class::HostIo
                    }
                }
            }
        };
        Self {
            class,
            call,
            errno: e.raw_os_error(),
        }
    }
    #[must_use]
    pub fn code(&self) -> crate::reason::Code {
        use crate::reason::Code;
        match self.class {
            Class::Superseded => Code::Superseded,
            Class::PublishFailed => Code::Incomplete,
            Class::HostIo | Class::HostMemory => Code::Host,
            Class::InvalidPath => Code::Internal,
            _ => Code::CouldNotStart,
        }
    }
}
