//! One transfer future. Every payload/ZIP pull runs on the existing pool,
//! under the slot's reservation, before the first timed request using it.
use super::{Listed, Target, source};
use crate::{
    archive,
    outcomes::Code,
    phase::{self, Buffer, Error},
    publish::{Owned, Point, archive::Descriptor, payload::Payloads},
    sink::{Sink, store::Store},
    store::{
        etag::Etag,
        multipart::{Target as Remote, Transfer},
        put::{self, Payload, Request},
    },
    sys::Shared,
    winners,
};
use std::io::Read;
use unityrs_core::limits::Charge;

pub(super) enum Item {
    Loose {
        frame: crate::sort::Frame,
        index: u64,
    },
    Archive(Descriptor),
}
impl Item {
    pub(super) fn size(&self) -> Result<u64, Error> {
        match self {
            Self::Loose { frame, .. } => Ok(winners::codec::decode(frame.bytes())?.size),
            Self::Archive(d) => Ok(d.size()),
        }
    }
    fn identity(&self) -> Result<(&str, Target, Etag, &'static str), Error> {
        Ok(match self {
            Self::Loose { frame, index } => {
                let row = winners::codec::decode(frame.bytes())?;
                (
                    row.key,
                    Target::Loose(*index),
                    source::expected(row)?,
                    if row.kind == 2 {
                        "application/json"
                    } else {
                        "image/png"
                    },
                )
            }
            Self::Archive(d) => (
                d.key()?,
                Target::Archive(u64::from(d.index())),
                Etag::new(b"").ok_or_else(Error::internal)?,
                "application/zip",
            ),
        })
    }
}
#[allow(clippy::large_enum_variant)] // One inline pull state per admitted slot; its envelope is charged before boxing the future.
enum Body {
    Loose(Box<dyn Read + Send>),
    Archive {
        descriptor: Descriptor,
        pull: Option<archive::Pull>,
    },
}
struct Input {
    owned: Shared<Owned>,
    slot: usize,
    remaining: u64,
    body: Body,
}
impl Input {
    fn open(owned: Shared<Owned>, item: Item, slot: usize) -> Result<Self, Error> {
        let remaining = item.size()?;
        let store = get(&owned)?;
        let body = match item {
            Item::Loose { frame, .. } => {
                let row = winners::codec::decode(frame.bytes())?;
                Body::Loose(owned.open(row.source, row.plan_index)?)
            }
            Item::Archive(descriptor) => {
                let cx = archive_context(&owned, store.account(slot)?);
                let pull = archive::Pull::build(
                    &cx,
                    descriptor.input()?,
                    crate::winners::layout::Layout::default(),
                    0,
                    &|| owned.control.stopped(),
                )?;
                Body::Archive {
                    descriptor,
                    pull: Some(pull),
                }
            }
        };
        Ok(Self {
            owned,
            slot,
            remaining,
            body,
        })
    }
    fn chunk(mut self) -> Result<(Self, Payload, Option<archive::Summary>), Error> {
        let store = get(&self.owned)?;
        let account = store.account(self.slot)?;
        let stopped = || self.owned.control.stopped();
        phase::check(&stopped)?;
        let size = usize::try_from(self.remaining.min(put::PART_SIZE as u64))
            .map_err(|_| Error::internal())?;
        let mut buffer = Buffer::new(account.meter(), size)?;
        buffer.fill(0);
        match &mut self.body {
            Body::Loose(reader) => reader.read_exact(&mut buffer)?,
            Body::Archive { descriptor, pull } => {
                let cx = archive_context(&self.owned, account);
                let mut used = 0;
                while used < size {
                    self.owned.hooks.point(Point::Buffer);
                    let n = pull.as_mut().ok_or_else(Error::internal)?.read(
                        &cx,
                        descriptor.input()?,
                        buffer.get_mut(used..).ok_or_else(Error::internal)?,
                        &stopped,
                    )?;
                    if n == 0 {
                        return Err(Error::internal());
                    }
                    used += n;
                }
            }
        }
        self.remaining -= size as u64;
        let mut summary = None;
        if self.remaining == 0 {
            match &mut self.body {
                Body::Loose(reader) => {
                    let mut tail = [0];
                    if reader.read(&mut tail)? != 0 {
                        return Err(Error::internal());
                    }
                }
                Body::Archive { descriptor, pull } => {
                    let cx = archive_context(&self.owned, account);
                    let mut tail = [0];
                    if pull.as_mut().ok_or_else(Error::internal)?.read(
                        &cx,
                        descriptor.input()?,
                        &mut tail,
                        &stopped,
                    )? != 0
                    {
                        return Err(Error::internal());
                    }
                    summary = Some(pull.take().ok_or_else(Error::internal)?.finish(
                        &cx,
                        descriptor.input()?,
                        &stopped,
                    )?);
                    // The stream has retired its central scratch before the final
                    // request; retain no stream or borrowed capability afterward.
                }
            }
        }
        Ok((self, Payload::from_buffer(buffer), summary))
    }
}
fn archive_context<'a>(
    owned: &'a Owned,
    account: &'a crate::phase::Account,
) -> archive::Context<'a> {
    archive::Context {
        workspace: &owned.workspace,
        payloads: owned,
        meter: account.meter(),
        hooks: &archive::NoHooks,
    }
}
fn get(owned: &Owned) -> Result<&Store, Error> {
    match &owned.sink {
        Sink::Store(s) => Ok(s),
        Sink::Directory(_) => Err(Error::internal()),
    }
}
pub(super) fn failure(error: put::Error) -> Result<Code, Error> {
    Ok(match error {
        put::Error::Fault(f) => return Err(crate::sink::store::errors::fault(f)),
        put::Error::Phase(e) => return Err(e),
        put::Error::Stopped => return Err(Error::Stopped),
        put::Error::CredentialsExpired => Code::CredentialsExpired,
        put::Error::ArchiveTooLarge => Code::ArchiveTooLarge,
        put::Error::PublishFailed => Code::PublishFailed,
    })
}
#[allow(clippy::too_many_lines)] // The transfer's request sequence and same-poll acknowledgment stay together.
pub(super) async fn run(
    owned: Shared<Owned>,
    slot: usize,
    item: Item,
    listed: Option<Listed>,
    pool: tokio::runtime::Handle,
    _reservation: Charge,
) -> Result<(), Error> {
    let store = get(&owned)?;
    let account = store.account(slot)?;
    let (raw, target, expected, content_type) = item.identity()?;
    let mut key = String::new();
    key.try_reserve_exact(store.settings().prefix.len() + raw.len())
        .map_err(|_| Error::memory())?;
    key.push_str(&store.settings().prefix);
    key.push_str(raw);
    let key = object_store::path::Path::parse(key).map_err(|_| Error::internal())?;
    let size = item.size()?;
    let stopped = || owned.control.stopped();
    owned.hooks.point(Point::Transfer);
    phase::check(&stopped)?;
    let state = owned.clone();
    let mut input = crate::execution::blocking(&pool, move || {
        phase::on_pool_named("store payload open", || Input::open(state, item, slot))
    })
    .await
    .map_err(|_| Error::internal())??;
    if let Some(listed) = listed {
        let hashed = hash(input, &pool).await?;
        let summary = hashed.1;
        owned.hooks.point(Point::ResumeReleased);
        if listed.matches(summary_tag(summary)?.as_bytes(), summary.size) {
            phase::check(&stopped)?;
            source::mark(
                &owned.outcomes,
                target,
                Code::Present,
                Some(summary),
                Some(listed.etag),
            )?;
            progress(&owned, target, Code::Present);
            return Ok(());
        }
        input = reopen(hashed.0, &pool).await?;
    }
    let tries = store.tries(&*owned.control);
    let client = match content_type {
        "application/zip" => &store.clients[2],
        "application/json" => &store.clients[1],
        _ => &store.clients[0],
    };
    let request = Request::new(client, &tries, store.slot(slot)?, store.deadline());
    let journal = store.journal.lock().clone().ok_or_else(Error::internal)?;
    let mut summary = None;
    let result = if size <= put::PART_SIZE as u64 {
        let (next, payload, done) = chunk(input, &pool).await?;
        drop(next);
        summary = done;
        let expected = summary.map_or(Ok(expected), summary_tag)?;
        request
            .put(&key, payload, content_type, expected.as_bytes())
            .await
    } else {
        let mut transfer = match Transfer::start(
            &request,
            account,
            store.parts(slot)?,
            &journal,
            Remote {
                key: &key,
                endpoint: store.settings().endpoint.as_deref(),
                bucket: &store.settings().bucket,
            },
            size,
        )
        .await
        {
            Ok(t) => t,
            Err(error) => {
                let code = failure(error)?;
                source::mark(&owned.outcomes, target, code, None, None)?;
                progress(&owned, target, code);
                return Ok(());
            }
        };
        let mut failed = None;
        while input.remaining > 0 {
            let (next, payload, done) = chunk(input, &pool).await?;
            input = next;
            summary = done.or(summary);
            if let Err(error) = transfer.part(&request, payload).await {
                failed = Some(error);
                break;
            }
        }
        drop(input);
        if let Some(error) = failed {
            Err(error)
        } else {
            let expected = summary.map_or(Ok(expected), summary_tag)?;
            transfer.complete(&request, expected.as_bytes()).await
        }
    };
    // No await or stop check between the final answer and its outcome.
    let (code, tag) = match result {
        Ok(w) => {
            let code = if w.verify_by_size {
                store.figures.lock().etag_not_md5 += 1;
                Code::WrittenBySize
            } else {
                Code::Written
            };
            if let Target::Loose(i) = target
                && w.verify_by_size
            {
                source::put_tag(store, i, w.etag)?;
            }
            (code, Some(w.etag))
        }
        Err(error) => (failure(error)?, None),
    };
    source::mark(&owned.outcomes, target, code, summary, tag)?;
    progress(&owned, target, code);
    Ok(())
}
async fn chunk(
    input: Input,
    pool: &tokio::runtime::Handle,
) -> Result<(Input, Payload, Option<archive::Summary>), Error> {
    crate::execution::blocking(pool, move || {
        phase::on_pool_named("store payload part", || input.chunk())
    })
    .await
    .map_err(|_| Error::internal())?
}
// Archive hashing uses the same bounded puller and retires scratch before a
// miss reopens it. The descriptor is retained while the completed input drops.
async fn hash(
    mut input: Input,
    pool: &tokio::runtime::Handle,
) -> Result<(Input, archive::Summary), Error> {
    let summary = loop {
        let (next, payload, summary) = chunk(input, pool).await?;
        drop(payload);
        input = next;
        if let Some(summary) = summary {
            break summary;
        }
        if input.remaining == 0 {
            return Err(Error::internal());
        }
    };
    Ok((input, summary))
}
async fn reopen(mut input: Input, pool: &tokio::runtime::Handle) -> Result<Input, Error> {
    let input = crate::execution::blocking(pool, move || {
        phase::on_pool_named("store archive reopen", || {
            let store = get(&input.owned)?;
            let Body::Archive { descriptor, pull } = &mut input.body else {
                return Err(Error::internal());
            };
            let cx = archive_context(&input.owned, store.account(input.slot)?);
            *pull = Some(archive::Pull::build(
                &cx,
                descriptor.input()?,
                crate::winners::layout::Layout::default(),
                0,
                &|| input.owned.control.stopped(),
            )?);
            input.remaining = descriptor.size();
            Ok(input)
        })
    })
    .await
    .map_err(|_| Error::internal())??;
    Ok(input)
}
fn summary_tag(summary: archive::Summary) -> Result<Etag, Error> {
    use std::io::Write;
    let mut bytes = [0; 100];
    let mut out = &mut bytes[..];
    write!(out, "{}", summary.etag).map_err(|_| Error::internal())?;
    let used = 100 - out.len();
    Etag::new(bytes.get(..used).ok_or_else(Error::internal)?).ok_or_else(Error::internal)
}
fn progress(owned: &Owned, target: Target, code: Code) {
    if code.published()
        && let Ok(store) = get(owned)
    {
        store.figures.lock().verified = Some(false);
    }
    if let Some(p) = &owned.progress {
        p.update(|s| {
            if code.published() {
                if code == Code::Present {
                    if matches!(target, Target::Loose(_)) {
                        s.outputs.present += 1;
                    }
                } else {
                    s.transfers.uploads_done += 1;
                    if matches!(target, Target::Loose(_)) {
                        s.outputs.published += 1;
                    }
                }
            } else {
                s.transfers.uploads_failed += 1;
                if matches!(target, Target::Loose(_)) {
                    s.outputs.failed += 1;
                }
            }
        });
    }
}
