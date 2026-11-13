//! Step 5: winner join, independent valid-path selection, then display sort.
//! Duplicate losers keep their reason even if their keeper is a prefix.
use super::{
    Context, Pass, State,
    member::{self, By, Codec, Member},
    stream::{self, Cursor},
};
use crate::{
    phase::Error,
    sort::{self, Control, Frame, Order, Source},
    spool::workspace::Sealed,
};

pub(super) struct Frames<'a> {
    reader: Option<sort::Reader<'a>>,
    frame: Frame,
    present: bool,
    unordered: bool,
}
impl<'a> Frames<'a> {
    pub fn open(
        file: Option<&Sealed>,
        order: &'a dyn Order,
        cx: &Context<'_>,
        unordered: bool,
    ) -> Result<Self, Error> {
        let mut out = Self {
            reader: file
                .map(|f| sort::Reader::open(cx.workspace, f, order, cx.account.meter(), cx.stopped))
                .transpose()?,
            frame: Frame::new(cx.account.meter())?,
            present: false,
            unordered,
        };
        out.advance(cx.stopped)?;
        Ok(out)
    }
    pub fn bytes(&self) -> Option<&[u8]> {
        self.present.then_some(self.frame.bytes())
    }
    pub fn member(&self) -> Result<Option<Member<'_>>, Error> {
        self.bytes().map(member::decode).transpose()
    }
    pub fn advance(&mut self, stopped: &dyn Fn() -> bool) -> Result<(), Error> {
        let next = self
            .reader
            .as_mut()
            .map(|r| {
                if self.unordered {
                    r.next_unordered(stopped)
                } else {
                    r.next(stopped)
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
}
impl Source for Frames<'_> {
    fn next(&mut self, out: &mut Frame, stopped: &dyn Fn() -> bool) -> Result<bool, Error> {
        let Some(bytes) = self.bytes() else {
            return Ok(false);
        };
        out.set(bytes)?;
        self.advance(stopped)?;
        Ok(true)
    }
}
pub(super) struct Output {
    writer: Option<sort::format::Writer>,
    codec: Codec,
    by: By,
}
impl Output {
    pub fn new(by: By, cx: &Context<'_>) -> Result<Self, Error> {
        Ok(Self {
            writer: None,
            codec: Codec::new(cx.account.meter())?,
            by,
        })
    }
    pub fn put(&mut self, row: Member<'_>, cx: &mut Context<'_>) -> Result<(), Error> {
        let control = Control {
            stopped: cx.stopped,
            hooks: cx.hooks,
        };
        if self.writer.is_none() {
            self.writer = Some(sort::format::Writer::new(
                cx.workspace,
                self.by.tag(),
                cx.account.meter(),
                &control,
            )?);
        }
        self.writer
            .as_mut()
            .ok_or_else(Error::internal)?
            .frame(self.codec.encode(row)?, &control)
    }
    pub fn finish(self, cx: &Context<'_>) -> Result<Option<Sealed>, Error> {
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
fn reorder(
    cx: &mut Context<'_>,
    file: Option<Sealed>,
    from: By,
    to: By,
) -> Result<Option<Sealed>, Error> {
    let mut source = Frames::open(file.as_ref(), &from, cx, true)?;
    let sorted = cx.sort(&mut source, to)?;
    drop(source);
    stream::retire(cx.workspace, file, cx.stopped)?;
    Ok(sorted)
}
pub(super) fn select(
    cx: &mut Context<'_>,
    winners: Option<&Sealed>,
) -> Result<Option<Sealed>, Error> {
    cx.boundary(Pass::Expansions)?;
    let expansions = {
        let mut source = super::source::Members::new(cx)?;
        cx.sort(&mut source, By::Identity)?
    };
    let mut expansions_read = Frames::open(expansions.as_ref(), &By::Identity, cx, false)?;
    let mut winners = Cursor::new(winners, &stream::By::Key, cx)?;
    let mut joined = Output::new(By::Select, cx)?;
    while let Some(mut m) = expansions_read.member()? {
        while winners.row()?.is_some_and(|w| w.key < m.key) {
            winners.advance(cx.stopped)?;
        }
        if let Some(w) = winners
            .row()?
            .filter(|w| w.key == m.key && w.input == m.input)
        {
            if w.state != State::Publish || w.md5 != m.md5 || w.source != m.source {
                return Err(Error::internal());
            }
            m.plan_index = w.plan_index;
            m.size = w.size;
            m.crc32 = w.crc32;
            joined.put(m, cx)?;
        }
        expansions_read.advance(cx.stopped)?;
    }
    drop((expansions_read, winners));
    stream::retire(cx.workspace, expansions, cx.stopped)?;
    let joined = joined.finish(cx)?;
    let joined = reorder(cx, joined, By::Select, By::Select)?;
    cx.boundary(Pass::Membership)?;
    let mut source = Frames::open(joined.as_ref(), &By::Select, cx, false)?;
    let mut result = Output::new(By::Display, cx)?;
    let mut keeper = Frame::new(cx.account.meter())?;
    while let Some(row) = source.member()? {
        if row.reason.is_some() {
            result.put(row, cx)?;
            source.advance(cx.stopped)?;
            continue;
        }
        keeper.set(source.bytes().ok_or_else(Error::internal)?)?;
        let mut kept = member::decode(keeper.bytes())?;
        source.advance(cx.stopped)?;
        while let Some(mut duplicate) = source
            .member()?
            .filter(|m| m.archive == kept.archive && m.path == kept.path && m.reason.is_none())
        {
            duplicate.reason = Some(crate::reason::LeftOut::DuplicateMember);
            result.put(duplicate, cx)?;
            source.advance(cx.stopped)?;
        }
        if source.member()?.is_some_and(|m| {
            m.archive == kept.archive
                && m.reason.is_none()
                && m.path
                    .strip_prefix(kept.path)
                    .is_some_and(|tail| tail.starts_with('/'))
        }) {
            kept.reason = Some(crate::reason::LeftOut::PrefixConflict);
        }
        result.put(kept, cx)?;
    }
    drop(source);
    stream::retire(cx.workspace, joined, cx.stopped)?;
    let result = result.finish(cx)?;
    cx.boundary(Pass::MemberRows)?;
    reorder(cx, result, By::Display, By::Display)
}
