//! Archive-key ordered metadata capabilities, included lists and public rows.
use super::{
    Context, Pass,
    codec::Decode,
    json::{Json, Rows},
    layout::Layout,
    member::{By, Member},
    members::{Frames, Output},
    stream::retire,
};
use crate::{
    phase::{Buffer, Error},
    sort::{self, Control, Order},
    spool::workspace::{Sealed, Workspace},
};
use std::{cmp::Ordering, sync::Arc};
use unityrs_core::limits::Meter;

#[derive(Debug)]
pub struct Archive<'a> {
    pub key: &'a str,
    pub index: u32,
    pub members: u64,
    pub left_out: u64,
    pub size: u64,
    pub list: Option<Sealed>,
}
struct ByArchive;
impl Order for ByArchive {
    fn tag(&self) -> u32 {
        115
    }
    fn validate(&self, bytes: &[u8]) -> Result<(), Error> {
        decode(bytes).map(|_| ())
    }
    fn compare(&self, a: &[u8], b: &[u8]) -> Ordering {
        match (decode(a), decode(b)) {
            (Ok(a), Ok(b)) => a.key.cmp(b.key),
            _ => a.cmp(b),
        }
    }
}
fn decode(bytes: &[u8]) -> Result<Archive<'_>, Error> {
    let mut d = Decode(bytes);
    let index = u32::from_le_bytes(d.fixed()?);
    let members = u64::from_le_bytes(d.fixed()?);
    let left_out = u64::from_le_bytes(d.fixed()?);
    let size = u64::from_le_bytes(d.fixed()?);
    let list = if d.flag()? {
        Some(Sealed::decode(d.take(Sealed::BYTES)?)?)
    } else {
        None
    };
    let key = d.text(1024)?;
    if key.is_empty() || !d.0.is_empty() || list.is_some() != (members > 0) {
        return Err(Error::internal());
    }
    Ok(Archive {
        key,
        index,
        members,
        left_out,
        size,
        list,
    })
}
pub struct Reader<'a>(sort::Reader<'a>);
impl Reader<'_> {
    pub fn open(
        workspace: &Workspace,
        file: &Sealed,
        meter: &Arc<Meter>,
        stopped: &dyn Fn() -> bool,
    ) -> Result<Self, Error> {
        Ok(Self(sort::Reader::open(
            workspace, file, &ByArchive, meter, stopped,
        )?))
    }
    pub fn next(&mut self, stopped: &dyn Fn() -> bool) -> Result<Option<Archive<'_>>, Error> {
        self.0.next(stopped)?.map(decode).transpose()
    }
}
pub(super) struct Artifacts {
    pub rows: Option<Sealed>,
    pub members: Option<Sealed>,
    pub metadata: Option<Sealed>,
}
#[allow(clippy::too_many_lines)] // One streaming archive lifecycle owns its list, counts, rows and capability.
pub(super) fn materialize(
    cx: &mut Context<'_>,
    selected: Option<Sealed>,
    blocked: bool,
) -> Result<Artifacts, Error> {
    cx.boundary(Pass::Handoff)?;
    let mut source = Frames::open(selected.as_ref(), &By::Display, cx, false)?;
    let mut rows = Rows::new(cx.account)?;
    let mut member_rows = Rows::new(cx.account)?;
    let mut json = Json::new(cx.account)?;
    let mut metadata = None;
    let mut bytes = Buffer::new(cx.account.meter(), 2048)?;
    for &index in &cx.prepared.archive_rows {
        crate::phase::check(cx.stopped)?;
        let archive = cx
            .spec
            .archives
            .get(usize::try_from(index).map_err(|_| Error::internal())?)
            .ok_or_else(Error::internal)?;
        let mut list = Output::new(By::List, cx)?;
        let mut layout = Layout::default();
        let mut left_out = 0_u64;
        while let Some(m) = source.member()?.filter(|m| m.archive == archive.key) {
            if m.index != index {
                return Err(Error::internal());
            }
            if m.reason.is_none() {
                layout.member(
                    u64::try_from(m.path.len()).map_err(|_| Error::internal())?,
                    m.size,
                )?;
                list.put(m, cx)?;
            } else {
                left_out = left_out.checked_add(1).ok_or_else(Error::internal)?;
            }
            member_row(&mut json, m)?;
            member_rows.put(json.end()?, cx)?;
            source.advance(cx.stopped)?;
        }
        let end = layout.finish()?;
        let list = list.finish(cx)?;
        json.start("key", &archive.key)?;
        json.text(
            "status",
            Some(if end.count == 0 {
                "empty"
            } else if blocked {
                "withheld"
            } else {
                "not_written"
            }),
        )?;
        json.text(
            "reason",
            (blocked && end.count > 0).then_some("publish_incomplete"),
        )?;
        json.value("members", end.count)?;
        json.value("left_out", left_out)?;
        for field in ["md5", "etag", "size"] {
            json.text(field, None)?;
        }
        rows.put(json.end()?, cx)?;
        bytes.clear();
        bytes.extend(&index.to_le_bytes())?;
        for n in [end.count, left_out, end.size] {
            bytes.extend(&n.to_le_bytes())?;
        }
        bytes.push(u8::from(list.is_some()))?;
        if let Some(list) = list {
            bytes.extend(&list.encode())?;
        }
        bytes.extend(
            &u16::try_from(archive.key.len())
                .map_err(|_| Error::internal())?
                .to_le_bytes(),
        )?;
        bytes.extend(archive.key.as_bytes())?;
        let control = Control {
            stopped: cx.stopped,
            hooks: cx.hooks,
        };
        if metadata.is_none() {
            metadata = Some(sort::format::Writer::new(
                cx.workspace,
                ByArchive.tag(),
                cx.account.meter(),
                &control,
            )?);
        }
        metadata
            .as_mut()
            .ok_or_else(Error::internal)?
            .frame(&bytes, &control)?;
    }
    if source.member()?.is_some() {
        return Err(Error::internal());
    }
    drop(source);
    retire(cx.workspace, selected, cx.stopped)?;
    Ok(Artifacts {
        rows: rows.finish(cx)?,
        members: member_rows.finish(cx)?,
        metadata: metadata
            .map(|w| {
                w.finish(&Control {
                    stopped: cx.stopped,
                    hooks: cx.hooks,
                })
            })
            .transpose()?,
    })
}
pub(super) fn member_row(json: &mut Json, m: Member<'_>) -> Result<(), Error> {
    json.start("archive", m.archive)?;
    json.text("path", Some(m.path))?;
    json.text("key", Some(m.key))?;
    json.text("input", Some(m.input))?;
    json.hex("md5", Some(&m.md5))?;
    json.value("included", m.reason.is_none())?;
    // The small reason vocabulary is rendered into the same fixed row buffer.
    match m.reason {
        None => json.text("reason", None)?,
        Some(crate::reason::LeftOut::DuplicateMember) => {
            json.text("reason", Some("duplicate_member"))?;
        }
        Some(crate::reason::LeftOut::PrefixConflict) => {
            json.text("reason", Some("prefix_conflict"))?;
        }
        Some(crate::reason::LeftOut::UnwritablePath(rule)) => json.text(
            "reason",
            Some(match rule {
                crate::reason::KeyRule::Empty => "unwritable_path(empty)",
                crate::reason::KeyRule::NotUtf8 => "unwritable_path(not_utf8)",
                crate::reason::KeyRule::ControlChar => "unwritable_path(control_char)",
                crate::reason::KeyRule::LeadingSlash => "unwritable_path(leading_slash)",
                crate::reason::KeyRule::TrailingSlash => "unwritable_path(trailing_slash)",
                crate::reason::KeyRule::EmptySegment => "unwritable_path(empty_segment)",
                crate::reason::KeyRule::DotSegment => "unwritable_path(dot_segment)",
                crate::reason::KeyRule::SegmentTooLong => "unwritable_path(segment_too_long)",
                crate::reason::KeyRule::TooLong => "unwritable_path(too_long)",
                crate::reason::KeyRule::ReservedPrefix => "unwritable_path(reserved_prefix)",
                crate::reason::KeyRule::ReservedName => "unwritable_path(reserved_name)",
                _ => return Err(Error::internal()),
            }),
        )?,
    }
    json.value("cut", m.cut)
}
