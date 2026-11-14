use super::Boot;
use crate::{
    attempt::{DetailCode, ReasonCode},
    control::{Control, Culprit},
    job::RunFailure,
    reason::{Code, StopReason},
};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering::SeqCst};

#[cfg(not(target_has_atomic = "64"))]
compile_error!("process lifetime requires lock-free 64-bit atomics");

/// Only a spool-full stop carries a detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpoolDetail {
    SizeBytes,
    Reserve,
    Enospc,
    Edquot,
}
impl SpoolDetail {
    /// Its name in a head's `stop_detail` (`BATCH_SPEC.md` 7.3).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SizeBytes => "size_bytes",
            Self::Reserve => "reserve",
            Self::Enospc => "ENOSPC",
            Self::Edquot => "EDQUOT",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stop {
    pub reason: StopReason,
    pub detail: Option<SpoolDetail>,
    pub culprit: Culprit,
}
impl Stop {
    #[must_use]
    pub const fn external(reason: StopReason) -> Self {
        Self {
            reason,
            detail: None,
            culprit: Culprit::None,
        }
    }
    /// Checked representation, independent of Rust enum layout. Invalid
    /// reason/detail pairs never enter the atomic word.
    fn pack(self) -> Option<u64> {
        let reason = match self.reason {
            StopReason::Deadline => 1,
            StopReason::Sigterm => 2,
            StopReason::Sigint => 3,
            StopReason::Sighup => 4,
            StopReason::ParentDied => 5,
            StopReason::StdinEof => 6,
            StopReason::SpoolFull => 7,
            StopReason::HostIo => 8,
            StopReason::HostMemory => 9,
            StopReason::Internal => 10,
        };
        let detail = match self.detail {
            None => 0,
            Some(SpoolDetail::SizeBytes) => 1,
            Some(SpoolDetail::Reserve) => 2,
            Some(SpoolDetail::Enospc) => 3,
            Some(SpoolDetail::Edquot) => 4,
        };
        if (reason == 7) != (detail != 0) {
            return None;
        }
        let (kind, index) = match self.culprit {
            Culprit::None => (0, 0),
            Culprit::Input(i) => (1, u64::from(i)),
            Culprit::Bundle(i) => (2, u64::from(i)),
        };
        if reason <= 6 && kind != 0 {
            return None;
        }
        Some(reason | detail << 4 | kind << 7 | index << 9)
    }
    fn unpack(word: u64) -> Option<Self> {
        let reason = match word & 15 {
            1 => StopReason::Deadline,
            2 => StopReason::Sigterm,
            3 => StopReason::Sigint,
            4 => StopReason::Sighup,
            5 => StopReason::ParentDied,
            6 => StopReason::StdinEof,
            7 => StopReason::SpoolFull,
            8 => StopReason::HostIo,
            9 => StopReason::HostMemory,
            10 => StopReason::Internal,
            _ => return None,
        };
        let detail = match (word >> 4) & 7 {
            0 => None,
            1 => Some(SpoolDetail::SizeBytes),
            2 => Some(SpoolDetail::Reserve),
            3 => Some(SpoolDetail::Enospc),
            4 => Some(SpoolDetail::Edquot),
            _ => return None,
        };
        let index = u32::try_from((word >> 9) & u64::from(u32::MAX)).ok()?;
        let culprit = match (word >> 7) & 3 {
            0 if index == 0 => Culprit::None,
            1 => Culprit::Input(index),
            2 => Culprit::Bundle(index),
            _ => return None,
        };
        let stop = Self {
            reason,
            detail,
            culprit,
        };
        (stop.pack()? == word & !TAKEN).then_some(stop)
    }
}
impl From<(RunFailure, Culprit)> for Stop {
    fn from((failure, culprit): (RunFailure, Culprit)) -> Self {
        let reason = match failure.reason {
            ReasonCode::SpoolFull => StopReason::SpoolFull,
            ReasonCode::HostIo => StopReason::HostIo,
            ReasonCode::HostMemory => StopReason::HostMemory,
            _ => StopReason::Internal,
        };
        let detail = if reason == StopReason::SpoolFull {
            match failure.detail {
                Some(DetailCode::SizeBytes) => Some(SpoolDetail::SizeBytes),
                Some(DetailCode::Reserve) => Some(SpoolDetail::Reserve),
                Some(DetailCode::Enospc) => Some(SpoolDetail::Enospc),
                Some(DetailCode::Edquot) => Some(SpoolDetail::Edquot),
                _ => return Self::external(StopReason::Internal),
            }
        } else {
            None
        };
        Self {
            reason,
            detail,
            culprit,
        }
    }
}
const TAKEN: u64 = 1 << 41;
/// Canonical live stop and immutable take are distinct: later requests keep
/// reducing this word, including after take, but cannot change its returned copy.
pub struct State {
    word: AtomicU64,
    hard: AtomicU64,
    final_code: AtomicU8,
    signals: AtomicU8,
}
impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}
impl State {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            word: AtomicU64::new(0),
            hard: AtomicU64::new(u64::MAX),
            final_code: AtomicU8::new(255),
            signals: AtomicU8::new(0),
        }
    }
    /// Invalid typed combinations become internal, rather than losing a stop.
    /// The process control wrapper writes its self-pipe after this publication.
    pub fn request(&self, stop: Stop) {
        let stop = if stop.pack().is_some() {
            stop
        } else {
            Stop::external(StopReason::Internal)
        };
        let Some(packed) = stop.pack() else { return };
        let mut old = self.word.load(SeqCst);
        loop {
            if Stop::unpack(old)
                .is_some_and(|s| s.reason.code().severity() <= stop.reason.code().severity())
            {
                return;
            }
            match self
                .word
                .compare_exchange(old, packed | old & TAKEN, SeqCst, SeqCst)
            {
                Ok(_) => return,
                Err(next) => old = next,
            }
        }
    }
    #[must_use]
    pub fn live(&self) -> Option<Stop> {
        Stop::unpack(self.word.load(SeqCst))
    }
    /// Exactly one caller captures a stop (or a normal-end absence).
    pub fn take(&self) -> Option<Stop> {
        let old = self.word.fetch_or(TAKEN, SeqCst);
        if old & TAKEN != 0 {
            None
        } else {
            Stop::unpack(old)
        }
    }
    /// Set once, never extended by a later wake. Zero means already due.
    pub fn arm(&self, when: Boot) -> Boot {
        match self.hard.compare_exchange(u64::MAX, when.0, SeqCst, SeqCst) {
            Ok(_) => when,
            Err(v) => Boot(v),
        }
    }
    #[must_use]
    pub fn hard(&self) -> Option<Boot> {
        let n = self.hard.load(SeqCst);
        (n != u64::MAX).then_some(Boot(n))
    }
    pub fn store_final(&self, code: Code) {
        self.final_code.store(code.value(), SeqCst);
    }
    #[must_use]
    pub fn end_code(&self, own: Option<StopReason>) -> Code {
        if let Some(code) = Code::from_value(self.final_code.load(SeqCst)) {
            return code;
        }
        let code = self.live().map_or(Code::Complete, |s| s.reason.code());
        own.map_or(code, |r| {
            if r.code().severity() < code.severity() {
                r.code()
            } else {
                code
            }
        })
    }
    /// The handled stop signals counted so far (saturating). An observation
    /// only: [`Self::second_signal`] alone decides.
    #[must_use]
    pub fn handled_signals(&self) -> u8 {
        self.signals.load(SeqCst)
    }
    /// Only handled stop signals count; parent death does not use this method.
    pub fn second_signal(&self) -> bool {
        self.signals
            .try_update(SeqCst, SeqCst, |count| Some(count.saturating_add(1)))
            .is_ok_and(|previous| previous != 0)
    }
}
impl Control for State {
    fn stopped(&self) -> bool {
        self.live().is_some()
    }
    fn request_stop(&self, failure: RunFailure, culprit: Culprit) {
        self.request((failure, culprit).into());
    }
}

#[cfg(test)]
mod tests;
