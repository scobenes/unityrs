//! Eight row streams over frozen received facts and sealed artifacts.
use super::{
    Family,
    chunks::Chunks,
    counts::{Counts, increment},
};
use crate::{
    lifetime::Stop,
    phase::{Account, Error, check},
    phase1::Facts,
    spool::workspace::Workspace,
    winners::{self, Handoff, Inputs, json::Json},
};

/// What the finisher can read at the phase the controller reached
/// (`BATCH_SPEC.md` 7.4): the frozen facts always; phase 6's sealed
/// artifacts and the outcomes only from phase 7 on.
pub struct Context<'a> {
    pub facts: &'a Facts,
    pub inputs: &'a dyn Inputs,
    /// None before phase 7: the four selected families are then null.
    pub selected: Option<Selected<'a>>,
    /// The taken stop, for its culprit's overlay; None at a normal end.
    pub stop: Option<Stop>,
    /// The hand-over slot's input that the finisher's one stat found
    /// committed: its records are read whatever its phase says (2.4).
    pub found: Option<u32>,
    /// Dedicated fixed finisher storage (`bound::C_FIN`), separate from the
    /// limit-`I` account: a stopped transfer may retain phase reservations.
    pub account: &'a Account,
    pub stopped: &'a dyn Fn() -> bool,
    /// The hand-over instant: a dispatched `stopped` row's `ms` ends here.
    pub taken_at: std::time::Instant,
}
/// Phase 6's hand-off and the outcomes as the gate's close left them.
#[derive(Clone, Copy)]
pub struct Selected<'a> {
    pub handoff: &'a Handoff,
    pub workspace: &'a Workspace,
    pub outcomes: &'a dyn crate::outcomes::Read,
}
impl Context<'_> {
    /// Committed by the controller, or by the finisher's one stat.
    #[must_use]
    pub fn committed(&self, input: u32) -> bool {
        self.found == Some(input)
            || usize::try_from(input)
                .ok()
                .and_then(|i| self.facts.inputs.get(i))
                .is_some_and(|s| s.phase == crate::phase1::InputPhase::Committed)
    }
    /// The culprit's reason, for its `detail`, when the stop names `culprit`.
    #[must_use]
    pub fn culprit(&self, culprit: crate::control::Culprit) -> Option<&'static str> {
        self.stop
            .filter(|s| s.culprit == culprit && culprit != crate::control::Culprit::None)
            .map(|s| s.reason.name())
    }
}
pub(super) fn generate(
    family: Family,
    cx: &Context<'_>,
    out: &mut Chunks<'_>,
    counts: &mut Counts,
) -> Result<(), super::WriteError> {
    let mut json = Json::new(cx.account)?;
    match family {
        Family::Bundles => super::received::bundles(cx, out, counts, &mut json),
        Family::Inputs | Family::Skips | Family::Diagnostics => {
            super::received::inputs(family, cx, out, counts, &mut json)
        }
        _ => selected(
            family,
            cx,
            cx.selected.ok_or_else(Error::internal)?,
            out,
            counts,
            &mut json,
        ),
    }
}
#[allow(clippy::too_many_lines)] // Four closed schemas over one reader, statuses and counts.
fn selected(
    family: Family,
    cx: &Context<'_>,
    sel: Selected<'_>,
    out: &mut Chunks<'_>,
    counts: &mut Counts,
    json: &mut Json,
) -> Result<(), super::WriteError> {
    use winners::rowread::{File, Reader, Row};
    let (artifact, file) = match family {
        Family::Outputs => (sel.handoff.outputs.as_ref(), File::Outputs),
        Family::Collisions => (sel.handoff.collisions.as_ref(), File::Collisions),
        Family::Archives => (sel.handoff.archives.as_ref(), File::Archives),
        Family::ArchiveMembers => (sel.handoff.archive_members.as_ref(), File::Members),
        _ => return Err(Error::internal().into()),
    };
    let mut reader = Reader::open(
        sel.workspace,
        artifact,
        file,
        cx.account.meter(),
        cx.stopped,
    )?;
    let mut winners = winners::stream::Cursor::read(
        sel.workspace,
        if family == Family::Outputs {
            sel.handoff.winners.as_ref()
        } else {
            None
        },
        &winners::stream::By::Key,
        cx.account.meter(),
        cx.stopped,
        true,
    )?;
    let mut metadata = if family == Family::Archives {
        sel.handoff.metadata.as_ref()
    } else {
        None
    }
    .map(|m| winners::archives::Reader::open(sel.workspace, m, cx.account.meter(), cx.stopped))
    .transpose()?;
    let mut index = 0_u64;
    let mut keys = 0_u64;
    let mut previous = crate::phase::Buffer::new(cx.account.meter(), 1024)?;
    while let Some(row) = reader.next(cx.stopped)? {
        check(cx.stopped)?;
        match row {
            Row::Output(r) => {
                increment(&mut keys)?;
                let winner = winners.row()?.filter(|w| w.key == r.key);
                // Base rows are publishable exactly when the sealed winners
                // stream contains that key, before the global overlay.
                if publishable(&r, sel.handoff.publication_blocked) != winner.is_some() {
                    return Err(Error::internal().into());
                }
                let status = if let Some(w) = winner {
                    if !same_output(&r, w) || w.state != winners::State::Publish {
                        return Err(Error::internal().into());
                    }
                    let code = sel.outcomes.loose(index)?;
                    index = index.checked_add(1).ok_or_else(Error::internal)?;
                    let result = if sel.handoff.publication_blocked {
                        ("withheld", Some("publish_incomplete"))
                    } else {
                        (code.status(), code.reason())
                    };
                    winners.advance(cx.stopped)?;
                    result
                } else {
                    (r.status, r.reason)
                };
                output(json, &r, status)?;
                counts.status(family, status.0)?;
            }
            Row::Collision(r) => {
                if previous.as_ref() != r.key.as_bytes() {
                    increment(&mut counts.collisions)?;
                    if r.flip {
                        increment(&mut counts.flips)?;
                    }
                    previous.clear();
                    previous.extend(r.key.as_bytes())?;
                }
                collision(json, &r)?;
            }
            Row::Archive(r) => {
                let m = metadata
                    .as_mut()
                    .ok_or_else(Error::internal)?
                    .next(cx.stopped)?
                    .ok_or_else(Error::internal)?;
                if (m.key, m.members, m.left_out) != (r.key, r.members, r.left_out) {
                    return Err(Error::internal().into());
                }
                let value = sel.outcomes.archive(u64::from(m.index))?;
                archive(json, &r, &value, counts)?;
            }
            Row::Member(r) => member(json, &r)?,
        }
        out.put(json.end()?)?;
    }
    if winners.row()?.is_some()
        || metadata
            .as_mut()
            .map(|m| m.next(cx.stopped))
            .transpose()?
            .flatten()
            .is_some()
        || (family == Family::Outputs
            && (index != sel.handoff.counts.winners || keys != sel.handoff.counts.keys))
    {
        return Err(Error::internal().into());
    }
    Ok(())
}
fn publishable(r: &winners::rowread::Output<'_>, blocked: bool) -> bool {
    r.status == "not_written"
        || (blocked && r.status == "withheld" && r.reason == Some("publish_incomplete"))
}
fn same_output(r: &winners::rowread::Output<'_>, w: winners::Row<'_>) -> bool {
    let kind = ["texture", "sprite", "atlas_json", "atlas_sprite"]
        .get(usize::from(w.kind))
        .copied();
    let mode = [None, Some("L"), Some("LA"), Some("RGB"), Some("RGBA")]
        .get(usize::from(w.mode))
        .copied();
    r.key == w.key
        && r.md5 == w.md5
        && r.size == w.size
        && r.input == w.input
        && r.serialized_file == w.serialized_file
        && r.path_id == w.path_id
        && r.unit == (!w.unit.is_empty()).then_some(w.unit)
        && r.visible_hash == w.visible_hash
        && r.cut == w.cut
        && Some(r.kind) == kind
        && Some(r.mode) == mode
}
fn output(
    json: &mut Json,
    r: &winners::rowread::Output<'_>,
    (status, reason): (&str, Option<&str>),
) -> Result<(), Error> {
    json.start("key", r.key)?;
    json.text("kind", Some(r.kind))?;
    json.text("unit", r.unit)?;
    json.text("input", Some(r.input))?;
    json.text("serialized_file", Some(r.serialized_file))?;
    json.value("path_id", r.path_id)?;
    json.hex("md5", Some(&r.md5))?;
    json.hex(
        "visible_hash",
        r.visible_hash.as_ref().map(<[u8; 32]>::as_slice),
    )?;
    json.text("mode", r.mode)?;
    json.value("size", r.size)?;
    json.text("status", Some(status))?;
    json.text("reason", reason)?;
    json.value("cut", r.cut)
}
fn collision(json: &mut Json, r: &winners::rowread::Collision<'_>) -> Result<(), Error> {
    json.start("key", r.key)?;
    json.text("input", Some(r.input))?;
    json.text("serialized_file", Some(r.serialized_file))?;
    json.hex("md5", Some(&r.md5))?;
    json.hex(
        "visible_hash",
        r.visible_hash.as_ref().map(<[u8; 32]>::as_slice),
    )?;
    json.value("winner", r.winner)?;
    json.text("rule", Some(r.rule))?;
    json.value("flip", r.flip)?;
    json.value("pixels_differ", r.pixels_differ)?;
    json.value("cut", r.cut)
}
fn archive(
    json: &mut Json,
    r: &winners::rowread::Archive<'_>,
    value: &crate::outcomes::Archive,
    counts: &mut Counts,
) -> Result<(), Error> {
    let (status, reason) = if r.status == "not_written" {
        (value.code.status(), value.code.reason())
    } else {
        (r.status, r.reason)
    };
    let success = matches!(status, "written" | "present");
    json.start("key", r.key)?;
    json.text("status", Some(status))?;
    json.text("reason", reason)?;
    json.value("members", r.members)?;
    json.value("left_out", r.left_out)?;
    json.hex("md5", success.then_some(value.md5.as_slice()))?;
    json.text("etag", if success { value.etag()? } else { None })?;
    json.optional("size", success.then_some(value.size))?;
    counts.status(Family::Archives, status)
}
fn member(json: &mut Json, r: &winners::rowread::Member<'_>) -> Result<(), Error> {
    json.start("archive", r.archive)?;
    json.text("path", Some(r.path))?;
    json.text("key", Some(r.key))?;
    json.text("input", Some(r.input))?;
    json.hex("md5", Some(&r.md5))?;
    json.value("included", r.included)?;
    json.text("reason", r.reason)?;
    json.value("cut", r.cut)
}
