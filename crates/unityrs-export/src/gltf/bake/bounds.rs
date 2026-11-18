//! Chord-error and timestamp bounds from `docs/anim_reference.md`.
use super::{AnimationTarget, Channel, Key, MAX_COMPONENT, Result, midpoint, round};
use unityrs_anim::BindingTarget;
use unityrs_anim::TransformProperty::Euler;

const FLOOR: f64 = 1. / 1_073_741_824.;
const ULPS: f64 = 0.500_001;

fn ulp(value: f64) -> f64 {
    let biased = i32::try_from((value.to_bits() >> 52) & 0x7ff).unwrap_or_default();
    2f64.powi((biased - 1023 - 23).max(-149))
}

fn clock(channel: &Channel<'_, '_, '_>, left: Key, right: Key) -> (f64, f64, f64) {
    let a = left.source - channel.start;
    let b = right.source - channel.start;
    let spacing = 0.5 * ulp(b.max(f64::from(right.output)));
    let displacement = (f64::from(left.output) - a)
        .abs()
        .max((f64::from(right.output) - b).abs());
    (
        right.source - left.source,
        spacing,
        (displacement - spacing).max(0.),
    )
}

pub(super) fn components(channel: &Channel<'_, '_, '_>, left: Key, right: Key) -> Result<bool> {
    let approximation = if channel.target == AnimationTarget::Translation {
        1e-4
    } else {
        1e-5
    };
    let (duration, time_rounding, excess) = clock(channel, left, right);
    let encoded_duration = f64::from(right.output) - f64::from(left.output);
    for axis in 0..channel.width {
        let a = channel.component(axis, left)?;
        let b = channel.component(axis, right)?;
        let output_a = f64::from(round(a)?);
        let output_b = f64::from(round(b)?);
        let (rate, acceleration) = channel.derivative(axis, left, right)?;
        // A twice differentiable function differs from its endpoint chord by
        // at most max|f''| * h²/8. A Lipschitz bound also covers linear clamps.
        let endpoint_gap = (b - channel.component(
            axis,
            Key {
                left: true,
                ..right
            },
        )?)
        .abs();
        let error =
            (acceleration * duration * duration / 8.).min(rate * duration * 0.5) + endpoint_gap;
        let magnitude = (a.abs().max(b.abs()) + error)
            .max(output_a.abs())
            .max(output_b.abs());
        let output_rate = slope((output_b - output_a).abs(), encoded_duration);
        if !(magnitude <= MAX_COMPONENT
            && rate + output_rate <= 4096.
            && error + excess * output_rate <= approximation)
        {
            return Ok(false);
        }
        let bound =
            approximation + FLOOR + time_rounding * (rate + output_rate) + ULPS * ulp(magnitude);
        for t in [0.25, 0.5, 0.75] {
            let source = left.source + duration * t;
            let reference = channel.component(
                axis,
                Key {
                    source,
                    left: false,
                    ..left
                },
            )?;
            let reconstructed = output_a + (output_b - output_a) * t;
            if (reconstructed - reference).abs() > bound {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

pub(super) fn rotation(channel: &Channel<'_, '_, '_>, left: Key, right: Key) -> Result<bool> {
    let (duration, time_rounding, excess) = clock(channel, left, right);
    let a = channel.rotation(left)?;
    let mut b = channel.rotation(right)?;
    let aligned = dot(a, b).abs();
    if aligned <= 8. * ulp(1.) {
        return Ok(false);
    }
    if dot(a, b) < 0. {
        b = b.map(|v| -v);
    }
    let arc = angle(a, b) * 0.5;
    let output_a = rounded(a)?;
    let output_b = rounded(b)?;
    let output_rate = slope(
        angle(output_a, output_b),
        f64::from(right.output) - f64::from(left.output),
    );
    let Some((speed, acceleration)) = rotation_derivatives(channel, left, right)? else {
        return Ok(false);
    };
    let rate = 2. * speed;
    let chord = ((acceleration * duration * duration + arc * arc) / 8.)
        .min((speed * duration).midpoint(arc));
    let endpoint_gap = angle(
        b,
        channel.rotation(Key {
            left: true,
            ..right
        })?,
    );
    let error = 4. * (chord * 0.5).min(1.).asin() + endpoint_gap;
    if !(rate + output_rate <= 512. && error + excess * output_rate <= 1e-4) {
        return Ok(false);
    }
    let bound = 1e-4 + FLOOR + time_rounding * (rate + output_rate) + 32. * ulp(1.);
    for t in [0.25, 0.5, 0.75] {
        let source = left.source + duration * t;
        let reference = channel.rotation(Key {
            source,
            left: false,
            ..left
        })?;
        if angle(slerp(output_a, output_b, t), reference) > bound {
            return Ok(false);
        }
    }
    Ok(true)
}

fn rotation_derivatives(
    channel: &Channel<'_, '_, '_>,
    left: Key,
    right: Key,
) -> Result<Option<(f64, f64)>> {
    let Some(track) = channel.tracks.first() else {
        return Ok(None);
    };
    let mut speeds = [0.; 4];
    let mut accelerations = [0.; 4];
    let (lo, hi) = (left.source, right.source);
    for (axis, (speed, acceleration)) in speeds
        .iter_mut()
        .zip(&mut accelerations)
        .enumerate()
        .take(track.width())
    {
        (*speed, *acceleration) = track
            .segment(axis, lo, false, channel.meter)?
            .derivatives(lo, hi);
    }
    if matches!(
        track.target,
        BindingTarget::Transform {
            property: Euler,
            ..
        }
    ) {
        let gain = std::f64::consts::PI / 360.;
        let speed = speeds.into_iter().sum::<f64>() * gain;
        let acceleration = accelerations.into_iter().sum::<f64>() * gain + speed * speed;
        return Ok(Some((speed, acceleration)));
    }
    let speed = norm(speeds);
    let raw = track.raw_at(midpoint(lo, hi), false, channel.meter)?;
    let floor = norm(raw) - speed * (hi - lo) * 0.5;
    if floor <= 0. || !floor.is_finite() {
        return Ok(None);
    }
    let normalized_speed = speed / floor;
    Ok(Some((
        normalized_speed,
        norm(accelerations) / floor + 3. * normalized_speed.powi(2),
    )))
}

fn slope(distance: f64, time: f64) -> f64 {
    if distance == 0. { 0. } else { distance / time }
}

pub(super) fn dot(a: [f64; 4], b: [f64; 4]) -> f64 {
    a.into_iter().zip(b).map(|(x, y)| x * y).sum()
}

fn norm(q: [f64; 4]) -> f64 {
    dot(q, q).sqrt()
}

pub(super) fn rounded([x, y, z, w]: [f64; 4]) -> Result<[f64; 4]> {
    let value = [round(x)?, round(y)?, round(z)?, round(w)?].map(f64::from);
    Ok(value.map(|v| v / norm(value)))
}

pub(super) fn angle(a: [f64; 4], b: [f64; 4]) -> f64 {
    let sign = if dot(a, b) < 0. { -1. } else { 1. };
    let mut minus = 0.;
    let mut plus = 0.;
    for (x, y) in a.into_iter().zip(b) {
        minus += (x - sign * y).powi(2);
        plus += (x + sign * y).powi(2);
    }
    4. * minus.sqrt().atan2(plus.sqrt())
}

fn slerp(a: [f64; 4], b: [f64; 4], t: f64) -> [f64; 4] {
    let phi = angle(a, b) * 0.5;
    if phi == 0. {
        return a;
    }
    let weights = [
        ((1. - t) * phi).sin() / phi.sin(),
        (t * phi).sin() / phi.sin(),
    ];
    let mut result = [0.; 4];
    for (out, (x, y)) in result.iter_mut().zip(a.into_iter().zip(b)) {
        *out = weights[0] * x + weights[1] * y;
    }
    result
}
