use super::Identity;
use crate::{
    manifest::{self, Family},
    phase::{Account, Error},
    phase1::PreparedRun,
    reason::Code,
    winners::json::Json,
};

pub(super) fn head(
    prepared: &PreparedRun,
    account: &Account,
    identity: &Identity<'_>,
    final_head: Option<(&manifest::Report, Code)>,
    ended_at: Option<&str>,
) -> Result<Json, Error> {
    let spec = &prepared.spec;
    let mut json = Json::new(account)?;
    json.start("run_id", &spec.run_id)?;
    // A version 2 spec's head adds its options (7.3): `manifest_version` 2.
    json.value("manifest_version", spec.spec_version.min(2))?;
    json.value("attempt", spec.attempt)?;
    json.value("write_seq", u8::from(final_head.is_some()))?;
    json.text(
        "status",
        final_head.map_or(Some("running"), |(_, code)| code.status()),
    )?;
    json.text(
        "phase",
        Some(if final_head.is_some() {
            "publish"
        } else {
            "start"
        }),
    )?;
    json.optional(
        "exit_code",
        final_head.map(|(_, code)| u64::from(code.value())),
    )?;
    json.text(
        "stop_reason",
        final_head.and_then(|(report, _)| {
            report
                .failure
                .filter(|_| report.filesystem_sync_failed)
                .map(crate::job::RunFailure::name)
        }),
    )?;
    json.text("stop_detail", None)?;
    json.text("binary_version", Some(env!("CARGO_PKG_VERSION")))?;
    json.value("spec_version", spec.spec_version)?;
    json.hex("spec_sha256", Some(&prepared.spec_sha256))?;
    if spec.spec_version >= 2 {
        // The effective options, never the key: whether one was named.
        json.value("formats", FormatList(spec.outputs.formats))?;
        json.text("image", Some(spec.outputs.image.name()))?;
        json.value("unitycn_key", spec.unitycn_key.is_some())?;
    }
    json.text("started_at", Some(identity.started_at))?;
    json.text("deadline", None)?;
    json.text("ended_at", ended_at)?;
    json.text("verified", None)?;
    json.value("verified_by_size", 0)?;
    json.value("unsettled_tries", 0)?;
    let empty = manifest::Report::default();
    let report = final_head.map_or(&empty, |(r, _)| r);
    counts(&mut json, &report.counts)?;
    run(&mut json, spec, &report.run, identity, final_head.is_some())?;
    json.object("warnings")?;
    for warning in [
        "resume_list_failed",
        "verify_list_failed",
        "etag_not_md5",
        "stderr_dropped",
        "abort_failed",
        "stdin_watch_ended",
    ] {
        json.value(warning, 0)?;
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
