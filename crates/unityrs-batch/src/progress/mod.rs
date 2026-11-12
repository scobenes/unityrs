//! The progress stream's lines (`docs/BATCH_SPEC.md` section 10): one JSON
//! object a line, every field on every line, in the stream's format
//! version [`P`].
//!
//! - [`Snapshot`]: the fields every line has; [`Snapshot::line`] renders a
//!   `snapshot` line or, given an [`Exit`], the `exit` line, which names
//!   the status, the code, the reason that set it and its detail.
//! - [`clock`]: `ms`, the time since the process started on the boot
//!   clock.
//! - [`early`]: how the main thread writes its own `exit` line before the
//!   progress thread exists (`docs/plans/batch.md` 2.11, *The main
//!   thread's `exit` line*): phase 1's 78s, `--check`'s line, and the
//!   69s of later PRs.
//!
//! The progress thread, its mailbox and its snapshots are later PRs'.

pub mod clock;
pub mod early;

use std::fmt::Write as _;

use crate::reason::Code;
use crate::spec::{MAX_DETAIL_BYTES, escaped_len};

/// The stream's format version, `p` (section 10).
pub const P: u64 = 1;

/// `phase`: `docs/plans/batch.md` 2.1's phases 1 to 8.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// 1, validation.
    Validate,
    /// 2.
    Preflight,
    /// 3, the spool's start.
    Start,
    /// 4.
    Stage,
    /// 5.
    Extract,
    /// 6.
    Winners,
    /// 7.
    Publish,
    /// 8, the finisher.
    Finish,
}

impl Phase {
    /// Its name on a line.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Validate => "validate",
            Self::Preflight => "preflight",
            Self::Start => "start",
            Self::Stage => "stage",
            Self::Extract => "extract",
            Self::Winners => "winners",
            Self::Publish => "publish",
            Self::Finish => "finish",
        }
    }
}

/// `inputs`, which partition `total` at every line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    pub total: u64,
    pub complete: u64,
    pub failed: u64,
    pub running: u64,
    pub queued: u64,
    pub not_run: u64,
}

/// `transfers`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Transfers {
    pub stages_active: u64,
    pub stages_done: u64,
    pub stages_failed: u64,
    pub uploads_active: u64,
    pub uploads_done: u64,
    pub uploads_failed: u64,
}

/// `memory`, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    /// `I`.
    pub inflight_limit: u64,
    pub slots_reserved: u64,
    pub attempts_used: u64,
    pub phase_used: u64,
    pub rss: u64,
}

/// `spool`, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Spool {
    pub used: u64,
    pub size: u64,
}

/// `outputs`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outputs {
    pub candidates: u64,
    pub winners: u64,
    pub published: u64,
    pub present: u64,
    pub failed: u64,
}

/// What every line holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// 0 on the first line, then +1 a line.
    pub seq: u64,
    /// Milliseconds since the process started.
    pub ms: u64,
    pub phase: Phase,
    pub inputs: Inputs,
    pub transfers: Transfers,
    pub memory: Memory,
    pub spool: Spool,
    pub outputs: Outputs,
}

/// What only the `exit` line holds non-null.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exit {
    /// The status of the attempt's last head write the store acknowledged.
    pub status: Option<&'static str>,
    /// The exit code.
    pub code: Code,
    /// The reason that set the code, where section 10 names one.
    pub reason: Option<String>,
    /// Its detail: cut to [`MAX_DETAIL_BYTES`] as written when rendered.
    pub detail: Option<String>,
}

impl Snapshot {
    /// The first line of a process that ends in phase 1: `seq` 0, every
    /// count 0.
    #[must_use]
    pub fn validate(ms: u64) -> Self {
        Self {
            seq: 0,
            ms,
            phase: Phase::Validate,
            inputs: Inputs::default(),
            transfers: Transfers::default(),
            memory: Memory::default(),
            spool: Spool::default(),
            outputs: Outputs::default(),
        }
    }

    /// The line, `\n` included: an `exit` line with `exit`, else a
    /// `snapshot` line (its four last fields null).
    #[allow(clippy::many_single_char_names)]
    #[must_use]
    pub fn line(&self, exit: Option<&Exit>) -> String {
        let mut s = String::with_capacity(512);
        let kind = if exit.is_some() { "exit" } else { "snapshot" };
        let i = &self.inputs;
        let t = &self.transfers;
        let m = &self.memory;
        let o = &self.outputs;
        let _ = write!(
            s,
            "{{\"p\":{P},\"type\":\"{kind}\",\"seq\":{},\"ms\":{},\"phase\":\"{}\",\
             \"inputs\":{{\"total\":{},\"complete\":{},\"failed\":{},\"running\":{},\"queued\":{},\"not_run\":{}}},\
             \"transfers\":{{\"stages_active\":{},\"stages_done\":{},\"stages_failed\":{},\
             \"uploads_active\":{},\"uploads_done\":{},\"uploads_failed\":{}}},\
             \"memory\":{{\"inflight_limit\":{},\"slots_reserved\":{},\"attempts_used\":{},\"phase_used\":{},\"rss\":{}}},\
             \"spool\":{{\"used\":{},\"size\":{}}},\
             \"outputs\":{{\"candidates\":{},\"winners\":{},\"published\":{},\"present\":{},\"failed\":{}}},",
            self.seq,
            self.ms,
            self.phase.name(),
            i.total,
            i.complete,
            i.failed,
            i.running,
            i.queued,
            i.not_run,
            t.stages_active,
            t.stages_done,
            t.stages_failed,
            t.uploads_active,
            t.uploads_done,
            t.uploads_failed,
            m.inflight_limit,
            m.slots_reserved,
            m.attempts_used,
            m.phase_used,
            m.rss,
            self.spool.used,
            self.spool.size,
            o.candidates,
            o.winners,
            o.published,
            o.present,
            o.failed,
        );
        let status = exit.and_then(|e| e.status);
        let code = exit.map(|e| e.code.value());
        let reason = exit.and_then(|e| e.reason.as_deref());
        let detail = exit.and_then(|e| e.detail.as_deref()).map(fit_detail);
        s.push_str("\"status\":");
        push_opt_str(&mut s, status);
        s.push_str(",\"code\":");
        match code {
            Some(c) => {
                let _ = write!(s, "{c}");
            }
            None => s.push_str("null"),
        }
        s.push_str(",\"reason\":");
        push_opt_str(&mut s, reason);
        s.push_str(",\"detail\":");
        push_opt_str(&mut s, detail);
        s.push_str("}\n");
        s
    }
}

fn push_opt_str(s: &mut String, v: Option<&str>) {
    match v {
        Some(v) => s.push_str(&serde_json::Value::from(v).to_string()),
        None => s.push_str("null"),
    }
}

/// `detail` cut at a code point boundary so that its escaped form is at
/// most [`MAX_DETAIL_BYTES`] (section 10). A `bad_spec` detail arrives
/// already cut by its own rule ([`crate::spec::BadSpec::detail`]).
fn fit_detail(detail: &str) -> &str {
    let mut used = 0;
    for (i, c) in detail.char_indices() {
        used += escaped_len(c);
        if used > MAX_DETAIL_BYTES {
            return detail.get(..i).unwrap_or_default();
        }
    }
    detail
}

/// This process's resident set, in bytes: `VmRSS` of `/proc/self/status`
/// (Linux); 0 where it cannot be read.
#[must_use]
pub fn rss() -> u64 {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.trim().strip_suffix("kB"))
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(0, |kb| kb.saturating_mul(1024))
}

#[cfg(all(test, target_os = "linux"))]
mod early_tests;
#[cfg(test)]
mod tests;
