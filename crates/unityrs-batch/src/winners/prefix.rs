//! Single original-stream lookahead; refusals never alter that relation.
use super::{
    Artifacts, Context, Pass,
    codec::{Codec, Row, State, decode},
    stream::{By, Cursor, Output, retire},
    units,
};
use crate::{
    phase::{Buffer, Error},
    sort::{Frame, Source},
};

struct PrefixSource<'a> {
    keys: Cursor<'a>,
    codec: Codec,
}
impl Source for PrefixSource<'_> {
    fn next(&mut self, out: &mut Frame, stopped: &dyn Fn() -> bool) -> Result<bool, Error> {
        while let Some(row) = self.keys.row()? {
            if row.state == State::Publish {
                let mut marker = Row::marker(row.key);
                marker.unit = row.unit;
                out.set(self.codec.encode(marker)?)?;
                self.keys.advance(stopped)?;
                return Ok(true);
            }
            self.keys.advance(stopped)?;
        }
        Ok(false)
    }
}
/// Declared archives merge directly from measured spec indices; they never
/// become temporary spool projections, including in empty-candidate jobs.
struct MergeArchives<'a, 'b> {
    keys: Cursor<'a>,
    spec: &'b crate::spec::model::Spec,
    prepared: &'b super::Prepared,
    index: usize,
    codec: Codec,
}
impl Source for MergeArchives<'_, '_> {
    fn next(&mut self, out: &mut Frame, stopped: &dyn Fn() -> bool) -> Result<bool, Error> {
        crate::phase::check(stopped)?;
        let archive = self
            .prepared
            .archives
            .get(self.index)
            .map(|&i| {
                self.spec
                    .archives
                    .get(usize::try_from(i).map_err(|_| Error::internal())?)
                    .ok_or_else(Error::internal)
            })
            .transpose()?;
        if let Some(archive) = archive {
            let before = self.keys.row()?.is_none_or(|r| {
                crate::sort::slash_first(archive.key.as_bytes(), r.key.as_bytes()).is_le()
            });
            if before {
                let mut row = Row::marker(&archive.key);
                row.archive = true;
                out.set(self.codec.encode(row)?)?;
                self.index += 1;
                return Ok(true);
            }
        }
        let Some(_) = self.keys.row()? else {
            return Ok(false);
        };
        out.set(self.keys.bytes())?;
        self.keys.advance(stopped)?;
        Ok(true)
    }
}
fn descendant(parent: &str, child: &str) -> bool {
    child
        .strip_prefix(parent)
        .is_some_and(|tail| tail.starts_with('/'))
}
pub(super) fn finish(cx: &mut Context<'_>, artifacts: Artifacts) -> Result<Artifacts, Error> {
    cx.boundary(Pass::Prefix)?;
    let Artifacts {
        units,
        keys,
        counts,
        ..
    } = artifacts;
    let mut source = PrefixSource {
        keys: Cursor::new(keys.as_ref(), &By::Key, cx)?,
        codec: Codec::new(cx.account.meter())?,
    };
    let sorted = cx.sort(&mut source, By::Slash)?;
    drop(source);
    let mut merged = MergeArchives {
        keys: Cursor::new(sorted.as_ref(), &By::Slash, cx)?,
        spec: cx.spec,
        prepared: cx.prepared,
        index: 0,
        codec: Codec::new(cx.account.meter())?,
    };
    let mut next = Frame::new(cx.account.meter())?;
    let mut present = merged.next(&mut next, cx.stopped)?;
    let mut current = Frame::new(cx.account.meter())?;
    let mut last_archive = Buffer::new(cx.account.meter(), 1024)?;
    let mut refused = Output::new(By::Key, cx.account)?;
    let mut affected = Output::new(By::Key, cx.account)?;
    while present {
        current.set(next.bytes())?;
        present = merged.next(&mut next, cx.stopped)?;
        let mut row = decode(current.bytes())?;
        if row.archive {
            last_archive.clear();
            last_archive.extend(row.key.as_bytes())?;
            continue;
        }
        let archive = std::str::from_utf8(&last_archive).map_err(|_| Error::internal())?;
        let state = if present && descendant(row.key, decode(next.bytes())?.key) {
            State::Prefix
        } else if !archive.is_empty() && (row.key == archive || descendant(archive, row.key)) {
            State::Archive
        } else {
            continue;
        };
        row.state = state;
        refused.put(row, cx)?;
        if !row.unit.is_empty() {
            affected.put(Row::marker(row.unit), cx)?;
        }
    }
    let refused = refused.finish(cx)?;
    let affected = affected.finish(cx)?;
    drop(merged);
    retire(cx.workspace, sorted, cx.stopped)?;
    let refused = cx.reorder_unordered(refused, By::Key, By::Key)?;
    let affected = cx.reorder_unordered(affected, By::Key, By::Key)?;
    propagate(cx, units, keys, refused, affected, counts)
}

fn propagate(
    cx: &mut Context<'_>,
    units: Option<crate::spool::workspace::Sealed>,
    keys: Option<crate::spool::workspace::Sealed>,
    refused: Option<crate::spool::workspace::Sealed>,
    affected: Option<crate::spool::workspace::Sealed>,
    mut counts: super::Counts,
) -> Result<Artifacts, Error> {
    cx.boundary(Pass::Propagation)?;
    let units = units::mark(cx, units, affected.as_ref())?;
    let mut cursor = Cursor::new(keys.as_ref(), &By::Key, cx)?;
    let mut refusals = Cursor::new(refused.as_ref(), &By::Key, cx)?;
    let mut output = Output::new(By::Key, cx.account)?;
    while let Some(mut row) = cursor.row()? {
        if refusals.row()?.is_some_and(|r| r.key < row.key) {
            return Err(Error::internal());
        }
        if let Some(refusal) = refusals.row()?.filter(|r| r.key == row.key) {
            row.state = refusal.state;
            refusals.advance(cx.stopped)?;
        }
        output.put(row, cx)?;
        cursor.advance(cx.stopped)?;
    }
    let replaced = output.finish(cx)?;
    drop(cursor);
    drop(refusals);
    retire(cx.workspace, keys, cx.stopped)?;
    retire(cx.workspace, refused, cx.stopped)?;
    let by_unit = cx.reorder(replaced, By::Key, By::Unit)?;
    let mut cursor = Cursor::new(by_unit.as_ref(), &By::Unit, cx)?;
    let mut markers = Cursor::new(affected.as_ref(), &By::Key, cx)?;
    let mut output = Output::new(By::Unit, cx.account)?;
    while let Some(mut row) = cursor.row()? {
        while markers.row()?.is_some_and(|m| m.key < row.unit) {
            markers.advance(cx.stopped)?;
        }
        if !row.unit.is_empty()
            && row.state == State::Publish
            && markers.row()?.is_some_and(|m| m.key == row.unit)
        {
            row.state = State::UnitConflict;
        }
        output.put(row, cx)?;
        cursor.advance(cx.stopped)?;
    }
    let replaced = output.finish(cx)?;
    drop(cursor);
    drop(markers);
    retire(cx.workspace, by_unit, cx.stopped)?;
    retire(cx.workspace, affected, cx.stopped)?;
    let keys = cx.reorder(replaced, By::Unit, By::Key)?;
    cx.boundary(Pass::Winners)?;
    let mut cursor = Cursor::new(keys.as_ref(), &By::Key, cx)?;
    let mut output = Output::new(By::Key, cx.account)?;
    while let Some(row) = cursor.row()? {
        if row.state == State::Publish {
            counts.winners = counts.winners.checked_add(1).ok_or_else(Error::internal)?;
            output.put(row, cx)?;
        }
        cursor.advance(cx.stopped)?;
    }
    let winners = output.finish(cx)?;
    drop(cursor);
    cx.boundary(Pass::Winners)?;
    Ok(Artifacts {
        units,
        keys,
        winners,
        publication_blocked: super::publication_blocked(cx)?,
        counts,
    })
}
