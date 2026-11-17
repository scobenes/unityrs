//! `object_store` 0.14.2 accepts fallible digest/HMAC context constructors.
//! Keep ring's cryptography and replace its default provider's aborting
//! boxes with charged, fallible storage. A refusal is a try fault before
//! any request reaches the connector, never a retryable signing error.
use super::{
    attempt::{Fault, TrySlot},
    connector::Refused,
};
use crate::http::permit::Charge;
use object_store::client::{
    CryptoProvider, DigestAlgorithm, DigestContext, HmacContext, Signer, SigningAlgorithm,
};
use ring::{digest, hmac};
use unityrs_core::limits::heap_cost;

#[derive(Debug)]
pub(super) struct Provider;

fn failed(fault: Fault) -> object_store::Error {
    if let Some(slot) = TrySlot::current() {
        slot.fail(fault);
    }
    object_store::Error::Generic {
        store: "batch",
        // Refused is zero-sized: even this error path allocates nothing.
        source: Box::new(Refused),
    }
}

fn charge<T>() -> object_store::Result<Charge> {
    TrySlot::current()
        .ok_or(Fault::Internal)
        .and_then(|slot| slot.charge(heap_cost(size_of::<T>() as u64)))
        .map_err(failed)
}

impl CryptoProvider for Provider {
    fn digest(&self, algorithm: DigestAlgorithm) -> object_store::Result<Box<dyn DigestContext>> {
        let algorithm = match algorithm {
            DigestAlgorithm::Sha256 => &digest::SHA256,
            _ => return Err(failed(Fault::Internal)),
        };
        let context = Digest {
            ctx: Some(digest::Context::new(algorithm)),
            out: None,
            _charge: charge::<Digest>()?,
        };
        crate::alloc::try_box(context)
            .map(|context| context as Box<dyn DigestContext>)
            .map_err(|_| failed(Fault::Memory))
    }

    fn hmac(
        &self,
        algorithm: DigestAlgorithm,
        secret: &[u8],
    ) -> object_store::Result<Box<dyn HmacContext>> {
        let algorithm = match algorithm {
            DigestAlgorithm::Sha256 => hmac::HMAC_SHA256,
            _ => return Err(failed(Fault::Internal)),
        };
        let context = Hmac {
            ctx: Some(hmac::Context::with_key(&hmac::Key::new(algorithm, secret))),
            out: None,
            _charge: charge::<Hmac>()?,
        };
        crate::alloc::try_box(context)
            .map(|context| context as Box<dyn HmacContext>)
            .map_err(|_| failed(Fault::Memory))
    }

    fn sign(&self, _: SigningAlgorithm, _: &[u8]) -> object_store::Result<Box<dyn Signer>> {
        // S3 SigV4 uses SHA-256 and HMAC only; RSA is outside this client.
        Err(failed(Fault::Internal))
    }
}

struct Digest {
    ctx: Option<digest::Context>,
    out: Option<digest::Digest>,
    _charge: Charge,
}
impl DigestContext for Digest {
    fn update(&mut self, data: &[u8]) {
        if let Some(ctx) = &mut self.ctx {
            ctx.update(data);
        }
    }
    fn finish(&mut self) -> object_store::Result<&[u8]> {
        if let Some(ctx) = self.ctx.take() {
            self.out = Some(ctx.finish());
        }
        self.out
            .as_ref()
            .map(AsRef::as_ref)
            .ok_or_else(|| failed(Fault::Internal))
    }
}

struct Hmac {
    ctx: Option<hmac::Context>,
    out: Option<hmac::Tag>,
    _charge: Charge,
}
impl HmacContext for Hmac {
    fn update(&mut self, data: &[u8]) {
        if let Some(ctx) = &mut self.ctx {
            ctx.update(data);
        }
    }
    fn finish(&mut self) -> object_store::Result<&[u8]> {
        if let Some(ctx) = self.ctx.take() {
            self.out = Some(ctx.sign());
        }
        self.out
            .as_ref()
            .map(AsRef::as_ref)
            .ok_or_else(|| failed(Fault::Internal))
    }
}
