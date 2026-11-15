//! Stream reports to bounded sinks; reasons are fixed ASCII, never source text.
use super::Result;
use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
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
impl Home {
    fn charge(&self, bytes: u64, what: Reservation) -> std::result::Result<(), BudgetExceeded> {
        let old = self
            .live
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
                live.checked_add(bytes)
            })
            .map_err(|_| BudgetExceeded {
                requested: bytes,
                what,
            })?;
        self.peak.fetch_max(old + bytes, Ordering::Relaxed);
        Ok(())
    }
}
#[derive(Debug)]
struct Phase {
    charges: Home,
    total: Arc<Home>,
}
impl PermitHome for Phase {
    fn release(&self, bytes: u64) {
        self.charges.live.fetch_sub(bytes, Ordering::Relaxed);
        self.total.live.fetch_sub(bytes, Ordering::Relaxed);
    }
}
#[derive(Debug)]
pub(super) struct Peak {
    home: Arc<Home>,
    input: Arc<Phase>,
    export: Arc<Phase>,
    exporting: AtomicBool,
}
impl Default for Peak {
    fn default() -> Self {
        let home = Arc::new(Home::default());
        Self {
            input: Arc::new(Phase {
                charges: Home::default(),
                total: home.clone(),
            }),
            export: Arc::new(Phase {
                charges: Home::default(),
                total: home.clone(),
            }),
            home,
            exporting: AtomicBool::new(false),
        }
    }
}
impl MemoryBudget for Peak {
    fn try_reserve(
        &self,
        bytes: u64,
        what: Reservation,
    ) -> std::result::Result<Permit, BudgetExceeded> {
        let phase = if self.exporting.load(Ordering::Relaxed) {
            &self.export
        } else {
            &self.input
        };
        self.home.charge(bytes, what)?;
        // Phase-live is a subset of total-live, which was checked above. Release
        // returns the phase charge first, preserving that bound during updates.
        let old = phase.charges.live.fetch_add(bytes, Ordering::Relaxed);
        phase.charges.peak.fetch_max(old + bytes, Ordering::Relaxed);
        // Permit keeps both phase homes inline when a retained charge grows
        // across the boundary; absorbing this permit cannot allocate.
        Ok(Permit::new(bytes, phase.clone()))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Peaks {
    total: u64,
    input: u64,
    export: u64,
}
impl Peak {
    pub fn peak(&self) -> u64 {
        self.home.peak.load(Ordering::Relaxed)
    }
    /// Input loading and unit selection precede this boundary. Profile parsing,
    /// graph construction, collection and writing charge export permits. Input
    /// permits retained across the boundary stay in the input phase until drop.
    pub fn start_export(&self) {
        self.exporting.store(true, Ordering::Relaxed);
    }
    pub fn peaks(&self) -> Peaks {
        Peaks {
            total: self.peak(),
            input: self.input.charges.peak.load(Ordering::Relaxed),
            export: self.export.charges.peak.load(Ordering::Relaxed),
        }
    }
}
pub(super) fn write(
    out: &mut dyn Write,
    report: &Report,
    graph: &unityrs_scene::Graph,
    peaks: Peaks,
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
        "{{\"normal_components\":{},\"rotation_components\":{},\"joint_components\":{},\"tangent_components\":{},\"weight_components\":{},\"zero_weight_rows\":{},\"color_components\":{},\"dropped_color_attributes\":{},\"dropped_texcoord_attributes\":{},\"dropped_texture_references\":{},\"peak_charged_bytes\":{peak},\"input_peak_charged_bytes\":{input_peak},\"export_peak_charged_bytes\":{export_peak},\"issues\":[",
        report.normal_components,
        report.rotation_components,
        report.joint_components,
        report.tangent_components,
        report.weight_components,
        report.zero_weight_rows,
        report.color_components,
        report.dropped_color_attributes,
        report.dropped_texcoord_attributes,
        report.dropped_texture_references,
        peak = peaks.total,
        input_peak = peaks.input,
        export_peak = peaks.export,
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
            Reason::InvalidColor => "invalid_color",
            Reason::InvalidTexCoord => "invalid_texcoord",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_charge_growth_during_export_allocates_no_bookkeeping() {
        let budget = Arc::new(Peak::default());
        let meter = Meter::new(1000, budget.clone(), Reservation::Input);
        let mut retained = meter.charge(100).unwrap();
        budget.start_export();
        let (grown, measured) = unityrs_batch::alloc::measured(|| retained.grow(30));
        grown.unwrap();
        assert_eq!(measured.peak, 0, "phase bookkeeping must not allocate");
        assert_eq!(
            budget.peaks(),
            Peaks {
                total: 130,
                input: 100,
                export: 30
            }
        );
        retained.release(20);
        let exported = meter.charge(40).unwrap();
        assert_eq!(
            budget.peaks(),
            Peaks {
                total: 150,
                input: 100,
                export: 50
            }
        );
        drop(retained);
        assert_eq!(budget.input.charges.live.load(Ordering::Relaxed), 0);
        assert_eq!(budget.export.charges.live.load(Ordering::Relaxed), 40);
        drop(exported);
        assert_eq!(budget.home.live.load(Ordering::Relaxed), 0);

        // A fresh retained charge must also cross the boundary safely while
        // the allocator refuses its next request (including bookkeeping).
        let budget = Arc::new(Peak::default());
        let meter = Meter::new(1000, budget.clone(), Reservation::Input);
        let mut retained = meter.charge(100).unwrap();
        budget.start_export();
        crate::test_alloc::REFUSE.with(|r| r.set(true));
        let grown = retained.grow(30);
        let still_armed = crate::test_alloc::REFUSE.with(|r| r.replace(false));
        grown.unwrap();
        assert!(still_armed, "growth must not call the allocator");
        assert_eq!(
            budget.peaks(),
            Peaks {
                total: 130,
                input: 100,
                export: 30
            }
        );
    }

    #[test]
    fn export_peak_excludes_retained_inputs_and_released_input_spikes() {
        let budget = Peak::default();
        let retained = budget.try_reserve(100, Reservation::Input).unwrap();
        let transient = budget.try_reserve(900, Reservation::Input).unwrap();
        drop(transient);
        budget.start_export();
        let exported = budget.try_reserve(30, Reservation::Input).unwrap();
        assert_eq!(
            budget.peaks(),
            Peaks {
                total: 1000,
                input: 1000,
                export: 30
            }
        );
        drop(retained);
        let more = budget.try_reserve(40, Reservation::Dependency).unwrap();
        assert_eq!(
            budget.peaks(),
            Peaks {
                total: 1000,
                input: 1000,
                export: 70
            }
        );
        drop(exported);
        drop(more);
    }

    #[test]
    fn export_peak_tracks_simultaneously_live_new_charges() {
        let budget = Peak::default();
        let retained = budget.try_reserve(100, Reservation::Input).unwrap();
        budget.start_export();
        let first = budget.try_reserve(30, Reservation::Provider).unwrap();
        drop(first);
        let second = budget.try_reserve(40, Reservation::Input).unwrap();
        assert_eq!(
            budget.peaks(),
            Peaks {
                total: 140,
                input: 100,
                export: 40
            }
        );
        drop(second);
        drop(retained);
    }
}
