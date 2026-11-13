//! Candidate-rule joins and the two candidate-derived base row streams.
use super::{
    Artifacts, Context, Pass, Row, Rule, State,
    json::{Json, Rows},
    source,
    stream::{By, Cursor, Output, retire},
};
use crate::{phase::Error, spool::workspace::Sealed};

pub(super) fn materialize(
    cx: &mut Context<'_>,
    artifacts: &mut Artifacts,
) -> Result<(Option<Sealed>, Option<Sealed>), Error> {
    cx.boundary(Pass::Rows)?;
    // Unit-order candidates acquire their own instance's rule before returning
    // to key/input order. Plain candidates sort before nonempty unit keys.
    let candidates = source::sorted(cx, false, By::Unit)?;
    let mut source = Cursor::new(candidates.as_ref(), &By::Unit, cx)?;
    let mut units = Cursor::new(artifacts.units.as_ref(), &By::Key, cx)?;
    let mut annotated = Output::new(By::Key, cx.account)?;
    while let Some(mut c) = source.row()? {
        if !c.unit.is_empty() {
            while units.row()?.is_some_and(|u| u.key < c.unit) {
                units.advance(cx.stopped)?;
            }
            let u = units
                .row()?
                .filter(|u| u.key == c.unit)
                .ok_or_else(Error::internal)?;
            c.rule = if u.state == State::Withheld {
                Rule::Withheld
            } else if u.state == State::UnitConflict && u.input == c.input {
                Rule::UnitConflict
            } else {
                u.rule
            };
        }
        annotated.put(c, cx)?;
        source.advance(cx.stopped)?;
    }
    drop((source, units));
    retire(cx.workspace, candidates, cx.stopped)?;
    let annotated = annotated.finish(cx)?;
    let annotated = cx.reorder_unordered(annotated, By::Key, By::Key)?;
    let mut source = Cursor::new(annotated.as_ref(), &By::Key, cx)?;
    let mut keys = Cursor::new(artifacts.keys.as_ref(), &By::Key, cx)?;
    let mut outputs = Rows::new(cx.account)?;
    let mut collisions = Rows::new(cx.account)?;
    let mut json = Json::new(cx.account)?;
    while let Some(k) = keys.row()? {
        output(&mut json, k, artifacts.publication_state(k.state))?;
        outputs.put(json.end()?, cx)?;
        let mut count = 0_u64;
        let mut representatives = 0_u64;
        while let Some(mut c) = source.row()?.filter(|c| c.key == k.key) {
            let winner = c.input == k.input && c.unit == k.unit;
            representatives += u64::from(winner);
            count = count.checked_add(1).ok_or_else(Error::internal)?;
            if c.unit.is_empty() {
                c.rule = if !k.unit.is_empty() && k.state != State::UnitLost {
                    Rule::Unit
                } else {
                    k.rule
                };
            }
            if k.count > 1 {
                collision(&mut json, c, k, winner)?;
                collisions.put(json.end()?, cx)?;
            }
            source.advance(cx.stopped)?;
        }
        if count != k.count || representatives != 1 {
            return Err(Error::internal());
        }
        keys.advance(cx.stopped)?;
    }
    if source.row()?.is_some() {
        return Err(Error::internal());
    }
    drop((source, keys));
    retire(cx.workspace, annotated, cx.stopped)?;
    retire(cx.workspace, artifacts.units.take(), cx.stopped)?;
    retire(cx.workspace, artifacts.keys.take(), cx.stopped)?;
    Ok((outputs.finish(cx)?, collisions.finish(cx)?))
}
pub(super) fn output(json: &mut Json, row: Row<'_>, state: State) -> Result<(), Error> {
    json.start("key", row.key)?;
    json.text(
        "kind",
        Some(match row.kind {
            0 => "texture",
            1 => "sprite",
            2 => "atlas_json",
            3 => "atlas_sprite",
            _ => return Err(Error::internal()),
        }),
    )?;
    json.text("unit", (!row.unit.is_empty()).then_some(row.unit))?;
    json.text("input", Some(row.input))?;
    json.text("serialized_file", Some(row.serialized_file))?;
    json.value("path_id", row.path_id)?;
    json.hex("md5", Some(&row.md5))?;
    json.hex(
        "visible_hash",
        row.visible_hash.as_ref().map(<[u8; 32]>::as_slice),
    )?;
    json.text(
        "mode",
        match row.mode {
            0 => None,
            1 => Some("L"),
            2 => Some("LA"),
            3 => Some("RGB"),
            4 => Some("RGBA"),
            _ => return Err(Error::internal()),
        },
    )?;
    json.value("size", row.size)?;
    json.text(
        "status",
        Some(match state {
            State::Publish => "not_written",
            State::Prefix | State::Archive => "unwritable_key",
            _ => "withheld",
        }),
    )?;
    json.text("reason", state.reason())?;
    json.value("cut", row.cut)
}

pub(super) fn collision(
    json: &mut Json,
    c: Row<'_>,
    k: Row<'_>,
    winner: bool,
) -> Result<(), Error> {
    json.start("key", c.key)?;
    json.text("input", Some(c.input))?;
    json.text("serialized_file", Some(c.serialized_file))?;
    json.hex("md5", Some(&c.md5))?;
    json.hex(
        "visible_hash",
        c.visible_hash.as_ref().map(<[u8; 32]>::as_slice),
    )?;
    json.value("winner", winner)?;
    json.text("rule", Some(c.rule.name()))?;
    json.value("flip", k.flip)?;
    json.value(
        "pixels_differ",
        !winner
            && c.visible_hash
                .zip(k.visible_hash)
                .is_some_and(|(a, b)| a != b),
    )?;
    json.value("cut", c.cut)?;
    Ok(())
}
