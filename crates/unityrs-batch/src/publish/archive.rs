//! Directory archive publication (B8b). One canonical stream per pass feeds
//! either discard hashing (resume) or the sibling temp; both passes retire
//! their central scratch before returning, so a resume miss transfers with at
//! most one central file live per drive. Success needs B7's acknowledgment:
//! `present` under the gate, or an authorized rename plus its outcome write.
use super::{Context, Point, resume, sink_error};
use crate::{
    archive::{self, Eligibility},
    outcomes::{Archive as Record, Code},
    phase::{Buffer, Error, check},
    sink,
    spool::workspace::Sealed,
    winners::archives,
};
use std::sync::Arc;
use unityrs_core::limits::Meter;

/// Writeback is initiated after each 64 MiB of streamed archive output.
pub const WRITEBACK: u64 = 64 * 1024 * 1024;
const KEY: usize = 1024;

/// One archive's bounded identity, copied from the metadata reader's frame so
/// no borrowed frame outlives its reader lock. The list capability moves here.
pub struct Descriptor {
    key: Buffer<u8>,
    index: u32,
    members: u64,
    size: u64,
    list: Option<Sealed>,
}
impl Descriptor {
    /// One charged key slot per drive, reused for every archive it takes.
    pub fn new(meter: &Arc<Meter>) -> Result<Self, Error> {
        Ok(Self {
            key: Buffer::new(meter, KEY)?,
            index: 0,
            members: 0,
            size: 0,
            list: None,
        })
    }
    pub fn set(
        &mut self,
        key: &str,
        index: u32,
        members: u64,
        size: u64,
        list: Option<Sealed>,
    ) -> Result<(), Error> {
        self.key.clear();
        self.key.extend(key.as_bytes())?;
        (self.index, self.members, self.size, self.list) = (index, members, size, list);
        Ok(())
    }
    pub(super) fn take(&mut self, archive: archives::Archive<'_>) -> Result<(), Error> {
        self.set(
            archive.key,
            archive.index,
            archive.members,
            archive.size,
            archive.list,
        )
    }
    #[must_use]
    pub const fn eligibility(&self, blocked: bool) -> Eligibility {
        archive::eligibility(self.members, blocked, self.size)
    }
    fn key(&self) -> Result<&str, Error> {
        std::str::from_utf8(&self.key).map_err(|_| Error::internal())
    }
}

/// `None` for an archive with no outcome of its own (`empty`, `withheld`).
/// Precedence follows `archive::eligibility`: empty, then global withholding,
/// then oversized, which is refused before any payload, scratch or temp.
pub fn one(
    cx: &Context<'_>,
    archive: &Descriptor,
    buffer: &mut [u8],
) -> Result<Option<Code>, Error> {
    let index = u64::from(archive.index);
    match archive.eligibility(cx.handoff.publication_blocked) {
        Eligibility::Empty | Eligibility::Withheld => return Ok(None),
        Eligibility::Oversized => {
            let _guard = cx.gate.lock();
            check(cx.stopped)?;
            cx.outcomes
                .set_archive(index, &Record::failed(Code::ArchiveTooLarge)?)?;
            return Ok(Some(Code::ArchiveTooLarge));
        }
        Eligibility::Eligible => {}
    }
    let input = archive::Input {
        key: archive.key()?,
        index: archive.index,
        members: archive.members,
        size: archive.size,
        list: archive.list.as_ref().ok_or_else(Error::internal)?,
    };
    cx.at(Point::Transfer)?;
    if cx.resume {
        let expected = hash(cx, &input, buffer)?;
        let found = resume::present(
            cx,
            resume::Expected {
                key: input.key,
                size: expected.size,
                md5: expected.md5,
            },
            buffer,
        )?;
        // The expected-hash pass retired its central file inside `finish`.
        cx.at(Point::ResumeReleased)?;
        if found {
            let _guard = cx.gate.lock();
            check(cx.stopped)?;
            cx.outcomes.set_archive(
                index,
                &Record::success(Code::Present, expected.size, expected.md5, None)?,
            )?;
            return Ok(Some(Code::Present));
        }
    }
    match transfer(cx, &input, buffer) {
        Ok(code) => Ok(Some(code)),
        Err(Failure::Run(e)) => Err(e),
        Err(Failure::Sink(e)) if e.class == sink::Class::PublishFailed => {
            let _guard = cx.gate.lock();
            check(cx.stopped)?;
            cx.outcomes
                .set_archive(index, &Record::failed(Code::PublishFailed)?)?;
            Ok(Some(Code::PublishFailed))
        }
        Err(Failure::Sink(e)) => Err(sink_error(e)),
    }
}
fn stream_context<'a>(cx: &'a Context<'_>) -> archive::Context<'a> {
    archive::Context {
        workspace: cx.workspace,
        payloads: cx.payloads,
        meter: cx.account.meter(),
        hooks: &archive::NoHooks,
    }
}
/// The expected-hash pass: the complete stream into discard storage.
fn hash(
    cx: &Context<'_>,
    input: &archive::Input<'_>,
    buffer: &mut [u8],
) -> Result<archive::Summary, Error> {
    let mut stream = archive::open(stream_context(cx), *input, cx.stopped)?;
    loop {
        cx.at(Point::Buffer)?;
        if stream.read(buffer, cx.stopped)? == 0 {
            break;
        }
    }
    stream.finish(cx.stopped)
}
enum Failure {
    Run(Error),
    Sink(sink::Error),
}
impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        Self::Run(e)
    }
}
impl From<sink::Error> for Failure {
    fn from(e: sink::Error) -> Self {
        Self::Sink(e)
    }
}
/// Destination-kind conflicts surface at the parent/temp, before any central
/// scratch exists, or at the rename, after retirement. Sink write and
/// writeback errors are run failures, so no failed pass is ever skipped over.
fn transfer(
    cx: &Context<'_>,
    input: &archive::Input<'_>,
    buffer: &mut [u8],
) -> Result<Code, Failure> {
    check(cx.stopped)?;
    let parent = cx
        .directory
        .root()
        .parent(input.key, true, sink::Context::Output)?;
    let ordinal = cx
        .handoff
        .counts
        .winners
        .checked_add(u64::from(input.index))
        .ok_or_else(Error::internal)?;
    let temp = parent.temporary(cx.run_id, ordinal, sink::Context::Output)?;
    let mut stream = archive::open(stream_context(cx), *input, cx.stopped)?;
    let mut pending = 0_u64;
    loop {
        cx.at(Point::Buffer)?;
        let n = stream.read(buffer, cx.stopped)?;
        if n == 0 {
            break;
        }
        check(cx.stopped)?;
        temp.write_all(buffer.get(..n).ok_or_else(Error::internal)?)?;
        pending += u64::try_from(n).map_err(|_| Error::internal())?;
        if pending >= WRITEBACK {
            temp.writeback()?;
            pending = 0;
        }
    }
    let summary = stream.finish(cx.stopped)?;
    temp.writeback()?;
    cx.at(Point::BeforeCommit)?;
    let _guard = cx.gate.lock();
    check(cx.stopped)?;
    cx.hooks.point(Point::Authorized);
    // No stop check after authorization: rename and its outcome are one gate.
    temp.rename(input.key.rsplit('/').next().ok_or_else(Error::internal)?)?;
    cx.hooks.point(Point::Outcome);
    cx.outcomes.set_archive(
        u64::from(input.index),
        &Record::success(Code::Written, summary.size, summary.md5, None)?,
    )?;
    Ok(Code::Written)
}
