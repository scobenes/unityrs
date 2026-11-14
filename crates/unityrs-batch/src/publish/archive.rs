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

/// Writeback is initiated at each 64 MiB of streamed archive output.
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
    // A stat-only size miss transfers at once; only a same-size regular file
    // is worth an expected-hash pass.
    if cx.resume && resume::sized(cx, input.key, input.size)? {
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
/// Destination-kind conflicts surface at the parent, at a no-follow pre-check
/// of the final name (a directory or another mount), both before any central
/// scratch or temp exists, or at the rename, after retirement. Any failure
/// but a stop removes an existing temp best-effort before returning; write
/// and writeback errors are run failures. Stops may leave the temp.
fn transfer(
    cx: &Context<'_>,
    input: &archive::Input<'_>,
    buffer: &mut [u8],
) -> Result<Code, Failure> {
    check(cx.stopped)?;
    let root = cx.directory.root();
    let parent = root.parent(input.key, true, sink::Context::Output)?;
    let basename = input.key.rsplit('/').next().ok_or_else(Error::internal)?;
    let name = sink::path::Name::new(basename)?;
    match root.fs().entry(&parent.file, name.cstr()) {
        Ok(entry) if entry.directory || !root.matches_mount(entry.mount) => {
            return Err(sink::Error::plain(sink::Class::PublishFailed, "fstatat").into());
        }
        Ok(_) => {}
        Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {}
        Err(e) => return Err(sink::Error::io(sink::Context::Output, "fstatat", &e).into()),
    }
    let ordinal = cx
        .handoff
        .counts
        .winners
        .checked_add(u64::from(input.index))
        .ok_or_else(Error::internal)?;
    let temp = parent.temporary(cx.run_id, ordinal, sink::Context::Output)?;
    let summary = match stream(cx, input, &temp, buffer) {
        Ok(summary) => summary,
        // A stop may leave its temp; the next attempt's create replaces it.
        Err(Failure::Run(Error::Stopped)) => return Err(Error::Stopped.into()),
        Err(e) => {
            temp.discard();
            return Err(e);
        }
    };
    cx.at(Point::BeforeCommit)?;
    let _guard = cx.gate.lock();
    check(cx.stopped)?;
    cx.hooks.point(Point::Authorized);
    // No stop check after authorization: rename and its outcome are one gate.
    if let Err(e) = temp.rename(basename) {
        temp.discard();
        return Err(e.into());
    }
    cx.hooks.point(Point::Outcome);
    cx.outcomes.set_archive(
        u64::from(input.index),
        &Record::success(Code::Written, summary.size, summary.md5, None)?,
    )?;
    Ok(Code::Written)
}
/// The transfer pass. Writeback starts each time the bytes written reach the
/// next multiple of 64 MiB, and once more after the stream completes, so an
/// archive of `size` bytes starts exactly `size / 64 MiB + 1` writebacks.
fn stream(
    cx: &Context<'_>,
    input: &archive::Input<'_>,
    temp: &sink::path::Temporary<'_>,
    buffer: &mut [u8],
) -> Result<archive::Summary, Failure> {
    let mut stream = archive::open(stream_context(cx), *input, cx.stopped)?;
    let (mut written, mut next) = (0_u64, WRITEBACK);
    loop {
        cx.at(Point::Buffer)?;
        let n = stream.read(buffer, cx.stopped)?;
        if n == 0 {
            break;
        }
        check(cx.stopped)?;
        temp.write_all(buffer.get(..n).ok_or_else(Error::internal)?)?;
        written += u64::try_from(n).map_err(|_| Error::internal())?;
        // Reads are at most 64 KiB, so one read crosses at most one multiple.
        if written >= next {
            temp.writeback()?;
            next = next.checked_add(WRITEBACK).ok_or_else(Error::internal)?;
        }
    }
    let summary = stream.finish(cx.stopped)?;
    temp.writeback()?;
    Ok(summary)
}
