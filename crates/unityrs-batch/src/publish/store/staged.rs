//! Transactional merge decisions, in bounded disk scratch. LIST failure must
//! change no outcome (batch 2.9); Drop retains partial claims for owned cleanup.
use super::{Context, Listed, Target};
use crate::{
    phase::{self, Error},
    spool::workspace::Independent,
    store::etag::Etag,
};
use std::io::Read;
use unityrs_core::limits::Charge;
const RECORD: usize = 128;
pub(super) struct Staged {
    file: Option<Independent>,
    _charge: Charge,
}
impl Staged {
    pub(super) fn new(cx: &Context<'_, '_>) -> Result<Self, Error> {
        // Generated paths and boxed file adapters, before any creation/open.
        let charge = cx.account.meter().charge(cx.workspace.adapter_charge()?)?;
        Ok(Self {
            file: None,
            _charge: charge,
        })
    }
    pub(super) fn push(
        &mut self,
        cx: &Context<'_, '_>,
        target: Target,
        listed: Option<Listed>,
    ) -> Result<(), Error> {
        phase::check(cx.stopped)?;
        let mut bytes = [0; RECORD];
        let (kind, index) = match target {
            Target::Loose(i) => (0, i),
            Target::Archive(i) => (1, i),
        };
        bytes[0] = kind;
        bytes[1..9].copy_from_slice(&index.to_le_bytes());
        if let Some(listed) = listed {
            bytes[9..17].copy_from_slice(&listed.size.to_le_bytes());
            let tag = listed.etag.as_bytes();
            bytes[17] = u8::try_from(tag.len()).map_err(|_| Error::internal())?;
            bytes
                .get_mut(18..18 + tag.len())
                .ok_or_else(Error::internal)?
                .copy_from_slice(tag);
        }
        if self.file.is_none() {
            self.file = Some(cx.workspace.create_independent(cx.stopped)?);
        }
        self.file
            .as_mut()
            .ok_or_else(Error::internal)?
            .append(&bytes, cx.stopped)?;
        Ok(())
    }
    pub(super) fn discard(self, cx: &Context<'_, '_>) -> Result<(), Error> {
        if let Some(file) = self.file {
            file.retire(cx.stopped)?;
        }
        Ok(())
    }
    pub(super) fn apply(
        mut self,
        cx: &Context<'_, '_>,
        mut put: impl FnMut(Target, Listed) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let Some(mut file) = self.file.take() else {
            return Ok(());
        };
        file.seal(cx.stopped)?;
        let mut reader = file.open(cx.stopped)?;
        for _ in 0..file.length() / RECORD as u64 {
            phase::check(cx.stopped)?;
            let mut bytes = [0; RECORD];
            reader.read_exact(&mut bytes)?;
            phase::check(cx.stopped)?;
            let index = u64::from_le_bytes(bytes[1..9].try_into().map_err(|_| Error::internal())?);
            let target = match bytes[0] {
                0 => Target::Loose(index),
                1 => Target::Archive(index),
                _ => return Err(Error::internal()),
            };
            let size = u64::from_le_bytes(bytes[9..17].try_into().map_err(|_| Error::internal())?);
            let etag = bytes
                .get(18..18 + usize::from(bytes[17]))
                .and_then(Etag::new)
                .ok_or_else(Error::internal)?;
            put(target, Listed { etag, size })?;
        }
        drop(reader);
        file.retire(cx.stopped)?;
        Ok(())
    }
}
