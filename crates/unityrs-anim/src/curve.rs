//! Source-space curve equations from `docs/anim_reference.md`.
use crate::AnimError;
use unityrs_core::limits::Meter;

pub(crate) fn work_for_search(len: usize) -> u64 {
    u64::from(len.bit_width()) + 1
}

pub(crate) fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn bezier([a, b, c, d]: [f64; 4], parameter: f64) -> f64 {
    let ab = lerp(a, b, parameter);
    let bc = lerp(b, c, parameter);
    let cd = lerp(c, d, parameter);
    lerp(lerp(ab, bc, parameter), lerp(bc, cd, parameter), parameter)
}

pub(crate) fn hermite(a: f64, b: f64, outgoing: f64, incoming: f64, h: f64, u: f64) -> f64 {
    let u2 = u * u;
    let u3 = u2 * u;
    (2. * u3 - 3. * u2 + 1.) * a
        + (u3 - 2. * u2 + u) * (h * outgoing)
        + (-2. * u3 + 3. * u2) * b
        + (u3 - u2) * (h * incoming)
}

#[allow(
    clippy::float_cmp,
    reason = "the reference terminates on exact equality"
)]
fn solve(controls: [f64; 4], target: f64) -> f64 {
    let (mut lo, mut hi) = (0., 1.);
    for _ in 0..64 {
        let mid = lo + (hi - lo) * 0.5;
        let value = bezier(controls, mid);
        if mid == lo || mid == hi || value == target {
            return mid;
        }
        if value < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo + (hi - lo) * 0.5
}

pub(crate) fn weighted(
    values: [f64; 2],
    slopes: [f64; 2],
    weights: [f64; 2],
    duration: f64,
    normalized_time: f64,
    meter: &Meter,
) -> Result<f64, AnimError> {
    let [w0, w1] = weights;
    // The quadratic derivative has these three Bézier controls (before
    // multiplying by 3). Check its interior minimum as well as endpoints.
    let middle = 1. - w0 - w1;
    let curvature = w0 - 2. * middle + w1;
    if curvature > 0. {
        let vertex = (w0 - middle) / curvature;
        if 0. < vertex
            && vertex < 1.
            && lerp(lerp(w0, middle, vertex), lerp(middle, w1, vertex), vertex) < 0.
        {
            return Err(AnimError::UnsupportedTangentMode);
        }
    }
    meter.count_decode(64 * 24 + 32)?;
    let parameter = solve([0., w0, 1. - w1, 1.], normalized_time);
    let [a, b] = values;
    let [outgoing, incoming] = slopes;
    Ok(bezier(
        [
            a,
            a + (duration * outgoing) * w0,
            b - (duration * incoming) * w1,
            b,
        ],
        parameter,
    ))
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct EulerOrder([usize; 3]);

impl EulerOrder {
    pub(crate) fn read(order: Option<i32>) -> Result<Self, AnimError> {
        let axes = match order {
            Some(0) => [0, 1, 2],
            Some(1) => [0, 2, 1],
            Some(2) => [1, 2, 0],
            Some(3) => [1, 0, 2],
            Some(4) => [2, 0, 1],
            Some(5) => [2, 1, 0],
            _ => return Err(AnimError::UnsupportedRotationOrder(order)),
        };
        Ok(Self(axes))
    }

    pub(crate) fn rotation(self, degrees: [f64; 4]) -> Result<[f64; 4], AnimError> {
        let mut result = [0., 0., 0., 1.];
        for i in self.0 {
            let angle =
                degrees.get(i).ok_or(AnimError::InvalidQuaternion)? * (std::f64::consts::PI / 360.);
            let (sine, cosine) = (angle.sin(), angle.cos());
            let mut axis = [0., 0., 0., cosine];
            *axis.get_mut(i).ok_or(AnimError::InvalidQuaternion)? = sine;
            result = multiply(axis, result);
        }
        normalize(result)
    }
}

#[allow(
    clippy::many_single_char_names,
    reason = "the Hamilton product uses component symbols"
)]
fn multiply([x, y, z, w]: [f64; 4], [a, b, c, d]: [f64; 4]) -> [f64; 4] {
    [
        w * a + x * d + y * c - z * b,
        w * b - x * c + y * d + z * a,
        w * c + x * b - y * a + z * d,
        w * d - x * a - y * b - z * c,
    ]
}

pub(crate) fn normalize(q: [f64; 4]) -> Result<[f64; 4], AnimError> {
    if q.into_iter().any(|x| !x.is_finite()) {
        return Err(AnimError::InvalidQuaternion);
    }
    // Scale first to avoid intermediate overflow or underflow.
    let max = q.into_iter().map(f64::abs).fold(0., f64::max);
    if max == 0. {
        return Err(AnimError::InvalidQuaternion);
    }
    let scaled = q.map(|x| x / max);
    let norm = scaled.into_iter().map(|x| x * x).sum::<f64>().sqrt();
    Ok(scaled.map(|x| x / norm))
}
