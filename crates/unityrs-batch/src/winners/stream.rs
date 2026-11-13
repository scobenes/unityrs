//! Fixed charged cursors and canonical stream orders for the external joins.
use super::codec::{Codec, Row, decode};
use crate::{
    phase::{Account, Error},
    sort::{self, Control, Frame, Order, Reader},
    spool::workspace::{Sealed, Workspace},
};
use std::cmp::Ordering;

#[derive(Clone, Copy)]
pub enum By {
    Key,
    Instance,
    Member,
    Claim,
    Unit,
    Slash,
}
impl Order for By {
    fn tag(&self) -> u32 {
        match self {
            Self::Key => 101,
            Self::Instance => 102,
            Self::Member => 103,
            Self::Claim => 104,
            Self::Unit => 105,
            Self::Slash => 106,
        }
    }
    fn validate(&self, bytes: &[u8]) -> Result<(), Error> {
        decode(bytes).map(|_| ())
    }
    fn compare(&self, a: &[u8], b: &[u8]) -> Ordering {
        let (Ok(a), Ok(b)) = (decode(a), decode(b)) else {
            return a.cmp(b);
        };
        match self {
            Self::Key => a.key.cmp(b.key).then(a.input.cmp(b.input)),
            Self::Instance => a
                .unit
                .cmp(b.unit)
                .then(a.input.cmp(b.input))
                .then((a.key != a.unit).cmp(&(b.key != b.unit)))
                .then(a.key.cmp(b.key)),
            Self::Member | Self::Unit => a
                .unit
                .cmp(b.unit)
                .then(a.key.cmp(b.key))
                .then(a.input.cmp(b.input)),
            Self::Claim => a.key.cmp(b.key).then(a.unit.cmp(b.unit)),
            Self::Slash => sort::slash_first(a.key.as_bytes(), b.key.as_bytes())
                .then(b.archive.cmp(&a.archive)),
        }
    }
}
/// One lookahead, never one allocation per record.
pub struct Cursor<'a> {
    reader: Option<Reader<'a>>,
    frame: Frame,
    present: bool,
    ordered: bool,
}
impl<'a> Cursor<'a> {
    pub fn new(
        file: Option<&Sealed>,
        order: &'a By,
        cx: &super::Context<'_>,
    ) -> Result<Self, Error> {
        Self::open(file, order, cx, true)
    }
    pub(super) fn unordered(
        file: Option<&Sealed>,
        order: &'a By,
        cx: &super::Context<'_>,
    ) -> Result<Self, Error> {
        Self::open(file, order, cx, false)
    }
    fn open(
        file: Option<&Sealed>,
        order: &'a By,
        cx: &super::Context<'_>,
        ordered: bool,
    ) -> Result<Self, Error> {
        Self::read(
            cx.workspace,
            file,
            order,
            cx.account.meter(),
            cx.stopped,
            ordered,
        )
    }
    /// Completed-stream reader independent of the selection context.
    pub fn read(
        workspace: &Workspace,
        file: Option<&Sealed>,
        order: &'a By,
        meter: &std::sync::Arc<unityrs_core::limits::Meter>,
        stopped: &dyn Fn() -> bool,
        ordered: bool,
    ) -> Result<Self, Error> {
        let reader = file
            .map(|f| Reader::open(workspace, f, order, meter, stopped))
            .transpose()?;
        let mut out = Self {
            reader,
            frame: Frame::new(meter)?,
            present: false,
            ordered,
        };
        out.advance(stopped)?;
        Ok(out)
    }
    pub fn advance(&mut self, stopped: &dyn Fn() -> bool) -> Result<(), Error> {
        let next = self
            .reader
            .as_mut()
            .map(|r| {
                if self.ordered {
                    r.next(stopped)
                } else {
                    r.next_unordered(stopped)
                }
            })
            .transpose()?
            .flatten();
        self.present = next.is_some();
        if let Some(bytes) = next {
            self.frame.set(bytes)?;
        }
        Ok(())
    }
    pub fn row(&self) -> Result<Option<Row<'_>>, Error> {
        if self.present {
            decode(self.frame.bytes()).map(Some)
        } else {
            Ok(None)
        }
    }
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.frame.bytes()
    }
}
impl sort::Source for Cursor<'_> {
    fn next(&mut self, out: &mut Frame, stopped: &dyn Fn() -> bool) -> Result<bool, Error> {
        if !self.present {
            return Ok(false);
        }
        out.set(self.frame.bytes())?;
        self.advance(stopped)?;
        Ok(true)
    }
}
pub(super) struct Output {
    writer: Option<sort::format::Writer>,
    codec: Codec,
    tag: u32,
}
impl Output {
    pub fn new(order: By, account: &Account) -> Result<Self, Error> {
        Ok(Self {
            writer: None,
            codec: Codec::new(account.meter())?,
            tag: order.tag(),
        })
    }
    pub fn put(&mut self, row: Row<'_>, cx: &mut super::Context<'_>) -> Result<(), Error> {
        let control = Control {
            stopped: cx.stopped,
            hooks: cx.hooks,
        };
        if self.writer.is_none() {
            self.writer = Some(sort::format::Writer::new(
                cx.workspace,
                self.tag,
                cx.account.meter(),
                &control,
            )?);
        }
        self.writer
            .as_mut()
            .ok_or_else(Error::internal)?
            .frame(self.codec.encode(row)?, &control)
    }
    pub fn finish(self, cx: &super::Context<'_>) -> Result<Option<Sealed>, Error> {
        self.writer
            .map(|w| {
                w.finish(&Control {
                    stopped: cx.stopped,
                    hooks: cx.hooks,
                })
            })
            .transpose()
    }
}
pub(super) fn retire(
    workspace: &mut Workspace,
    file: Option<Sealed>,
    stopped: &dyn Fn() -> bool,
) -> Result<(), Error> {
    if let Some(file) = file {
        workspace.retire(file, stopped)?;
    }
    Ok(())
}
