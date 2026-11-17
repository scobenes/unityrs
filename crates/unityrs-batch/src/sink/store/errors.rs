//! Closed error mappings; no dependency error, endpoint or response renders.
use crate::{
    phase::Error,
    store::{attempt::Fault, head, put},
};
pub(crate) fn fault(error: Fault) -> Error {
    match error {
        Fault::Memory => Error::memory(),
        Fault::Internal => Error::internal(),
    }
}
pub(crate) fn phase(error: Error) -> super::super::Error {
    use super::super::{Class, Error as SinkError};
    SinkError::plain(
        match error {
            Error::Run(f) if f.reason == crate::attempt::ReasonCode::HostMemory => {
                Class::HostMemory
            }
            _ => Class::InvalidPath,
        },
        "store",
    )
}
pub(crate) fn head(error: head::Error) -> super::super::Error {
    use super::super::{Class, Error as SinkError};
    let class = match error {
        head::Error::Operation(put::Error::Fault(f)) => return phase(fault(f)),
        head::Error::Operation(put::Error::Phase(e)) => return phase(e),
        head::Error::BucketMissing => Class::BucketMissing,
        head::Error::ManifestUnreadable => Class::ManifestUnreadable,
        head::Error::PreviousAttemptLive => Class::PreviousAttemptLive,
        head::Error::Superseded => Class::Superseded,
        head::Error::PublishFailed => Class::PublishFailed,
        _ => Class::SinkUnreachable,
    };
    SinkError::plain(class, "store")
}
