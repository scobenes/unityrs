//! Bounded resolver output; blocking closures, rather than handles, own tokens.
use super::{
    Error,
    permit::{Operation, OperationToken, RequestPermit},
};
use std::io;
use tokio::runtime::Handle;

pub use super::addresses::{Addresses, MAX_ADDRESSES};
/// Rust-side resolver storage, including the fallible host copy. The first
/// sixteen answers use fixed storage; NSS's C allocations have the separate
/// allowance in `batch.md` 5.4. This follows a timed-out closure to completion.
pub const LOOKUP_ENVELOPE: u64 = 512 * 1_024;
#[cfg(any(test, feature = "test-hooks"))]
pub trait Resolver: Send + Sync {
    fn resolve(&self, host: &str, port: u16) -> io::Result<Addresses>;
}
#[derive(Clone, Default)]
pub(crate) enum Source {
    #[default]
    System,
    #[cfg(any(test, feature = "test-hooks"))]
    Hook(std::sync::Arc<dyn Resolver>),
}
impl Source {
    fn resolve(&self, name: &[u8], port: u16) -> io::Result<Addresses> {
        match self {
            Self::System => crate::sys::resolve(name, port),
            #[cfg(any(test, feature = "test-hooks"))]
            Self::Hook(resolver) => {
                let host = name.strip_suffix(&[0]).ok_or(io::ErrorKind::InvalidInput)?;
                let host = std::str::from_utf8(host).map_err(|_| io::ErrorKind::InvalidInput)?;
                resolver.resolve(host, port)
            }
        }
    }
}
pub(crate) async fn lookup(
    pool: &Handle,
    permit: &mut RequestPermit,
    source: &Source,
    host: &str,
    port: u16,
    token: Option<OperationToken>,
) -> Result<Addresses, Error> {
    let charge = permit.charge(LOOKUP_ENVELOPE).ok_or(Error::Memory)?;
    if host.as_bytes().contains(&0) {
        return Err(super::classify::connect(
            &io::ErrorKind::InvalidInput.into(),
        ));
    }
    let capacity = host.len().checked_add(1).ok_or(Error::Memory)?;
    let mut name = Vec::new();
    name.try_reserve_exact(capacity)
        .map_err(|_| Error::Memory)?;
    name.extend_from_slice(host.as_bytes());
    name.push(0);
    let token = token
        .or_else(|| permit.begin(Operation::Lookup))
        .ok_or(Error::Busy)?;
    let source = source.clone();
    pool.spawn_blocking(move || {
        let result = {
            let source = source;
            source
                .resolve(&name, port)
                .map_err(|e| super::classify::connect(&e))
        };
        // All Rust host storage ends before the slot is made available again.
        drop(name);
        drop(charge);
        drop(token);
        result
    })
    .await
    .map_err(|_| Error::Pool)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{Detail, permit::PermitPool};

    #[test]
    fn interior_nul_is_connect_and_releases_the_lookup_charge() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let pool = PermitPool::new(1).unwrap();
        let mut permit = pool.acquire().unwrap();
        let error = runtime
            .block_on(lookup(
                runtime.handle(),
                &mut permit,
                &Source::System,
                "bad\0host",
                80,
                None,
            ))
            .unwrap_err();
        assert_eq!(error.detail(), Some(Detail::Connect));
        assert_eq!(permit.charged(), 0);
        assert_eq!(pool.snapshot().outstanding, 0);
    }
}
