// Ported from UnityPy 1.25.2 (MIT): UnityPy/config.py. See THIRD_PARTY_LICENSES.
//! Fallback retrieval and warning filtering (`config.py:21-40`).
//! A binding shares one warning state across call sites, and supplies its warning
//! emitter. Retrieval does not validate version syntax; parsing happens later.

use crate::RawStr;
use crate::limits::{ChargeError, Meter, MeteredVec};
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;

/// UnityPy's warning category and message, borrowed without allocating.
#[derive(Debug)]
pub struct UnityVersionFallbackWarning<'a>(pub &'a RawStr);
impl UnityVersionFallbackWarning<'_> {
    /// Python warning category.
    #[must_use]
    pub fn category(&self) -> &'static str {
        "UnityVersionFallbackWarning"
    }
}
impl fmt::Display for UnityVersionFallbackWarning<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "No valid Unity version found, defaulting to UnityPy.config.FALLBACK_UNITY_VERSION (",
        )?;
        if let Some(s) = self.0.to_str() {
            f.write_str(s)?;
        } else {
            write!(f, "{}", self.0)?;
        }
        f.write_str(")")
    }
}

/// A missing string fallback or a refusal to retain the warning's once key.
#[derive(Debug, thiserror::Error)]
pub enum UnityVersionFallbackError {
    /// `None` and non-string values at the binding boundary.
    #[error(
        "No valid Unity version found, and the fallback version is not correctly configured. Please explicitly set the value of UnityPy.config.FALLBACK_UNITY_VERSION."
    )]
    Unconfigured,
    /// Warning state could not grow; never silently suppress a warning.
    #[error(transparent)]
    Charge(#[from] ChargeError),
}
impl UnityVersionFallbackError {
    /// Python exception category.
    #[must_use]
    pub fn python_kind(&self) -> &'static str {
        match self {
            Self::Unconfigured => "UnityVersionFallbackError",
            Self::Charge(_) => "MemoryError",
        }
    }
}

/// The `once` filter's keys: one per distinct fallback message, across call sites.
#[derive(Debug)]
pub struct FallbackWarnings {
    seen: MeteredVec<RawStr>,
}
impl FallbackWarnings {
    /// Warning keys charge `meter` until this state is dropped.
    #[must_use]
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            seen: MeteredVec::new(meter),
        }
    }

    fn first(&mut self, value: &RawStr) -> Result<bool, ChargeError> {
        let meter = self.seen.charge().meter();
        for key in self.seen.as_slice() {
            // One comparison step, plus an upper bound on bytes examined.
            let bytes = if key.len() == value.len() {
                key.len()
            } else {
                0
            };
            meter.count_decode(1_u64.saturating_add(u64::try_from(bytes).unwrap_or(u64::MAX)))?;
            if key == value {
                return Ok(false);
            }
        }
        meter.count_decode(u64::try_from(value.len()).unwrap_or(u64::MAX))?;
        let key = RawStr::charged(value.as_bytes(), meter)?;
        self.seen.push(key)?;
        Ok(true)
    }
}

/// Warning delivery for version resolution. A Python binding installs its
/// warning emitter here; native callers can collect or display the same warning.
pub trait FallbackWarningSink: Send + Sync + fmt::Debug {
    /// Emit the configured fallback's warning, respecting the sink's filter.
    fn warn(&self, value: &RawStr) -> Result<(), ChargeError>;
}

/// One `once` registry shared across files and call sites. The callback runs
/// after releasing the registry lock, so reentrant warning handlers are safe.
pub struct FilteredWarnings<E> {
    state: Mutex<FallbackWarnings>,
    emit: E,
}
impl<E> fmt::Debug for FilteredWarnings<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FilteredWarnings").finish_non_exhaustive()
    }
}
impl<E> FilteredWarnings<E> {
    /// Create warning state on the caller's meter without allocating.
    #[must_use]
    pub fn new(meter: &Arc<Meter>, emit: E) -> Self {
        Self {
            state: Mutex::new(FallbackWarnings::new(meter)),
            emit,
        }
    }
}
impl<E: Fn(UnityVersionFallbackWarning<'_>) + Send + Sync> FallbackWarningSink
    for FilteredWarnings<E>
{
    fn warn(&self, value: &RawStr) -> Result<(), ChargeError> {
        let first = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .first(value)?;
        if first {
            (self.emit)(UnityVersionFallbackWarning(value));
        }
        Ok(())
    }
}

/// Return any configured string and emit its warning once. `None` represents
/// any non-string config value, as UnityPy's `isinstance(value, str)` does.
pub fn get_fallback_version<'a>(
    value: Option<&'a RawStr>,
    warnings: &mut FallbackWarnings,
    mut emit: impl FnMut(UnityVersionFallbackWarning<'_>),
) -> Result<&'a RawStr, UnityVersionFallbackError> {
    let value = value.ok_or(UnityVersionFallbackError::Unconfigured)?;
    if warnings.first(value)? {
        emit(UnityVersionFallbackWarning(value));
    }
    Ok(value)
}

const _: () = {
    crate::send_sync::<UnityVersionFallbackWarning<'static>>();
    crate::send_sync::<UnityVersionFallbackError>();
    crate::send_sync::<FallbackWarnings>();
    crate::send_sync::<dyn FallbackWarningSink>();
    crate::send_sync::<FilteredWarnings<fn(UnityVersionFallbackWarning<'_>)>>();
};
