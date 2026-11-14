use crate::spec::model::DateTime;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Milliseconds on `CLOCK_BOOTTIME`, never an Instant fallback. All conversion
/// rounds toward the past, at most one millisecond early, never late. MAX is
/// reserved for the unarmed atomic representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Boot(pub u64);
impl Boot {
    pub fn now() -> Result<Self, TimingError> {
        Self::from_duration(crate::sys::boottime().ok_or(TimingError::Clock)?)
    }
    pub fn from_duration(value: Duration) -> Result<Self, TimingError> {
        u64::try_from(value.as_millis())
            .ok()
            .filter(|n| *n < u64::MAX)
            .map(Self)
            .ok_or(TimingError::Overflow)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingError {
    Clock,
    Overflow,
    DeadlineTooClose,
    Grace,
    Date,
}

#[derive(Clone, Copy, Debug)]
pub struct Policy {
    pub deadline: Option<Boot>,
    pub cutoff: Option<Boot>,
    grace_ms: u64,
}
impl Policy {
    /// Production samples the wall and boot clocks once, before services start.
    pub fn new(deadline: Option<&DateTime>, grace_ms: u64) -> Result<Self, TimingError> {
        // Sample boot first: the sampling interval can only move the mapped
        // deadline earlier. Reversing these samples could make it later.
        let boot = crate::sys::boottime().ok_or(TimingError::Clock)?;
        let wall = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => i128::try_from(d.as_nanos()).map_err(|_| TimingError::Overflow)?,
            Err(e) => {
                -i128::try_from(e.duration().as_nanos()).map_err(|_| TimingError::Overflow)?
            }
        };
        Self::at(deadline, grace_ms, wall, boot)
    }
    /// Injectable samples. Fractions beyond nine digits were truncated by
    /// validation. Accepted leap second 60 maps to the following POSIX second;
    /// offsets are applied before the one checked wall-to-boot conversion.
    /// Year 0000 and 9999 remain signed until cutoff checking, never wrapping.
    pub fn at(
        deadline: Option<&DateTime>,
        grace_ms: u64,
        wall_ns: i128,
        boot: Duration,
    ) -> Result<Self, TimingError> {
        if !(10_000..=600_000).contains(&grace_ms) {
            return Err(TimingError::Grace);
        }
        let now = Boot::from_duration(boot)?;
        let deadline = deadline
            .map(|d| {
                let delta = unix_ns(d)?
                    .checked_sub(wall_ns)
                    .ok_or(TimingError::Overflow)?;
                let n = i128::try_from(boot.as_nanos())
                    .map_err(|_| TimingError::Overflow)?
                    .checked_add(delta)
                    .ok_or(TimingError::Overflow)?
                    .div_euclid(1_000_000);
                if n <= i128::from(now.0) + i128::from(grace_ms) {
                    return Err(TimingError::DeadlineTooClose);
                }
                let n = u64::try_from(n).map_err(|_| TimingError::Overflow)?;
                if n == u64::MAX {
                    return Err(TimingError::Overflow);
                }
                Ok(Boot(n))
            })
            .transpose()?;
        Ok(Self {
            deadline,
            cutoff: deadline.map(|d| Boot(d.0 - grace_ms)),
            grace_ms,
        })
    }
    pub fn hard(self, now: Boot) -> Result<Boot, TimingError> {
        let end = now
            .0
            .checked_add(self.grace_ms)
            .ok_or(TimingError::Overflow)?;
        Ok(Boot(
            self.deadline
                .map_or(end, |d| end.min(d.0))
                .saturating_sub(500),
        ))
    }
    /// Preserve 3 s for an ordinary head or 5 s for running-write reconciliation.
    #[must_use]
    pub fn non_head_cut(hard: Boot, reconcile: bool) -> Boot {
        Boot(hard.0.saturating_sub(if reconcile { 5_000 } else { 3_000 }))
    }
    #[must_use]
    pub fn head_cut(hard: Boot) -> Boot {
        Boot(hard.0.saturating_sub(2_000))
    }
    #[must_use]
    pub fn abort_ms(hard: Boot, now: Boot, reconcile: bool) -> Option<u64> {
        let left = Self::non_head_cut(hard, reconcile)
            .0
            .saturating_sub(now.0)
            .min(5_000);
        (left >= 500).then_some(left)
    }
}
fn unix_ns(d: &DateTime) -> Result<i128, TimingError> {
    let leap =
        d.year.is_multiple_of(4) && (!d.year.is_multiple_of(100) || d.year.is_multiple_of(400));
    let months = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let m = usize::try_from(d.month.saturating_sub(1)).map_err(|_| TimingError::Date)?;
    if d.year > 9999
        || d.month == 0
        || !months.get(m).is_some_and(|n| (1..=*n).contains(&d.day))
        || d.hour > 23
        || d.minute > 59
        || d.second > 60
        || d.nanos >= 1_000_000_000
        || d.offset_minutes.unsigned_abs() > 1439
        || (d.second == 60
            && (i64::from(d.hour) * 60 + i64::from(d.minute) - i64::from(d.offset_minutes))
                .rem_euclid(1440)
                != 1439)
    {
        return Err(TimingError::Date);
    }
    let y = i128::from(d.year);
    // Gregorian days before January 1, including the leap year 0000.
    let mut days = 365 * y + (y + 3) / 4 - (y + 99) / 100 + (y + 399) / 400 - 719_528;
    for n in months.iter().take(m) {
        days += i128::from(*n);
    }
    days += i128::from(d.day) - 1;
    let seconds = days * 86_400
        + i128::from(d.hour) * 3600
        + i128::from(d.minute) * 60
        + i128::from(d.second)
        - i128::from(d.offset_minutes) * 60;
    Ok(seconds * 1_000_000_000 + i128::from(d.nanos))
}

#[cfg(test)]
mod tests;
