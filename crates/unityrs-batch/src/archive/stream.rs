//! One current member, one central owner and fixed hash state. No payload cache.
use super::{
    Context, Eligibility, Input, Point, Summary, boundary,
    central::{Central, read_some},
    eligibility, headers,
};
use crate::{
    digest::Incremental,
    phase::{Buffer, Error, check},
    winners::{
        layout::{End, Layout},
        member,
    },
};
use md5::{Digest, Md5};
use std::io::Read;
use unityrs_core::limits::Charge;
const CHUNK: usize = 64 * 1024;

struct Payload {
    read: Box<dyn Read + Send>,
    remaining: u64,
    crc: crc32fast::Hasher,
    md5: Md5,
    expected_crc: u32,
    expected_md5: [u8; 16],
}
impl Payload {
    fn read(
        &mut self,
        out: &mut [u8],
        cx: &Context<'_>,
        stopped: &dyn Fn() -> bool,
    ) -> Result<usize, Error> {
        boundary(cx.hooks, Point::PayloadRead, stopped)?;
        if self.remaining == 0 {
            let mut byte = [0];
            if read_some(&mut self.read, &mut byte, stopped)? != 0 {
                return Err(Error::internal());
            }
            if self.crc.clone().finalize() != self.expected_crc
                || <[u8; 16]>::from(self.md5.clone().finalize()) != self.expected_md5
            {
                return Err(Error::internal());
            }
            return Ok(0);
        }
        let limit = usize::try_from(self.remaining.min(65_536))
            .map_err(|_| Error::internal())?
            .min(out.len());
        let output = out.get_mut(..limit).ok_or_else(Error::internal)?;
        let n = read_some(&mut self.read, output, stopped)?;
        if n == 0 {
            return Err(Error::internal());
        }
        let bytes = output.get(..n).ok_or_else(Error::internal)?;
        self.crc.update(bytes);
        self.md5.update(bytes);
        self.remaining -= u64::try_from(n).map_err(|_| Error::internal())?;
        Ok(n)
    }
}
#[derive(Clone, Copy)]
enum State {
    Members,
    Central,
    End,
    Done,
}
pub struct Stream<'a> {
    cx: Context<'a>,
    input: Input<'a>,
    pull: Pull,
}
pub(crate) struct Pull {
    members: Option<member::Reader<'static>>,
    central: Central,
    header: Buffer<u8>,
    position: usize,
    payload: Option<Payload>,
    layout: Layout,
    end: Option<End>,
    seen: u64,
    origin: u64,
    digest: Incremental,
    state: State,
    failed: bool,
    _adapters: Charge,
}
pub fn open<'a>(
    cx: Context<'a>,
    input: Input<'a>,
    stopped: &dyn Fn() -> bool,
) -> Result<Stream<'a>, Error> {
    build(cx, input, Layout::default(), 0, stopped)
}
/// `zip_offset` is the first local header's logical position. Prefix bytes
/// are neither allocated nor emitted/hashed. `Input.size` includes the prefix.
#[cfg(any(test, feature = "test-hooks"))]
pub fn open_at<'a>(
    cx: Context<'a>,
    input: Input<'a>,
    zip_offset: u64,
    stopped: &dyn Fn() -> bool,
) -> Result<Stream<'a>, Error> {
    build(
        cx,
        input,
        Layout::seed(zip_offset, 0, 0)?,
        zip_offset,
        stopped,
    )
}
fn build<'a>(
    cx: Context<'a>,
    input: Input<'a>,
    layout: Layout,
    origin: u64,
    stopped: &dyn Fn() -> bool,
) -> Result<Stream<'a>, Error> {
    let pull = Pull::build(&cx, input, layout, origin, stopped)?;
    Ok(Stream { cx, input, pull })
}
impl Pull {
    pub(crate) fn build(
        cx: &Context<'_>,
        input: Input<'_>,
        layout: Layout,
        origin: u64,
        stopped: &dyn Fn() -> bool,
    ) -> Result<Self, Error> {
        check(stopped)?;
        if eligibility(input.members, false, input.size) != Eligibility::Eligible
            || input.key.is_empty()
            || input.key.len() > 1024
            || input.index >= 256
        {
            return Err(Error::internal());
        }
        let adapters = cx.meter.charge(cx.workspace.adapter_charge()?)?;
        let members = member::Reader::open(cx.workspace, input.list, cx.meter, stopped)?;
        let central = Central::new(cx.workspace, cx.meter, stopped)?;
        let header = Buffer::new(cx.meter, 4096)?;
        Ok(Self {
            members: Some(members),
            central,
            header,
            position: 0,
            payload: None,
            layout,
            end: None,
            seen: 0,
            origin,
            digest: Incremental::default(),
            state: State::Members,
            failed: false,
            _adapters: adapters,
        })
    }
}
impl Pull {
    /// Return at most 64 KiB into caller-owned memory. An empty output buffer
    /// returns zero without advancing; only `finish` certifies exhaustion.
    /// Any read failure poisons the stream and retains central claims.
    pub(crate) fn read(
        &mut self,
        cx: &Context<'_>,
        input: Input<'_>,
        out: &mut [u8],
        stopped: &dyn Fn() -> bool,
    ) -> Result<usize, Error> {
        if self.failed {
            return Err(Error::internal());
        }
        self.failed = true;
        check(stopped)?;
        let limit = out.len().min(CHUNK);
        let n = if limit == 0 {
            0
        } else {
            self.pull(
                cx,
                input,
                out.get_mut(..limit).ok_or_else(Error::internal)?,
                stopped,
            )?
        };
        self.digest
            .update(out.get(..n).ok_or_else(Error::internal)?)?;
        self.failed = false;
        Ok(n)
    }
    fn pull(
        &mut self,
        cx: &Context<'_>,
        input: Input<'_>,
        out: &mut [u8],
        stopped: &dyn Fn() -> bool,
    ) -> Result<usize, Error> {
        loop {
            check(stopped)?;
            if self.position < self.header.len() {
                let header = self
                    .header
                    .get(self.position..)
                    .ok_or_else(Error::internal)?;
                let n = out.len().min(header.len());
                out.get_mut(..n)
                    .ok_or_else(Error::internal)?
                    .copy_from_slice(header.get(..n).ok_or_else(Error::internal)?);
                self.position += n;
                return Ok(n);
            }
            if let Some(payload) = &mut self.payload {
                let n = payload.read(out, cx, stopped)?;
                if n > 0 {
                    return Ok(n);
                }
                drop(self.payload.take());
            }
            match self.state {
                State::Members => self.member(cx, input, stopped)?,
                State::Central => {
                    let n = self.central.read(out, cx.hooks, stopped)?;
                    if n > 0 {
                        return Ok(n);
                    }
                    headers::end(&mut self.header, self.end.ok_or_else(Error::internal)?)?;
                    self.position = 0;
                    self.state = State::End;
                }
                State::End => {
                    self.state = State::Done;
                }
                State::Done => return Ok(0),
            }
        }
    }
    fn member(
        &mut self,
        cx: &Context<'_>,
        input: Input<'_>,
        stopped: &dyn Fn() -> bool,
    ) -> Result<(), Error> {
        boundary(cx.hooks, Point::Member, stopped)?;
        let next = self
            .members
            .as_mut()
            .ok_or_else(Error::internal)?
            .next(stopped)?;
        if let Some(m) = next {
            if m.archive != input.key
                || m.index != input.index
                || m.reason.is_some()
                || self.seen >= input.members
            {
                return Err(Error::internal());
            }
            let entry = self.layout.member(
                u64::try_from(m.path.len()).map_err(|_| Error::internal())?,
                m.size,
            )?;
            if self.layout.finish()?.size > input.size {
                return Err(Error::internal());
            }
            headers::central(&mut self.header, m.path, m.size, m.crc32, entry)?;
            self.central.append(&self.header, cx.hooks, stopped)?;
            headers::local(&mut self.header, m.path, m.size, m.crc32, entry)?;
            self.position = 0;
            check(stopped)?;
            self.payload = Some(Payload {
                read: cx.payloads.open(m.source, m.plan_index)?,
                remaining: m.size,
                crc: crc32fast::Hasher::new(),
                md5: Md5::new(),
                expected_crc: m.crc32,
                expected_md5: m.md5,
            });
            self.seen += 1;
        } else {
            let end = self.layout.finish()?;
            if self.seen != input.members
                || end.count != self.seen
                || end.size != input.size
                || self.digest.size().checked_add(self.origin) != Some(end.central_offset)
            {
                return Err(Error::internal());
            }
            drop(self.members.take());
            self.header.clear();
            self.position = 0;
            self.central.replay(end.central_size, cx.hooks, stopped)?;
            self.end = Some(end);
            self.state = State::Central;
        }
        Ok(())
    }
    /// No summary until complete exhaustion, final size verification and
    /// successful explicit retirement. Failure/stop/drop performs no cleanup.
    pub(crate) fn finish(
        self,
        cx: &Context<'_>,
        input: Input<'_>,
        stopped: &dyn Fn() -> bool,
    ) -> Result<Summary, Error> {
        check(stopped)?;
        if self.failed
            || !matches!(self.state, State::Done)
            || self.digest.size().checked_add(self.origin) != Some(input.size)
        {
            return Err(Error::internal());
        }
        self.central.retire(cx.hooks, stopped)?;
        boundary(cx.hooks, Point::Complete, stopped)?;
        let hashes = self.digest.finish();
        Ok(Summary {
            size: input.size,
            emitted_size: hashes.size,
            md5: hashes.md5,
            etag: hashes.etag,
        })
    }
}
impl Stream<'_> {
    pub fn read(&mut self, out: &mut [u8], stopped: &dyn Fn() -> bool) -> Result<usize, Error> {
        self.pull.read(&self.cx, self.input, out, stopped)
    }
    pub fn finish(self, stopped: &dyn Fn() -> bool) -> Result<Summary, Error> {
        self.pull.finish(&self.cx, self.input, stopped)
    }
}
