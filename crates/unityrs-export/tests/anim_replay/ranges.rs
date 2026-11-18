//! Audit the recorded budget path and envelopes against actual source segments.
use crate::{J, Meter, Result, Target, list, number, require};
use unityrs_anim::{BindingTarget, BoundTrack, ClipTiming, ScalarSegment, TransformProperty};

impl<'c, 'r> Target<'_, 'c, 'r> {
    pub(super) fn track(&self, axis: usize) -> Option<(&BoundTrack<'c, 'r>, usize)> {
        self.tracks.iter().find_map(|&track| match track.component {
            Some(component) if component == axis => Some((track, 0)),
            None if axis < track.width() => Some((track, axis)),
            _ => None,
        })
    }

    pub(super) fn rotation_at(
        &self,
        time: f64,
        left: bool,
        meter: &Meter,
        joint: bool,
    ) -> Result<[f64; 4]> {
        let Some(track) = self.tracks.first() else {
            return Ok(self.rest.as_slice().try_into()?);
        };
        let raw = track.raw_at(time, left, meter)?;
        if joint {
            Ok(track.rotation(raw, meter)?)
        } else {
            Ok(raw)
        }
    }

    pub(super) fn component(
        &self,
        axis: usize,
        source: f64,
        left: bool,
        meter: &Meter,
    ) -> Result<f64> {
        let Some((track, component)) = self.track(axis) else {
            return self
                .rest
                .get(axis)
                .copied()
                .ok_or_else(|| "missing rest component".into());
        };
        Ok(track.raw_at(source, left, meter)?[component])
    }
}

struct Stage<'a> {
    name: &'a str,
    ranges: &'a [J],
}

impl Stage<'_> {
    fn range(&self, i: usize, lo: f64, hi: f64) -> Result<()> {
        require(inside(&self.ranges[i], lo, hi)?, "bundle stage range")
    }

    fn point(&self, i: usize, value: f64) -> Result<()> {
        self.range(i, value, value)
    }

    fn clamp(&self, value: f64) -> Result<()> {
        match self.name {
            "streamed" => self.point(3, value),
            // A clamp uses a single key value, with no interpolation.
            "dense" | "hermite" | "weighted" => require(
                inside(&self.ranges[0], value, value)? || inside(&self.ranges[1], value, value)?,
                "bundle stage range",
            ),
            _ => self.point(0, value),
        }
    }

    fn segment(&self, segment: ScalarSegment, lo: f64, hi: f64) -> Result<()> {
        use ScalarSegment as S;
        match segment {
            S::Constant(value) | S::Held(value) => self.clamp(value),
            S::Streamed { key, coefficients } => {
                for (i, value) in coefficients.into_iter().enumerate() {
                    self.point(i, value)?;
                }
                self.range(4, lo - key, hi - key)
            }
            S::Dense {
                values: [a, b],
                rate,
                begin,
                offset,
                ..
            } => {
                for (i, value) in [a, b, rate].into_iter().enumerate() {
                    self.point(i, value)?;
                }
                self.range(3, lo, hi)?;
                self.point(4, begin)?;
                self.point(5, offset)
            }
            S::Hermite {
                values: [a, b],
                slopes: [outgoing, incoming],
                duration,
                ..
            }
            | S::Weighted {
                values: [a, b],
                slopes: [outgoing, incoming],
                duration,
                ..
            } => {
                for (i, value) in [a, b, outgoing, incoming, duration].into_iter().enumerate() {
                    self.point(i, value)?;
                }
                if let S::Weighted {
                    weights: [outgoing, incoming],
                    ..
                } = segment
                {
                    self.point(5, outgoing)?;
                    self.point(6, incoming)?;
                }
                Ok(())
            }
        }
    }
}

fn inside(range: &J, lo: f64, hi: f64) -> Result<bool> {
    Ok(number(&range[0])? <= lo && hi <= number(&range[1])?)
}

fn stage_name(segment: ScalarSegment) -> Option<&'static str> {
    use ScalarSegment as S;
    match segment {
        S::Constant(_) => None,
        S::Held(_) => Some("held"),
        S::Streamed { .. } => Some("streamed"),
        S::Dense { .. } => Some("dense"),
        S::Hermite { .. } => Some("hermite"),
        S::Weighted { .. } => Some("weighted"),
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) fn check(target: &Target<'_, '_, '_>, timing: ClipTiming, meter: &Meter) -> Result<()> {
    let mut times = vec![timing.start(), timing.stop()];
    for track in &target.tracks {
        track.visit_key_times(|time| {
            meter.count_decode(1)?;
            if timing.start() < time && time < timing.stop() {
                times.push(time);
            }
            Ok(())
        })?;
    }
    times.sort_unstable_by(f64::total_cmp);
    times.dedup();
    let joint = target.tracks.iter().any(|track| {
        matches!(
            track.target,
            BindingTarget::Transform {
                property: TransformProperty::Euler,
                ..
            }
        )
    });
    let mut name = None;
    for pair in times.windows(2) {
        let mid = center(pair[0], pair[1]);
        for axis in 0..target.rest.len() {
            if let Some((track, component)) = target.track(axis)
                && let Some(stage) = stage_name(track.segment(component, mid, false, meter)?)
            {
                // A held segment of a streamed track is a cubic with zero
                // slopes, which the streamed stage bounds.
                let stage = match (name, stage) {
                    (Some("streamed"), "held") | (Some("held"), "streamed") => "streamed",
                    _ => stage,
                };
                require(
                    name.is_none_or(|old| old == stage || (old, stage) == ("held", "streamed")),
                    "bundle mixed-stage track kind",
                )?;
                name = Some(stage);
            }
        }
    }
    let name = name.unwrap_or(if target.tracks.is_empty() {
        "rest"
    } else {
        "constant"
    });
    let kind = if joint {
        format!("euler_{name}")
    } else {
        name.to_owned()
    };
    require(target.budget["track_kind"] == kind, "bundle track kind")?;
    require(
        target.channel.is_none() || target.budget["weight_units"] == "editor",
        "bundle weight stage units",
    )?;
    let stages = list(&target.budget["stages"])?;
    require(
        stages.len() == 1 + usize::from(joint)
            && stages[0]["stage"] == name
            && (!joint || stages[1]["stage"] == "joint"),
        "bundle track stage path",
    )?;
    let stage = Stage {
        name,
        ranges: list(&stages[0]["range"])?,
    };
    // A vector row may give each component its own ranges, inside the
    // row's envelope; each component's values must then lie in its own.
    let components = match target.budget.get("component_ranges") {
        Some(rows) => {
            let rows = list(rows)?;
            require(
                rows.len() == target.rest.len(),
                "bundle component range coverage",
            )?;
            rows.iter()
                .map(|ranges| {
                    Ok(Stage {
                        name,
                        ranges: list(ranges)?,
                    })
                })
                .collect::<Result<Vec<_>>>()?
        }
        None => Vec::new(),
    };
    let stage_for = |axis: usize| components.get(axis).unwrap_or(&stage);
    for (index, &time) in times.iter().enumerate() {
        for axis in 0..target.rest.len() {
            if let Some((track, component)) = target.track(axis) {
                let segment = track.segment(component, time, false, meter)?;
                if segment.is_held() {
                    stage.segment(segment, time, time)?;
                    stage_for(axis).segment(segment, time, time)?;
                }
            } else {
                stage.clamp(target.rest[axis])?;
                stage_for(axis).clamp(target.rest[axis])?;
            }
        }
        if target.property() == "rotation" {
            audit_rotation(target, stages, joint, time, time, meter, 0)?;
        }
        if let Some(&stop) = times.get(index + 1) {
            for axis in 0..target.rest.len() {
                if let Some((track, component)) = target.track(axis) {
                    let segment = track.segment(component, center(time, stop), false, meter)?;
                    stage.segment(segment, time, stop)?;
                    stage_for(axis).segment(segment, time, stop)?;
                }
            }
            if target.property() == "rotation" {
                audit_rotation(target, stages, joint, time, stop, meter, 0)?;
            }
        }
    }
    Ok(())
}

fn center(lo: f64, hi: f64) -> f64 {
    lo + (hi - lo) * 0.5
}

/// Each subinterval must fit in one declared box. The second derivative
/// bound expands the endpoint chord to cover changes between observations.
#[allow(clippy::too_many_arguments)]
fn audit_rotation(
    target: &Target<'_, '_, '_>,
    stages: &[J],
    joint: bool,
    lo: f64,
    hi: f64,
    meter: &Meter,
    depth: u32,
) -> Result<()> {
    let last = stages.last().ok_or("missing rotation stage")?;
    let a = target.rotation_at(lo, false, meter, joint)?;
    let b = target.rotation_at(hi, hi > lo, meter, joint)?;
    let mut expansions = [0.; 4];
    if let Some(track) = target.tracks.first() {
        let mut speed = 0.;
        let mut curvature = 0.;
        for (axis, expansion) in expansions.iter_mut().enumerate().take(track.width()) {
            let (rate, acceleration) = if hi > lo {
                track
                    .segment(axis, center(lo, hi), false, meter)?
                    .derivatives(lo, hi)
            } else {
                (0., 0.)
            };
            *expansion = ((hi - lo).powi(2) * acceleration / 8.).min((hi - lo) * rate * 0.5);
            speed += rate;
            curvature += acceleration;
            if joint {
                let a = track.raw_at(lo, false, meter)?[axis];
                let b = track.raw_at(hi, hi > lo, meter)?[axis];
                if !inside(
                    &last["range"][0],
                    a.min(b) - *expansion,
                    a.max(b) + *expansion,
                )? {
                    require(
                        inside(&last["range"][0], a.min(b), a.max(b))?,
                        "bundle Euler stage range",
                    )?;
                    let middle = center(lo, hi);
                    require(
                        depth < 20 && lo < middle && middle < hi,
                        "bundle Euler stage range",
                    )?;
                    audit_rotation(target, stages, joint, lo, middle, meter, depth + 1)?;
                    return audit_rotation(target, stages, joint, middle, hi, meter, depth + 1);
                }
            }
        }
        if joint {
            let gain = std::f64::consts::PI / 360.;
            let speed = speed * gain;
            let acceleration = curvature * gain + speed.powi(2);
            expansions.fill(((hi - lo).powi(2) * acceleration / 8.).min((hi - lo) * speed * 0.5));
        }
    }
    let boxes = list(&last["norm_ranges"])?;
    for range in boxes {
        let mut fits = true;
        for (axis, ((&a, &b), &expand)) in a.iter().zip(&b).zip(&expansions).enumerate() {
            fits &= inside(&range[axis], a.min(b) - expand, a.max(b) + expand)?;
        }
        if fits {
            return Ok(());
        }
    }
    let middle = center(lo, hi);
    require(
        depth < 20 && lo < middle && middle < hi,
        "bundle rotation output boxes",
    )?;
    audit_rotation(target, stages, joint, lo, middle, meter, depth + 1)?;
    audit_rotation(target, stages, joint, middle, hi, meter, depth + 1)
}
