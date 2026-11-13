//! Plain-key contests joined with immutable unit reservations.
use super::{
    Context, Pass,
    codec::{Row, Rule, State, decode},
    select::Choice,
    source,
    stream::{By, Cursor, Output, retire},
};
use crate::{phase::Error, sort::Frame, spool::workspace::Sealed};

pub(super) fn decide(
    cx: &mut Context<'_>,
    reserved: Option<Sealed>,
) -> Result<(Option<Sealed>, super::Counts), Error> {
    cx.boundary(Pass::Keys)?;
    let candidates = source::sorted(cx, false, By::Key)?;
    let mut cursor = Cursor::new(candidates.as_ref(), &By::Key, cx)?;
    let mut reservations = Cursor::new(reserved.as_ref(), &By::Key, cx)?;
    let mut first = Frame::new(cx.account.meter())?;
    let mut previous = Frame::new(cx.account.meter())?;
    let mut choice = Choice::new(cx)?;
    let mut output = Output::new(By::Key, cx.account)?;
    let mut counts = super::Counts::default();
    while let Some(head) = cursor.row()? {
        first.set(cursor.bytes())?;
        let key = head.key;
        if reservations.row()?.is_some_and(|r| r.key < key) {
            return Err(Error::internal());
        }
        choice.clear();
        let first_row = decode(first.bytes())?;
        let mut count = 0_u64;
        while let Some(row) = cursor.row()? {
            if row.key != first_row.key {
                break;
            }
            if count > 0 && decode(previous.bytes())?.input == row.input {
                return Err(Error::internal());
            }
            if row.unit.is_empty() {
                choice.add(cursor.bytes(), row.key, cx)?;
            }
            previous.set(cursor.bytes())?;
            count = count.checked_add(1).ok_or_else(Error::internal)?;
            cursor.advance(cx.stopped)?;
        }
        let mut decision = if let Some(row) = reservations.row()?.filter(|r| r.key == first_row.key)
        {
            row
        } else if let Some(mut row) = choice.row()? {
            if cx.prepared.withheld(cx, first_row.key)? {
                row.state = State::Withheld;
                row.rule = Rule::Withheld;
            }
            row
        } else {
            Row {
                state: State::UnitLost,
                flip: false,
                ..first_row
            }
        };
        decision.count = count;
        counts.keys = counts.keys.checked_add(1).ok_or_else(Error::internal)?;
        counts.collisions = counts
            .collisions
            .checked_add(u64::from(count > 1))
            .ok_or_else(Error::internal)?;
        counts.flips = counts
            .flips
            .checked_add(u64::from(decision.flip))
            .ok_or_else(Error::internal)?;
        output.put(decision, cx)?;
        if reservations.row()?.is_some_and(|r| r.key == first_row.key) {
            reservations.advance(cx.stopped)?;
        }
    }
    if reservations.row()?.is_some() {
        return Err(Error::internal());
    }
    let result = output.finish(cx)?;
    drop(cursor);
    drop(reservations);
    retire(cx.workspace, candidates, cx.stopped)?;
    retire(cx.workspace, reserved, cx.stopped)?;
    Ok((result, counts))
}
