//! Unit instances, immutable reservations and external conflict propagation.
use super::{
    Context, Pass,
    codec::{Codec, Row, Rule, State, decode},
    select::Choice,
    source,
    stream::{By, Cursor, Output, retire},
};
use crate::{
    phase::Error,
    sort::{Frame, Source},
    spool::workspace::Sealed,
};

pub(super) fn roots(cx: &mut Context<'_>) -> Result<Option<Sealed>, Error> {
    cx.boundary(Pass::Roots)?;
    let file = source::sorted(cx, true, By::Instance)?;
    let mut cursor = Cursor::new(file.as_ref(), &By::Instance, cx)?;
    let mut previous = Frame::new(cx.account.meter())?;
    let mut choice = Choice::new(cx)?;
    let mut output = Output::new(By::Key, cx.account)?;
    let mut codec = Codec::new(cx.account.meter())?;
    let mut decision = Frame::new(cx.account.meter())?;
    let mut present = false;
    while let Some(row) = cursor.row()? {
        let old = if present {
            Some(decode(previous.bytes())?)
        } else {
            None
        };
        if old.is_some_and(|p| p.unit != row.unit) {
            let key = old.ok_or_else(Error::internal)?.unit;
            choice.decision(key, cx, &mut codec, &mut decision)?;
            let mut d = decode(decision.bytes())?;
            d.key = d.unit;
            output.put(d, cx)?;
            choice.clear();
        }
        let new_instance = old.is_none_or(|p| p.unit != row.unit || p.input != row.input);
        if new_instance {
            if row.key != row.unit {
                return Err(Error::internal());
            }
            choice.add(cursor.bytes(), row.unit, cx)?;
        } else if old.is_some_and(|p| p.key == row.key) {
            return Err(Error::internal());
        }
        previous.set(cursor.bytes())?;
        present = true;
        cursor.advance(cx.stopped)?;
    }
    if present {
        let key = decode(previous.bytes())?.unit;
        choice.decision(key, cx, &mut codec, &mut decision)?;
        let mut d = decode(decision.bytes())?;
        d.key = d.unit;
        output.put(d, cx)?;
    }
    let result = output.finish(cx)?;
    drop(cursor);
    retire(cx.workspace, file, cx.stopped)?;
    Ok(result)
}
struct Claims<'a> {
    members: Cursor<'a>,
    units: Cursor<'a>,
    first: Frame,
    chosen: Frame,
    codec: Codec,
}
impl Source for Claims<'_> {
    fn next(&mut self, out: &mut Frame, stopped: &dyn Fn() -> bool) -> Result<bool, Error> {
        loop {
            let Some(head) = self.members.row()? else {
                return Ok(false);
            };
            self.first.set(self.members.bytes())?;
            while self.units.row()?.is_some_and(|u| u.key < head.unit) {
                self.units.advance(stopped)?;
            }
            let unit = self
                .units
                .row()?
                .filter(|u| u.key == head.unit)
                .ok_or_else(Error::internal)?;
            let first = decode(self.first.bytes())?;
            let mut found = false;
            let mut different = false;
            while let Some(row) = self.members.row()? {
                if row.unit != first.unit || row.key != first.key {
                    break;
                }
                different |= row.md5 != first.md5;
                if row.input == unit.input {
                    if found {
                        return Err(Error::internal());
                    }
                    self.chosen.set(self.members.bytes())?;
                    found = true;
                }
                self.members.advance(stopped)?;
            }
            if !found && unit.state != State::Withheld {
                continue;
            }
            let mut row = decode(if found {
                self.chosen.bytes()
            } else {
                self.first.bytes()
            })?;
            row.rule = unit.rule;
            row.state = unit.state;
            row.flip = unit.rule == Rule::FirstInput && different;
            out.set(self.codec.encode(row)?)?;
            return Ok(true);
        }
    }
}

pub(super) fn reserve(
    cx: &mut Context<'_>,
    units: Option<Sealed>,
) -> Result<(Option<Sealed>, Option<Sealed>), Error> {
    cx.boundary(Pass::Claims)?;
    let members = source::sorted(cx, true, By::Member)?;
    let mut source = Claims {
        members: Cursor::new(members.as_ref(), &By::Member, cx)?,
        units: Cursor::new(units.as_ref(), &By::Key, cx)?,
        first: Frame::new(cx.account.meter())?,
        chosen: Frame::new(cx.account.meter())?,
        codec: Codec::new(cx.account.meter())?,
    };
    let claims = cx.sort(&mut source, By::Claim)?;
    drop(source);
    retire(cx.workspace, members, cx.stopped)?;
    cx.boundary(Pass::Keepers)?;
    let mut cursor = Cursor::new(claims.as_ref(), &By::Claim, cx)?;
    let mut keepers = Output::new(By::Key, cx.account)?;
    // Markers are intentionally unsorted here and sorted externally below.
    let mut conflicts = Output::new(By::Key, cx.account)?;
    let mut previous = Frame::new(cx.account.meter())?;
    let mut present = false;
    while let Some(row) = cursor.row()? {
        if !present || decode(previous.bytes())?.key != row.key {
            keepers.put(row, cx)?;
        } else {
            conflicts.put(Row::marker(row.unit), cx)?;
        }
        previous.set(cursor.bytes())?;
        present = true;
        cursor.advance(cx.stopped)?;
    }
    let keepers = keepers.finish(cx)?;
    let conflicts = conflicts.finish(cx)?;
    drop(cursor);
    retire(cx.workspace, claims, cx.stopped)?;
    // Writer framing is shared, but unsorted marker readers need the raw
    // source adapter: sorting cannot demand that the input already be sorted.
    let conflicts = cx.reorder_unordered(conflicts, By::Key, By::Key)?;
    cx.boundary(Pass::UnitConflicts)?;
    let units = mark(cx, units, conflicts.as_ref())?;
    retire(cx.workspace, conflicts, cx.stopped)?;
    cx.boundary(Pass::Reservations)?;
    let keepers = cx.reorder(keepers, By::Key, By::Unit)?;
    let mut keys = Cursor::new(keepers.as_ref(), &By::Unit, cx)?;
    let mut states = Cursor::new(units.as_ref(), &By::Key, cx)?;
    let mut joined = Output::new(By::Unit, cx.account)?;
    while let Some(mut row) = keys.row()? {
        while states.row()?.is_some_and(|u| u.key < row.unit) {
            states.advance(cx.stopped)?;
        }
        let unit = states
            .row()?
            .filter(|u| u.key == row.unit)
            .ok_or_else(Error::internal)?;
        row.state = unit.state;
        joined.put(row, cx)?;
        keys.advance(cx.stopped)?;
    }
    let joined = joined.finish(cx)?;
    drop(keys);
    drop(states);
    retire(cx.workspace, keepers, cx.stopped)?;
    let reserved = cx.reorder(joined, By::Unit, By::Key)?;
    Ok((units, reserved))
}
/// Join a sorted (possibly repeated) set of unit markers without a set in RAM.
pub(super) fn mark(
    cx: &mut Context<'_>,
    units: Option<Sealed>,
    markers: Option<&Sealed>,
) -> Result<Option<Sealed>, Error> {
    let mut cursor = Cursor::new(units.as_ref(), &By::Key, cx)?;
    let mut marks = Cursor::new(markers, &By::Key, cx)?;
    let mut output = Output::new(By::Key, cx.account)?;
    while let Some(mut unit) = cursor.row()? {
        while marks.row()?.is_some_and(|m| m.key < unit.key) {
            marks.advance(cx.stopped)?;
        }
        if marks.row()?.is_some_and(|m| m.key == unit.key) && unit.state != State::Withheld {
            unit.state = State::UnitConflict;
        }
        output.put(unit, cx)?;
        cursor.advance(cx.stopped)?;
    }
    let result = output.finish(cx)?;
    drop(cursor);
    drop(marks);
    retire(cx.workspace, units, cx.stopped)?;
    Ok(result)
}
