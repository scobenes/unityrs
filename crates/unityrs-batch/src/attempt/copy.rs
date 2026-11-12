use super::{
    AttemptArgs, AttemptEnd, Point,
    classify::{internal, skip_code},
    hooks::Engine,
};
use crate::{
    reason::{KeyRule, LeftOut},
    records::{self, Record, Text, Writer},
};
use unityrs_export::ngui::{self, Failure, Output};

pub(super) fn output(
    args: &AttemptArgs<'_>,
    engine: &Engine<'_>,
    writer: &mut Writer,
    end: &mut AttemptEnd,
    output: Output,
) -> Result<(), Failure> {
    engine.check()?;
    let hashes = engine.records(|work| {
        records::copy_output(args.directory, output.plan_index, &output.bytes, work)
    })?;
    // Export computes MD5 too: disagreement is an implementation error.
    if hashes.md5 != output.md5 {
        return Err(internal());
    }
    engine.records(|work| {
        writer.append(
            Record::Candidate(records::Candidate {
                key: &output.key,
                unit: output.unit.as_deref(),
                input: args.view.input.id,
                serialized_file: Text::Raw(output.serialized_file.as_bytes()),
                kind: output.kind,
                path_id: output.path_id,
                hashes,
                picture: output.picture.as_ref(),
                size: output.bytes.len() as u64,
                plan_index: output.plan_index,
                cut: false,
            }),
            work,
        )
    })?;
    for member in &output.members {
        engine.check()?;
        let archive = args.archive_keys.get(member.archive).ok_or_else(internal)?;
        let (path, reason, cut) = match &member.path {
            Ok(path) => (Text::Raw(path.as_bytes()), None, false),
            Err(r) => {
                let rule = KeyRule::ALL
                    .into_iter()
                    .find(|rule| rule.name() == r.rule.name())
                    .ok_or_else(internal)?;
                (
                    Text::Prepared(&r.path, r.cut),
                    Some(LeftOut::UnwritablePath(rule)),
                    r.cut,
                )
            }
        };
        engine.records(|work| {
            writer.member(
                archive,
                records::Member {
                    archive: member.archive as u64,
                    path,
                    key: &output.key,
                    input: args.view.input.id,
                    md5: &output.md5,
                    reason,
                    cut,
                },
                work,
            )
        })?;
    }
    let index = output.plan_index;
    drop(output); // Return its memory before the cutter makes another output.
    end.outputs = end.outputs.checked_add(1).ok_or_else(internal)?;
    engine.point(Point::Copied(index))
}
pub(super) fn skip(
    input: &str,
    engine: &Engine<'_>,
    writer: &mut Writer,
    end: &mut AttemptEnd,
    skip: &ngui::Skip,
) -> Result<(), Failure> {
    engine.check()?;
    let code = skip_code(&skip.reason.reason).ok_or_else(internal)?;
    end.code = end.code.worst(code);
    end.figures.skips = end.figures.skips.checked_add(1).ok_or_else(internal)?;
    if code == crate::reason::Code::Complete {
        end.skips_expected += 1;
    }
    engine.records(|work| {
        writer.append(
            Record::Skip(records::Skip {
                input,
                serialized_file: Text::Raw(skip.serialized_file.as_bytes()),
                path_id: skip.path_id,
                class: &skip.class,
                output: skip.output,
                sprite: skip.sprite.map(|s| s as u64),
                reason: records::Reason::Text(Text::Prepared(&skip.reason.reason, skip.reason.cut)),
                detail: skip
                    .reason
                    .detail
                    .as_deref()
                    .map(|value| Text::Prepared(value, skip.reason.cut)),
                cut: skip.reason.cut,
            }),
            work,
        )
    })
}
