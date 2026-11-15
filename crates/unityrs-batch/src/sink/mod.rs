//! Descriptor-rooted directory safety. No output inventory is retained.
pub mod directory;
pub mod store;

#[derive(Clone)]
pub enum Sink {
    Directory(std::sync::Arc<directory::Directory>),
    Store(crate::sys::Shared<store::Store>),
}
impl From<std::sync::Arc<directory::Directory>> for Sink {
    fn from(value: std::sync::Arc<directory::Directory>) -> Self {
        Self::Directory(value)
    }
}
impl Sink {
    pub fn directory(&self) -> Result<&directory::Directory, crate::phase::Error> {
        match self {
            Self::Directory(d) => Ok(d),
            Self::Store(_) => Err(crate::phase::Error::internal()),
        }
    }
}

pub mod fs;
pub mod path;
mod probe;
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
    SinkUnreachable,
    BucketMissing,
    PreviousAttemptLive,
    CredentialsExpired,
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
#[derive(Clone, Copy, Debug)]
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
