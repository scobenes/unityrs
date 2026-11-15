use super::Identity;
use crate::{
    manifest::{self, Family},
    phase::{Account, Error},
    progress::Phase,
    reason::Code,
    spec::model::Spec,
    winners::json::Json,
};

/// A final head: its report and code, the phase the controller reached,
/// and the stop it names with its detail (7.3's `stop_reason`).
pub(super) struct Final<'a> {
    pub report: &'a manifest::Report,
    pub code: Code,
    pub phase: Phase,
    pub stop: Option<(&'static str, Option<&'static str>)>,
}
pub(super) fn head(
    spec: &Spec,
    spec_sha256: &[u8; 32],
    account: &Account,
    identity: &Identity<'_>,
    final_head: Option<&Final<'_>>,
    ended_at: Option<&str>,
) -> Result<Json, Error> {
    let mut json = Json::new(account)?;
    json.start("run_id", &spec.run_id)?;
    // A version 2 spec's head adds its options (7.3): `manifest_version` 2.
    json.value("manifest_version", spec.spec_version.min(2))?;
    json.value("attempt", spec.attempt)?;
    json.value("write_seq", u8::from(final_head.is_some()))?;
    json.text(
        "status",
        final_head.map_or(Some("running"), |f| f.code.status()),
    )?;
    json.text(
        "phase",
        Some(final_head.map_or(Phase::Start, |f| f.phase).name()),
    )?;
    json.optional("exit_code", final_head.map(|f| u64::from(f.code.value())))?;
    let stop = final_head.and_then(|f| f.stop);
    json.text("stop_reason", stop.map(|(reason, _)| reason))?;
    json.text("stop_detail", stop.and_then(|(_, detail)| detail))?;
    json.text("binary_version", Some(env!("CARGO_PKG_VERSION")))?;
    json.value("spec_version", spec.spec_version)?;
    json.hex("spec_sha256", Some(spec_sha256))?;
    if spec.spec_version >= 2 {
        // The effective options, never the key: whether one was named.
        json.value("formats", FormatList(spec.outputs.formats))?;
        match spec.outputs.image {
            crate::spec::model::Image::Png => json.text("image", Some("png"))?,
            crate::spec::model::Image::PngFast => json.text("image", Some("png_fast"))?,
            crate::spec::model::Image::Webp { quality } => {
                json.object("image")?;
                json.object("webp")?;
                json.value("quality", quality)?;
                json.close_object()?;
                json.close_object()?;
            }
        }
        json.value("unitycn_key", spec.unitycn_key.is_some())?;
    }
    json.text("started_at", Some(identity.started_at))?;
    let mut deadline = [0; 30];
    let deadline = spec
        .deadline
        .as_ref()
        .map(|d| utc(&d.at, &mut deadline))
        .transpose()?;
    json.text("deadline", deadline)?;
    json.text("ended_at", ended_at)?;
    json.text("verified", None)?;
    json.value("verified_by_size", 0)?;
    json.value("unsettled_tries", 0)?;
    let empty = manifest::Report::default();
    let report = final_head.map_or(&empty, |f| f.report);
    counts(&mut json, &report.counts)?;
    run(&mut json, spec, &report.run, identity, final_head.is_some())?;
    json.object("warnings")?;
    let warnings = identity.warnings.read();
    for (warning, count) in [
        ("resume_list_failed", 0),
        ("verify_list_failed", 0),
        ("etag_not_md5", 0),
        ("stderr_dropped", warnings.stderr_dropped),
        ("abort_failed", 0),
        ("stdin_watch_ended", u64::from(warnings.stdin_watch_ended)),
    ] {
        json.value(warning, count)?;
    }
    json.close_object()?;
    json.object("rows")?;
    for (family, count) in Family::ALL.into_iter().zip(report.rows) {
        json.optional(family.name(), count)?;
    }
    json.close_object()?;
    Ok(json)
}
/// `formats` (7.3): the names, in `TextureFormat` order, as a JSON array.
/// Each is a `TextureFormat` member name, which needs no escaping.
struct FormatList(unityrs_export::ngui::Formats);
impl std::fmt::Display for FormatList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[")?;
        for (i, format) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(",")?;
            }
            write!(f, "\"{}\"", format.name())?;
        }
        f.write_str("]")
    }
}
fn counts(json: &mut Json, counts: &manifest::counts::Counts) -> Result<(), Error> {
    json.object("counts")?;
    macro_rules! fields { ($($field:ident),+ $(,)?) => { $(json.value(stringify!($field), counts.$field)?;)+ }; }
    fields!(
        bundles,
        bundles_failed,
        inputs,
        inputs_complete,
        inputs_with_skips,
        inputs_failed,
        inputs_not_run,
        outputs_written,
        outputs_present,
        outputs_not_written,
        outputs_withheld,
        outputs_failed,
        outputs_unwritable,
        skips,
        skips_expected,
        collisions,
        flips,
        archives_written,
        archives_present
    );
    json.close_object()
}
fn run(
    json: &mut Json,
    spec: &crate::spec::model::Spec,
    figures: &manifest::figures::Figures,
    identity: &Identity<'_>,
    final_head: bool,
) -> Result<(), Error> {
    json.object("run")?;
    macro_rules! fields { ($($field:ident),+ $(,)?) => { $(json.value(stringify!($field), figures.$field)?;)+ }; }
    fields!(
        same_named_cabs,
        undeclared_dependencies,
        nested_containers,
        evictions,
        input_peak,
        dependency_peak,
        export_work_peak,
        decode_work_peak,
        peek_peak,
        peek_work_peak,
        spooled_parts
    );
    json.value(
        "spool_peak",
        if final_head { identity.spool_peak } else { 0 },
    )?;
    json.value("vm_hwm", if final_head { identity.vm_hwm } else { 0 })?;
    json.value("memory_bound", identity.memory_bound)?;
    json.value(
        "max_inflight_uncompressed_bytes",
        spec.max_inflight_uncompressed_bytes,
    )?;
    json.value("max_decoded_bytes", spec.max_decoded_bytes)?;
    json.value("dep_cache_bytes", spec.dep_cache_bytes)?;
    json.value("max_attempt_evictions", spec.max_attempt_evictions)?;
    json.value("max_export_work", spec.max_export_work)?;
    json.object("spool")?;
    json.value("size_bytes", spec.spool.size_bytes)?;
    json.value("reserve_bytes", spec.spool.reserve_bytes)?;
    json.close_object()?;
    json.close_object()
}
/// A deadline as the head writes it (7.3): RFC 3339 in UTC, its fraction
/// kept to the nanosecond, trailing zeros dropped.
fn utc<'a>(at: &crate::spec::model::DateTime, out: &'a mut [u8; 30]) -> Result<&'a str, Error> {
    use std::io::Write;
    let ns = crate::lifetime::unix_ns(at).map_err(|_| Error::internal())?;
    let seconds = i64::try_from(ns.div_euclid(1_000_000_000)).map_err(|_| Error::internal())?;
    let mut nanos = ns.rem_euclid(1_000_000_000);
    let date = crate::sys::utc(seconds).map_err(|_| Error::internal())?;
    let mut used = 19;
    out.get_mut(..used)
        .ok_or_else(Error::internal)?
        .copy_from_slice(date.get(..used).ok_or_else(Error::internal)?);
    if nanos != 0 {
        let mut width = 9;
        while nanos % 10 == 0 {
            nanos /= 10;
            width -= 1;
        }
        let mut rest = out.get_mut(used..).ok_or_else(Error::internal)?;
        write!(rest, ".{nanos:0width$}").map_err(|_| Error::internal())?;
        used += 1 + width;
    }
    *out.get_mut(used).ok_or_else(Error::internal)? = b'Z';
    std::str::from_utf8(out.get(..=used).ok_or_else(Error::internal)?)
        .map_err(|_| Error::internal())
}
