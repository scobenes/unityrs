//! Fixed job failure codes, shared by staging and sealed peeks.
use crate::{
    attempt::{DetailCode, ReasonCode},
    reason::Code,
    spool,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RunFailure {
    pub reason: ReasonCode,
    pub detail: Option<DetailCode>,
}
impl RunFailure {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self.reason {
            ReasonCode::HostIo => "host_io",
            ReasonCode::HostMemory => "host_memory",
            ReasonCode::SpoolFull => "spool_full",
            _ => "internal",
        }
    }
    #[must_use]
    pub const fn new(reason: ReasonCode) -> Self {
        Self {
            reason,
            detail: None,
        }
    }
    #[must_use]
    pub fn code(self) -> Code {
        match self.reason {
            ReasonCode::HostIo | ReasonCode::HostMemory => Code::Host,
            ReasonCode::SpoolFull => Code::Budget,
            _ => Code::Internal,
        }
    }
    /// Keep the more severe failure; in particular a leak/panic cannot be
    /// hidden by an allocator refusal observed during the same job.
    pub fn reduce(slot: &mut Option<Self>, failure: Self) {
        if slot.is_none_or(|old| failure.code().severity() < old.code().severity()) {
            *slot = Some(failure);
        }
    }
}
impl From<spool::Error> for RunFailure {
    fn from(error: spool::Error) -> Self {
        match error {
            spool::Error::Full(full) => Self {
                reason: ReasonCode::SpoolFull,
                detail: Some(match full {
                    spool::Full::SizeBytes => DetailCode::SizeBytes,
                    spool::Full::Reserve => DetailCode::Reserve,
                    spool::Full::Enospc => DetailCode::Enospc,
                    spool::Full::Edquot => DetailCode::Edquot,
                }),
            },
            spool::Error::HostIo(_) => Self::new(ReasonCode::HostIo),
            spool::Error::HostMemory => Self::new(ReasonCode::HostMemory),
            spool::Error::Internal | spool::Error::Stopped => Self::new(ReasonCode::Internal),
        }
    }
}

/// The bounded worker end channel's actual union. Stage copies complete on
/// the blocking pool; their ends are separate from this channel.
#[allow(clippy::large_enum_variant)] // Fixed inline storage is the contract.
#[derive(Clone, Copy, Debug)]
pub enum End {
    Attempt(crate::attempt::AttemptEnd),
    Peek(crate::peek::PeekEnd),
}
