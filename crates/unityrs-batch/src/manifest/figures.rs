//! Logical received figures are independent of row publication. No queued
//! end, worker-published word or destination byte is used to reconstruct them.
use crate::{
    phase::{Error, check},
    phase1::Facts,
    records::{DiagnosticKind, read},
};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Figures {
    pub same_named_cabs: u64,
    pub undeclared_dependencies: u64,
    pub nested_containers: u64,
    pub evictions: u64,
    pub input_peak: u64,
    pub dependency_peak: u64,
    pub export_work_peak: u64,
    pub decode_work_peak: u64,
    pub peek_peak: u64,
    pub peek_work_peak: u64,
    pub spooled_parts: u64,
}
/// Fixed scalar projection of received ends. Diagnostic kinds require the
/// separate validated scan below, since B5 retains only their combined count.
pub fn received(prepared: &Facts) -> Result<Figures, Error> {
    let mut out = Figures::default();
    for state in &prepared.inputs {
        if let Some(r) = state.received {
            out.evictions = out
                .evictions
                .checked_add(r.figures.evictions)
                .ok_or_else(Error::internal)?;
            out.spooled_parts = out
                .spooled_parts
                .checked_add(r.spooled_part)
                .ok_or_else(Error::internal)?;
            out.input_peak = out.input_peak.max(r.figures.input_peak);
            out.dependency_peak = out.dependency_peak.max(r.figures.dependency_peak);
            out.export_work_peak = out.export_work_peak.max(r.figures.export_work);
            out.decode_work_peak = out.decode_work_peak.max(r.figures.decode_work_peak);
        }
    }
    for state in &prepared.bundles {
        if let Some(r) = state.peek {
            out.peek_peak = out.peek_peak.max(r.input_peak);
            out.peek_work_peak = out.peek_work_peak.max(r.decode_work);
        }
    }
    Ok(out)
}
pub(super) fn complete(cx: &super::rows::Context<'_>) -> Result<Figures, Error> {
    let mut out = received(cx.facts)?;
    for (i, state) in cx.facts.inputs.iter().enumerate() {
        check(cx.stopped)?;
        let index = u32::try_from(i).map_err(|_| Error::internal())?;
        // Received records count only after a confirmed commit. A failed
        // attempt's discarded diagnostic rows do not become logical records.
        if !cx.committed(index) {
            continue;
        }
        let id = cx.facts.spec.bundle(i).ok_or_else(Error::internal)?.id();
        let mut reader = read::Reader::new(
            cx.inputs.open(index)?,
            id,
            cx.inputs.failed(index)?,
            u64::try_from(cx.facts.spec.archives.len()).map_err(|_| Error::internal())?,
            cx.account.meter(),
        )?;
        while let Some(row) = reader.next(cx.stopped)? {
            match row {
                read::Row::Diagnostic(d) => super::counts::increment(match d.kind {
                    DiagnosticKind::SameNamedCab => &mut out.same_named_cabs,
                    DiagnosticKind::UndeclaredDependency => &mut out.undeclared_dependencies,
                    DiagnosticKind::NestedContainer => &mut out.nested_containers,
                })?,
                read::Row::Summary(s) if state.received.is_none() => {
                    // A received controller failure write before dispatch has
                    // no AttemptFacts, but its committed flush still counts.
                    out.spooled_parts = out
                        .spooled_parts
                        .checked_add(s.spooled_part)
                        .ok_or_else(Error::internal)?;
                }
                _ => {}
            }
        }
    }
    Ok(out)
}
