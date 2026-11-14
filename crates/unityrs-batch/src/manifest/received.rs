//! Received facts and validated committed records. Every scan reaches summary
//! and EOF; inherited source-byte cuts are preserved without truncating again.
use super::{
    Family,
    chunks::Chunks,
    counts::{Counts, increment},
    rows::Context,
};
use crate::{
    phase::{Error, check},
    records::{
        Status,
        read::{self, Row},
    },
    winners::json::Json,
};

pub(super) fn bundles(
    cx: &Context<'_>,
    out: &mut Chunks<'_>,
    counts: &mut Counts,
    json: &mut Json,
) -> Result<(), super::WriteError> {
    for &index in cx.facts.graph.order() {
        check(cx.stopped)?;
        let i = usize::try_from(index).map_err(|_| Error::internal())?;
        let state = cx.facts.bundles.get(i).ok_or_else(Error::internal)?;
        let id = cx.facts.spec.bundle(i).ok_or_else(Error::internal)?.id();
        // A stop or a run failure ends a staging try without a data reason:
        // the bundle is not staged, never a data-failure row (7.3).
        let failure = state
            .stage
            .and_then(|s| s.failure)
            .filter(|f| matches!(f, crate::stage::Failure::Data(_)));
        let status = match state.stage {
            Some(s) if s.failure.is_none() => "staged",
            Some(_) if failure.is_some() => "failed",
            _ => "not_staged",
        };
        json.start("id", id)?;
        json.text("status", Some(status))?;
        bundle_reason(json, failure)?;
        let culprit = cx.culprit(crate::control::Culprit::Bundle(index));
        match failure {
            _ if culprit.is_some() => json.text("detail", culprit)?,
            Some(crate::stage::Failure::Data(crate::stage::DataFailure::DownloadFailed(d))) => {
                json.display_text("detail", d)?;
            }
            Some(crate::stage::Failure::Data(crate::stage::DataFailure::RedirectRefused(
                status,
            ))) => json.display_text("detail", crate::http::Detail::Status(status))?,
            _ => json.text(
                "detail",
                state
                    .stage
                    .and_then(|s| s.integrity)
                    .map(crate::stage::Integrity::name),
            )?,
        }
        let facts = state.stage.and_then(|s| s.facts);
        json.optional("size", facts.map(|f| f.size))?;
        json.hex("sha256", facts.as_ref().map(|f| f.sha256.as_slice()))?;
        json.value("peek_peak", state.peek.map_or(0, |p| p.input_peak))?;
        json.value("peek_work", state.peek.map_or(0, |p| p.decode_work))?;
        peek_reason(
            json,
            state.peek.and_then(|p| p.outcome),
            cx.facts.spec.max_decoded_bytes,
        )?;
        out.put(json.end()?)?;
        counts.status(Family::Bundles, status)?;
    }
    Ok(())
}
fn bundle_reason(json: &mut Json, failure: Option<crate::stage::Failure>) -> Result<(), Error> {
    use crate::stage::{DataFailure, Failure};
    match failure {
        None => json.text("reason", None),
        Some(Failure::Data(f)) => match f {
            DataFailure::DownloadFailed(_) => json.text("reason", Some("download_failed")),
            DataFailure::RedirectRefused(_) => json.text("reason", Some("redirect_refused")),
            DataFailure::SourceMissing => json.text("reason", Some("source_missing")),
            DataFailure::SourceNotRegular => json.text("reason", Some("source_not_regular")),
            DataFailure::SourceChanged => json.text("reason", Some("source_changed")),
            DataFailure::IntegrityMismatch => json.text("reason", Some("integrity_mismatch")),
            DataFailure::InputBytes(n) => json.display_text(
                "reason",
                crate::reason::Limit {
                    kind: crate::reason::LimitKind::InputBytes,
                    value: n,
                },
            ),
        },
        // Filtered by the caller: a stop's staging end is `not_staged`.
        Some(Failure::Run(_) | Failure::Stopped) => Err(Error::internal()),
    }
}
fn peek_reason(
    json: &mut Json,
    outcome: Option<crate::peek::word::Outcome>,
    m: u64,
) -> Result<(), Error> {
    use crate::peek::word::{FailureCode, Outcome};
    match outcome {
        Some(Outcome::Failed(f)) => match f {
            FailureCode::NotABundle => json.text("peek", Some("not_a_bundle")),
            FailureCode::Container(_) => json.text("peek", Some("container_error")),
            FailureCode::Limit(_) => {
                json.display_text("peek", f.limit(m).ok_or_else(Error::internal)?)
            }
        },
        _ => json.text("peek", None),
    }
}
pub(super) fn inputs(
    family: Family,
    cx: &Context<'_>,
    out: &mut Chunks<'_>,
    counts: &mut Counts,
    json: &mut Json,
) -> Result<(), super::WriteError> {
    for &index in cx.facts.graph.order() {
        check(cx.stopped)?;
        let i = usize::try_from(index).map_err(|_| Error::internal())?;
        if i >= cx.facts.spec.input_count() {
            continue;
        }
        let id = cx.facts.spec.bundle(i).ok_or_else(Error::internal)?.id();
        if !cx.committed(index) {
            if family == Family::Inputs {
                not_run(json, id, index, cx)?;
                out.put(json.end()?)?;
                counts.status(family, "not_run")?;
            }
            continue;
        }
        let mut reader = read::Reader::new(
            cx.inputs.open(index)?,
            id,
            cx.inputs.failed(index)?,
            u64::try_from(cx.facts.spec.archives.len()).map_err(|_| Error::internal())?,
            cx.account.meter(),
        )?;
        while let Some(row) = reader.next(cx.stopped)? {
            match (family, row) {
                (Family::Inputs, Row::Summary(s)) => {
                    summary(json, &s)?;
                    counts.status(family, status_name(s.status))?;
                }
                (Family::Skips, Row::Skip(s)) => {
                    skip(json, &s)?;
                    increment(&mut counts.skips)?;
                    if expected(s.reason) {
                        increment(&mut counts.skips_expected)?;
                    }
                }
                (Family::Diagnostics, Row::Diagnostic(d)) => diagnostic(json, &d)?,
                _ => continue,
            }
            out.put(json.end()?)?;
        }
    }
    Ok(())
}
/// Dispatched, or failed before dispatch, and not confirmed committed:
/// `stopped`; never dispatched: no reason (2.4, `BATCH_SPEC.md` 7.3).
fn not_run(json: &mut Json, id: &str, index: u32, cx: &Context<'_>) -> Result<(), Error> {
    let uncommitted = cx
        .facts
        .uncommitted
        .get(usize::try_from(index).map_err(|_| Error::internal())?)
        .ok_or_else(Error::internal)?;
    json.start("id", id)?;
    json.text("status", Some("not_run"))?;
    json.text("reason", uncommitted.then_some("stopped"))?;
    json.text(
        "detail",
        cx.culprit(crate::control::Culprit::Input(index))
            .filter(|_| uncommitted),
    )?;
    for field in ["input_peak", "dependency_peak", "evictions", "export_work"] {
        json.text(field, None)?;
    }
    // A dispatched `stopped` row's `ms` runs from dispatch to the hand-over.
    let ms = uncommitted
        .then(|| cx.facts.dispatched(index))
        .flatten()
        .map(|start| {
            cx.taken_at
                .checked_duration_since(start)
                .ok_or_else(Error::internal)
                .and_then(|elapsed| {
                    u64::try_from(elapsed.as_millis()).map_err(|_| Error::internal())
                })
        })
        .transpose()?;
    json.optional("ms", ms)?;
    json.value("skips", 0)?;
    json.value("unresolved_scripts", 0)?;
    json.value("cut", false)
}
fn status_name(status: Status) -> &'static str {
    match status {
        Status::Complete => "complete",
        Status::CompleteWithSkips => "complete_with_skips",
        Status::Failed => "failed",
    }
}
fn summary(json: &mut Json, s: &read::Summary<'_>) -> Result<(), Error> {
    json.start("id", s.id)?;
    json.text("status", Some(status_name(s.status)))?;
    json.text("reason", s.reason)?;
    json.text("detail", s.detail)?;
    json.optional("input_peak", s.figures.map(|f| f.input_peak))?;
    json.optional("dependency_peak", s.figures.map(|f| f.dependency_peak))?;
    json.optional("evictions", s.figures.map(|f| f.evictions))?;
    json.optional("export_work", s.figures.map(|f| f.export_work))?;
    json.optional("ms", s.figures.map(|f| f.ms))?;
    json.value("skips", s.figures.map_or(0, |f| f.skips))?;
    json.value(
        "unresolved_scripts",
        s.figures.map_or(0, |f| f.unresolved_scripts),
    )?;
    json.value("cut", s.cut)
}
fn skip(json: &mut Json, s: &read::Skip<'_>) -> Result<(), Error> {
    json.start("input", s.input)?;
    json.text("serialized_file", Some(s.serialized_file))?;
    json.value("path_id", s.path_id)?;
    json.text("class", Some(s.class))?;
    json.text(
        "output",
        Some(match s.output {
            read::Kind::Texture => "texture",
            read::Kind::Sprite => "sprite",
            read::Kind::AtlasJson => "atlas_json",
            read::Kind::AtlasSprite => "atlas_sprite",
            read::Kind::Atlas => "atlas",
        }),
    )?;
    json.optional("sprite", s.sprite)?;
    json.text("reason", Some(s.reason))?;
    json.text("detail", s.detail)?;
    json.value("cut", s.cut)
}
fn diagnostic(json: &mut Json, d: &read::Diagnostic<'_>) -> Result<(), Error> {
    json.start("kind", diagnostic_name(d.kind))?;
    json.text("input", Some(d.input))?;
    json.text("bundle", d.bundle)?;
    json.text("name", Some(d.name))?;
    json.value("cut", d.cut)
}
fn diagnostic_name(kind: crate::records::DiagnosticKind) -> &'static str {
    use crate::records::DiagnosticKind;
    match kind {
        DiagnosticKind::SameNamedCab => "same_named_cab",
        DiagnosticKind::UndeclaredDependency => "undeclared_dependency",
        DiagnosticKind::NestedContainer => "nested_container",
    }
}
fn expected(reason: &str) -> bool {
    crate::attempt::classify::skip_code(reason) == Some(crate::reason::Code::Complete)
}
