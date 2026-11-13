use super::{Context, Point, sink_error};
use crate::{
    outcomes::Code,
    phase::{Error, check},
    sink,
    winners::Row,
};
use md5::{Digest, Md5};
use std::io::Read;

pub(super) fn one(
    cx: &Context<'_>,
    row: Row<'_>,
    index: u64,
    buffer: &mut [u8],
) -> Result<Code, Error> {
    let result = transfer(cx, row, index, buffer);
    match result {
        Ok(code) => Ok(code),
        Err(Failure::Run(e)) => Err(e),
        Err(Failure::Sink(e)) if e.class == sink::Class::PublishFailed => {
            let _guard = cx.gate.lock();
            check(cx.stopped)?;
            cx.outcomes.set(index, Code::PublishFailed)?;
            Ok(Code::PublishFailed)
        }
        Err(Failure::Sink(e)) => Err(sink_error(e)),
    }
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
fn present(cx: &Context<'_>, row: Row<'_>, buffer: &mut [u8]) -> Result<bool, Error> {
    let found = super::resume::present(cx, row, buffer)?;
    // The serial hashing pass releases its loan before transfer uses the same
    // storage; the driver's single fallible reservation remains charged.
    cx.at(Point::ResumeReleased)?;
    Ok(found)
}
fn transfer(
    cx: &Context<'_>,
    row: Row<'_>,
    index: u64,
    buffer: &mut [u8],
) -> Result<Code, Failure> {
    cx.at(Point::Transfer)?;
    if cx.resume && present(cx, row, buffer)? {
        let _guard = cx.gate.lock();
        check(cx.stopped)?;
        cx.outcomes.set(index, Code::Present)?;
        return Ok(Code::Present);
    }
    check(cx.stopped)?;
    let mut source = cx.payloads.open(row.source, row.plan_index)?;
    let parent = cx
        .directory
        .root()
        .parent(row.key, true, sink::Context::Output)?;
    let temp = parent.temporary(cx.run_id, index, sink::Context::Output)?;
    let mut size = 0_u64;
    let mut md5 = Md5::new();
    let mut crc = crc32fast::Hasher::new();
    loop {
        cx.at(Point::Buffer)?;
        let n = crate::sys::retry_interrupted(|| source.read(buffer)).map_err(Error::from)?;
        if n == 0 {
            break;
        }
        let bytes = buffer.get(..n).ok_or_else(Error::internal)?;
        size = size
            .checked_add(u64::try_from(n).map_err(|_| Error::internal())?)
            .ok_or_else(Error::internal)?;
        if size > row.size {
            return Err(Error::internal().into());
        }
        md5.update(bytes);
        crc.update(bytes);
        check(cx.stopped)?;
        temp.write_all(bytes)?;
    }
    let hash: [u8; 16] = md5.finalize().into();
    if size != row.size || hash != row.md5 || crc.finalize() != row.crc32 {
        return Err(Error::internal().into());
    }
    temp.writeback()?;
    cx.at(Point::BeforeCommit)?;
    let _guard = cx.gate.lock();
    check(cx.stopped)?;
    cx.hooks.point(Point::Authorized);
    // No stop check after authorization: rename and its outcome are one gate.
    temp.rename(row.key.rsplit('/').next().ok_or_else(Error::internal)?)?;
    cx.hooks.point(Point::Outcome);
    cx.outcomes.set(index, Code::Written)?;
    Ok(Code::Written)
}
