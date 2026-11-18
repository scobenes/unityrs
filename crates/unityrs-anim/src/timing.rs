use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};

use crate::AnimError;

/// Closed interval in source seconds, with the clip-level serialized rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipTiming {
    start: f64,
    stop: f64,
    rate: f64,
}

impl ClipTiming {
    pub(crate) fn new(start: f64, stop: f64, rate: f64) -> Result<Self, AnimError> {
        if !start.is_finite() || !stop.is_finite() || start > stop || !(stop - start).is_finite() {
            return Err(AnimError::InvalidClipInterval);
        }
        check_rate(rate)?;
        Ok(Self { start, stop, rate })
    }

    #[must_use]
    pub fn start(self) -> f64 {
        self.start
    }

    #[must_use]
    pub fn stop(self) -> f64 {
        self.stop
    }

    #[must_use]
    pub fn rate(self) -> f64 {
        self.rate
    }

    pub(crate) fn contains(self, source: f64) -> bool {
        (self.start..=self.stop).contains(&source)
    }

    /// Generate source times, retaining the exact endpoints and filtering
    /// interior times by the rounded f32 local clock.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn schedule(
        self,
        rate_override: Option<f64>,
        meter: &Arc<Meter>,
    ) -> Result<MeteredVec<f64>, AnimError> {
        let rate = rate_override.unwrap_or(self.rate);
        check_rate(rate)?;
        let mut times = MeteredVec::new(meter);
        meter.count_decode(1)?;
        times.push(self.start)?;
        if self.start >= self.stop {
            return Ok(times);
        }
        let duration = self.stop - self.start;
        let end = duration as f32;
        // u32 keeps the iteration index exactly representable in f64 and
        // bounds even callers using an unlimited meter.
        let count = (duration * rate).ceil();
        if !end.is_finite() || end == 0. || !(0. ..=f64::from(u32::MAX)).contains(&count) {
            return Err(AnimError::InvalidSchedule);
        }
        meter.count_decode(count as u64 + 1)?;
        let mut previous = 0.;
        for i in 1..=count as u32 {
            let time = self.start + f64::from(i) / rate;
            let rounded = (time - self.start) as f32;
            if time >= self.stop || rounded >= end {
                break;
            }
            if rounded > previous {
                times.push(time)?;
                previous = rounded;
            }
        }
        times.push(self.stop)?;
        Ok(times)
    }
}

pub(crate) fn check_rate(rate: f64) -> Result<(), AnimError> {
    if rate.is_finite() && rate > 0. {
        Ok(())
    } else {
        Err(AnimError::InvalidSampleRate)
    }
}

const _: () = crate::send_sync::<ClipTiming>();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_schedule_keeps_exact_stop_when_rounded_neighbor_collides() {
        let start = -f64::from(1e-10_f32);
        let timing = ClipTiming::new(start, 1., 1.).unwrap();
        let times = timing.schedule(None, &Meter::unlimited()).unwrap();
        assert_eq!(times.as_slice(), &[start, 1.]);
    }

    #[test]
    fn an_unrepresentable_or_overflowing_schedule_refuses() {
        let meter = Meter::unlimited();
        let collapsed = ClipTiming::new(0., f64::MIN_POSITIVE, 1.).unwrap();
        let overflow = ClipTiming::new(-f64::from(f32::MAX), f64::from(f32::MAX), 1.).unwrap();
        for timing in [collapsed, overflow] {
            assert!(matches!(
                timing.schedule(None, &meter),
                Err(AnimError::InvalidSchedule)
            ));
        }
        let timing = ClipTiming::new(0., 1., 1.).unwrap();
        assert!(matches!(
            timing.schedule(Some(f64::MAX), &meter),
            Err(AnimError::InvalidSchedule)
        ));
        assert!(matches!(
            timing.schedule(Some(0.), &meter),
            Err(AnimError::InvalidSampleRate)
        ));
    }
}
