//! The finisher's synchronous row destination (spec 7.1; batch 2.11).
//! It runs only on the finisher's own thread outside any runtime context.
use super::{
    attempt::Fault,
    put::{self, Payload, Request},
    scan::Code,
};
use crate::{
    diagnostic::Secrets,
    manifest::{Family, WriteError, chunks},
    phase::{Account, Buffer, Error},
};
use object_store::path::Path;
use std::fmt::Write;

pub struct Destination<'a> {
    pub runtime: &'a tokio::runtime::Runtime,
    pub request: Request<'a>,
    pub account: &'a Account,
    pub prefix: &'a str,
    pub run_id: &'a str,
    pub attempt: u32,
}
impl Destination<'_> {
    fn key(&self, family: Family, chunk: u64) -> Result<Path, WriteError> {
        let mut key = String::new();
        key.try_reserve_exact(self.prefix.len() + self.run_id.len() + 96)
            .map_err(|_| Error::memory())?;
        write!(
            key,
            "{}_unityrs/{}/{}/{}.{}.ndjson",
            self.prefix,
            self.run_id,
            self.attempt,
            family.name(),
            chunk
        )
        .map_err(|_| Error::internal())?;
        Path::parse(key).map_err(|_| Error::internal().into())
    }
    fn send(
        &mut self,
        family: Family,
        chunk: u64,
        bytes: hyper::body::Bytes,
    ) -> Result<(), WriteError> {
        // A runtime nested inside block_on would panic, including in release.
        if tokio::runtime::Handle::try_current().is_ok() {
            return Err(Error::internal().into());
        }
        let key = self.key(family, chunk)?;
        self.runtime
            .block_on(self.request.bytes(&key, bytes, "application/x-ndjson", b""))
            .map(drop)
            .map_err(map)
    }
}
pub(crate) fn map(error: put::Error) -> WriteError {
    match error {
        put::Error::Fault(Fault::Memory) => Error::memory().into(),
        put::Error::Fault(Fault::Internal) => Error::internal().into(),
        put::Error::Phase(e) => e.into(),
        _ => WriteError::Publish,
    }
}
impl chunks::Destination for Destination<'_> {
    /// Borrowed callers explicitly pay for an owning copy. The chunker uses
    /// `publish_owned`, keeping its 16 MiB buffer as the request's sole owner.
    fn publish(&mut self, family: Family, chunk: u64, bytes: &[u8]) -> Result<(), WriteError> {
        let payload = Payload::read(self.account, bytes.len(), &mut &bytes[..]).map_err(map)?;
        self.send(family, chunk, payload.into_bytes())
    }
    fn publish_owned(
        &mut self,
        family: Family,
        chunk: u64,
        bytes: Buffer<u8>,
    ) -> Result<Option<Buffer<u8>>, WriteError> {
        self.send(family, chunk, Payload::from_buffer(bytes).into_bytes())?;
        Ok(None)
    }
}

/// The only S3 code vocabulary available to a diagnostic row. Unknown or
/// credential-equal codes render as `other`; raw server bytes never render.
#[must_use]
pub fn code(code: Code, secrets: &Secrets<'_>) -> &'static str {
    const KNOWN: &[&str] = &[
        "NoSuchUpload",
        "ExpiredToken",
        "InternalError",
        "SlowDown",
        "AccessDenied",
        "NoSuchBucket",
        "NoSuchKey",
        "PreconditionFailed",
        "ConditionalRequestConflict",
        "SignatureDoesNotMatch",
        "RequestTimeTooSkewed",
        "InvalidAccessKeyId",
        "InvalidToken",
        "InvalidPart",
        "InvalidPartOrder",
        "EntityTooSmall",
        "MalformedXML",
        "InvalidRequest",
        "RequestTimeout",
        "ServiceUnavailable",
        "NotImplemented",
    ];
    for &name in KNOWN {
        if code.is(name) {
            if secrets
                .values
                .iter()
                .copied()
                .chain(secrets.credentials.get().into_iter().flatten().copied())
                .any(|secret| !secret.is_empty() && name.contains(secret))
            {
                return "other";
            }
            return name;
        }
    }
    "other"
}
