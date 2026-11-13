//! Committed enumeration through a frozen controller adapter. No spool paths
//! enter projections; locators are numeric input positions and output ordinals.
use super::{
    Context, Prepared,
    codec::{Codec, Row},
};
use crate::{
    phase::{Account, Error, check},
    records::read,
    sort::{Frame, Source},
    spec::model::Spec,
};
use std::io::Read;

/// Frozen commits only. Each position appears once, all are committed, and
/// failure facts are final. Tests implement this over synthetic B4 directories;
/// the controller's concrete adapter is wired at the B6c boundary.
pub trait Inputs {
    fn open(&self, input: u32) -> Result<Box<dyn Read + Send>, Error>;
    fn failed(&self, input: u32) -> Result<bool, Error>;
    fn position(&self, ordinal: u32) -> u32 {
        ordinal
    }
}
pub(super) struct Candidates<'a> {
    spec: &'a Spec,
    inputs: &'a dyn Inputs,
    reader: read::Reader<'a, Box<dyn Read + Send>>,
    next: u32,
    current: u32,
    units: bool,
    codec: Codec,
}
impl<'a> Candidates<'a> {
    pub fn new(
        spec: &'a Spec,
        inputs: &'a dyn Inputs,
        account: &'a Account,
        units: bool,
    ) -> Result<Self, Error> {
        let current = inputs.position(0);
        let index = usize::try_from(current).map_err(|_| Error::internal())?;
        if index >= spec.input_count() {
            return Err(Error::internal());
        }
        let id = spec.bundle(index).ok_or_else(Error::internal)?.id();
        let reader = read::Reader::new(
            inputs.open(current)?,
            id,
            inputs.failed(current)?,
            u64::try_from(spec.archives.len()).map_err(|_| Error::internal())?,
            account.meter(),
        )?;
        Ok(Self {
            spec,
            inputs,
            reader,
            next: 1,
            current,
            units,
            codec: Codec::new(account.meter())?,
        })
    }
}
impl Candidates<'_> {
    fn reopen(&mut self) -> Result<bool, Error> {
        if usize::try_from(self.next).map_err(|_| Error::internal())? == self.spec.input_count() {
            return Ok(false);
        }
        self.current = self.inputs.position(self.next);
        let index = usize::try_from(self.current).map_err(|_| Error::internal())?;
        if index >= self.spec.input_count() {
            return Err(Error::internal());
        }
        self.next = self.next.checked_add(1).ok_or_else(Error::internal)?;
        let id = self.spec.bundle(index).ok_or_else(Error::internal)?.id();
        self.reader.reopen(
            self.inputs.open(self.current)?,
            id,
            self.inputs.failed(self.current)?,
        )?;
        Ok(true)
    }
}
impl Source for Candidates<'_> {
    #[allow(clippy::collapsible_match)] // A guard would retain the reader borrow across reopen (E0499).
    fn next(&mut self, out: &mut Frame, stopped: &dyn Fn() -> bool) -> Result<bool, Error> {
        loop {
            check(stopped)?;
            let row = self.reader.next(stopped)?;
            match row {
                Some(read::Row::Candidate(c)) => {
                    unityrs_export::keys::check(c.key.as_bytes()).map_err(|_| Error::internal())?;
                    if let Some(unit) = c.unit {
                        unityrs_export::keys::check(unit.as_bytes())
                            .map_err(|_| Error::internal())?;
                    }
                    if !self.units || c.unit.is_some() {
                        out.set(self.codec.encode(Row::candidate(&c, self.current))?)?;
                        return Ok(true);
                    }
                }
                None => {
                    if !self.reopen()? {
                        return Ok(false);
                    }
                }
                _ => {}
            }
        }
    }
}
pub(super) fn sorted(
    cx: &mut Context<'_>,
    units: bool,
    by: super::stream::By,
) -> Result<Option<crate::spool::workspace::Sealed>, Error> {
    let mut source = Candidates::new(cx.spec, cx.inputs, cx.account, units)?;
    cx.sort(&mut source, by)
}
impl Prepared {
    pub(super) fn preference<'a>(
        &self,
        spec: &'a Spec,
        key: &str,
    ) -> Option<crate::spec::model::Prefer<'a>> {
        let index = self
            .preferences
            .binary_search_by(|&i| spec.prefer_at(i).map_or("", |p| p.0).cmp(key))
            .ok()?;
        spec.prefer_at(*self.preferences.get(index)?).map(|p| p.1)
    }
    pub(super) fn withheld(&self, cx: &Context<'_>, key: &str) -> Result<bool, Error> {
        let Some(input) = self.preference(cx.spec, key).and_then(|p| p.input) else {
            return Ok(false);
        };
        let index = self
            .inputs
            .binary_search_by(|&i| {
                cx.spec
                    .bundle(usize::try_from(i).unwrap_or(usize::MAX))
                    .map_or("", |b| b.id())
                    .cmp(input)
            })
            .map_err(|_| Error::internal())?;
        cx.inputs
            .failed(*self.inputs.get(index).ok_or_else(Error::internal)?)
    }
}

pub(super) struct Members<'a>(Candidates<'a>, super::member::Codec);
impl<'a> Members<'a> {
    pub fn new(cx: &Context<'a>) -> Result<Self, Error> {
        Ok(Self(
            Candidates::new(cx.spec, cx.inputs, cx.account, false)?,
            super::member::Codec::new(cx.account.meter())?,
        ))
    }
}
impl Source for Members<'_> {
    #[allow(clippy::collapsible_match)] // A guard would retain the reader borrow across reopen (E0499).
    fn next(&mut self, out: &mut Frame, stopped: &dyn Fn() -> bool) -> Result<bool, Error> {
        loop {
            check(stopped)?;
            match self.0.reader.next(stopped)? {
                Some(read::Row::Member(m)) => {
                    let index = u32::try_from(m.archive).map_err(|_| Error::internal())?;
                    let archive = self
                        .0
                        .spec
                        .archives
                        .get(usize::try_from(index).map_err(|_| Error::internal())?)
                        .ok_or_else(Error::internal)?;
                    let row = super::member::Member {
                        archive: &archive.key,
                        index,
                        path: m.path,
                        key: m.key,
                        input: m.input,
                        md5: m.md5,
                        reason: m.reason,
                        cut: m.cut,
                        source: self.0.current,
                        plan_index: 0,
                        size: 0,
                        crc32: 0,
                    };
                    out.set(self.1.encode(row)?)?;
                    return Ok(true);
                }
                None => {
                    if !self.0.reopen()? {
                        return Ok(false);
                    }
                }
                _ => {}
            }
        }
    }
}
