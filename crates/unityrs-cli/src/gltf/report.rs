//! Stream reports to bounded sinks; reasons are fixed ASCII, never source text.
use super::Result;
use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use unityrs_core::budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation};
use unityrs_core::limits::Meter;
use unityrs_gltf::{Reason, Report};
use unityrs_scene::Issue;
#[derive(Debug, Default)]
pub(super) struct Home {
    live: AtomicU64,
    peak: AtomicU64,
}
impl PermitHome for Home {
    fn release(&self, bytes: u64) {
        self.live.fetch_sub(bytes, Ordering::Relaxed);
    }
}
#[derive(Debug, Default)]
pub(super) struct Peak {
    home: Arc<Home>,
}
impl MemoryBudget for Peak {
    fn try_reserve(
        &self,
        bytes: u64,
        what: Reservation,
    ) -> std::result::Result<Permit, BudgetExceeded> {
        let old = self
            .home
            .live
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                live.checked_add(bytes)
            })
            .map_err(|_| BudgetExceeded {
                requested: bytes,
                what,
            })?;
        self.home.peak.fetch_max(old + bytes, Ordering::Relaxed);
        Ok(Permit::new(bytes, self.home.clone()))
    }
}
impl Peak {
    pub fn peak(&self) -> u64 {
        self.home.peak.load(Ordering::Relaxed)
    }
}
pub(super) fn write(
    out: &mut dyn Write,
    report: &Report,
    graph: &unityrs_scene::Graph,
    peak: u64,
    meter: &Meter,
) -> Result<()> {
    meter.count_decode(
        (report.issues.len() as u64
            + report.collection_issues.len() as u64
            + graph.issues().len() as u64)
            .saturating_mul(512)
            .saturating_add(2048),
    )?;
    write!(
        out,
        "{{\"normal_components\":{},\"rotation_components\":{},\"joint_components\":{},\"tangent_components\":{},\"weight_components\":{},\"zero_weight_rows\":{},\"peak_charged_bytes\":{peak},\"issues\":[",
        report.normal_components,
        report.rotation_components,
        report.joint_components,
        report.tangent_components,
        report.weight_components,
        report.zero_weight_rows
    )?;
    for (i, issue) in report.issues.as_slice().iter().enumerate() {
        if i != 0 {
            out.write_all(b",")?;
        }
        out.write_all(b"{\"mesh\":")?;
        optional(out, issue.mesh)?;
        out.write_all(b",\"primitive\":")?;
        optional(out, issue.primitive)?;
        let reason = match issue.reason {
            Reason::NonFinitePosition => "non_finite_position",
            Reason::InvalidNormal => "invalid_normal",
            Reason::InvalidTangent => "invalid_tangent",
        };
        write!(
            out,
            ",\"source_mesh\":{},\"source_primitive\":{},\"reason\":\"{reason}\"}}",
            issue.source_mesh, issue.source_primitive
        )?;
    }
    out.write_all(b"],\"collection_issues\":[")?;
    for (i, issue) in report.collection_issues.as_slice().iter().enumerate() {
        if i != 0 {
            out.write_all(b",")?;
        }
        write!(out, "{{\"node\":{},\"reason\":", issue.node)?;
        string(out, issue.reason)?;
        write!(out, ",\"count\":{}}}", issue.count)?;
    }
    out.write_all(b"],\"graph_issues\":[")?;
    for (i, issue) in graph.issues().iter().enumerate() {
        if i != 0 {
            out.write_all(b",")?;
        }
        let (object, reason) = match issue {
            Issue::Unresolved { from, .. } => (from, "unresolved_pptr"),
            Issue::Builtin { from, .. } => (from, "builtin"),
            Issue::Malformed { object, .. } => (object, "malformed"),
            Issue::Cycle { transform, .. } => (transform, "cycle"),
            Issue::Depth { transform, .. } => (transform, "depth"),
            Issue::ParentMismatch { transform, .. } => (transform, "parent_mismatch"),
        };
        write!(
            out,
            "{{\"path_id\":{},\"reason\":\"{reason}\",\"count\":1}}",
            object.path_id()
        )?;
    }
    out.write_all(b"]}\n")?;
    Ok(())
}
fn optional(out: &mut dyn Write, value: Option<usize>) -> std::io::Result<()> {
    match value {
        Some(v) => write!(out, "{v}"),
        None => out.write_all(b"null"),
    }
}

fn string(out: &mut dyn Write, value: &str) -> std::io::Result<()> {
    out.write_all(b"\"")?;
    for c in value.chars() {
        match c {
            '"' => out.write_all(b"\\\"")?,
            '\\' => out.write_all(b"\\\\")?,
            '\u{00}'..='\u{1f}' => write!(out, "\\u{:04x}", u32::from(c))?,
            _ => {
                let mut bytes = [0; 4];
                out.write_all(c.encode_utf8(&mut bytes).as_bytes())?;
            }
        }
    }
    out.write_all(b"\"")
}
