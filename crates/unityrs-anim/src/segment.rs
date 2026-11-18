//! Bundle-derived scalar segments shared by baking and replay evidence.
use crate::{AnimError, curve};
use unityrs_core::limits::Meter;

#[derive(Clone, Copy, Debug)]
pub enum ScalarSegment {
    Constant(f64),
    Held(f64),
    Streamed {
        key: f64,
        coefficients: [f64; 4],
    },
    Dense {
        values: [f64; 2],
        key: f64,
        rate: f64,
        begin: f64,
        offset: f64,
    },
    Hermite {
        values: [f64; 2],
        slopes: [f64; 2],
        key: f64,
        duration: f64,
    },
    Weighted {
        values: [f64; 2],
        slopes: [f64; 2],
        key: f64,
        duration: f64,
        weights: [f64; 2],
    },
}

const _: () = crate::send_sync::<ScalarSegment>();

impl ScalarSegment {
    pub fn at(self, time: f64, meter: &Meter) -> Result<f64, AnimError> {
        meter.count_decode(32)?;
        Ok(match self {
            Self::Constant(value) | Self::Held(value) => value,
            Self::Streamed {
                key,
                coefficients: [c3, c2, c1, c0],
            } => {
                let delta = time - key;
                ((c3 * delta + c2) * delta + c1) * delta + c0
            }
            Self::Dense {
                values: [a, b],
                key,
                rate,
                ..
            } => curve::lerp(a, b, ((time - key) * rate).clamp(0., 1.)),
            Self::Hermite {
                values: [a, b],
                slopes: [outgoing, incoming],
                key,
                duration,
            } => {
                let parameter = (time - key) / duration;
                if parameter <= 0. {
                    a
                } else if parameter >= 1. {
                    b
                } else {
                    curve::hermite(a, b, outgoing, incoming, duration, parameter)
                }
            }
            Self::Weighted {
                values: [a, b],
                slopes,
                key,
                duration,
                weights,
            } => {
                let parameter = (time - key) / duration;
                if parameter <= 0. {
                    a
                } else if parameter >= 1. {
                    b
                } else {
                    curve::weighted([a, b], slopes, weights, duration, parameter, meter)?
                }
            }
        })
    }

    #[must_use]
    pub fn is_held(self) -> bool {
        matches!(self, Self::Held(_) | Self::Constant(_))
    }

    /// Upper bounds on the first and second derivative over the source interval.
    #[must_use]
    #[allow(
        clippy::float_cmp,
        reason = "the affine shortcut requires exact equality"
    )]
    pub fn derivatives(self, lo: f64, hi: f64) -> (f64, f64) {
        match self {
            Self::Held(_) | Self::Constant(_) => (0., 0.),
            Self::Streamed {
                key,
                coefficients: [c3, c2, c1, _],
            } => {
                let end = (hi - key).abs().max((lo - key).abs());
                (
                    3. * c3.abs() * end * end + 2. * c2.abs() * end + c1.abs(),
                    6. * c3.abs() * end + 2. * c2.abs(),
                )
            }
            Self::Dense {
                values: [a, b],
                key,
                rate,
                ..
            } => {
                // Rounding the dense grid origin can place the clamp inside
                // an interval. A derivative jump needs a Lipschitz bound.
                let slope = (b - a).abs() * rate;
                let crosses_clamp = slope != 0. && (lo - key) * rate < 1. && (hi - key) * rate > 1.;
                (slope, if crosses_clamp { f64::INFINITY } else { 0. })
            }
            Self::Hermite {
                values,
                slopes,
                duration,
                ..
            } => {
                let (first, second) = bezier_derivatives(values, slopes, duration, [1. / 3.; 2]);
                (first / duration, second / duration.powi(2))
            }
            Self::Weighted {
                values,
                slopes,
                duration,
                weights: [outgoing, incoming],
                key,
            } => {
                // Identical affine time and value controls reduce to a
                // straight line even where the parameterization stalls.
                let delta = values[1] - values[0];
                if (outgoing == 0. || duration * slopes[0] == delta)
                    && (incoming == 0. || duration * slopes[1] == delta)
                {
                    return (delta.abs() / duration, 0.);
                }
                let (first, second) =
                    bezier_derivatives(values, slopes, duration, [outgoing, incoming]);
                let middle = 1. - outgoing - incoming;
                let controls = [0., outgoing, 1. - incoming, 1.];
                let a = parameter_bound(controls, (lo - key) / duration, true);
                let b = parameter_bound(controls, (hi - key) / duration, false);
                let time_derivative = |u| {
                    curve::lerp(
                        curve::lerp(outgoing, middle, u),
                        curve::lerp(middle, incoming, u),
                        u,
                    )
                };
                let mut floor = time_derivative(a).min(time_derivative(b));
                let curvature = outgoing - 2. * middle + incoming;
                if curvature > 0. {
                    let vertex = (outgoing - middle) / curvature;
                    if (a..=b).contains(&vertex) {
                        floor = floor.min(time_derivative(vertex));
                    }
                }
                let time_rate = 3. * (floor - 16. * f64::EPSILON) * duration;
                if time_rate <= 0. {
                    return if first == 0. {
                        (0., 0.)
                    } else {
                        (f64::INFINITY, f64::INFINITY)
                    };
                }
                let time_curvature =
                    6. * duration * (middle - outgoing).abs().max((incoming - middle).abs());
                (
                    first / time_rate,
                    second / time_rate.powi(2) + first * time_curvature / time_rate.powi(3),
                )
            }
        }
    }
}

/// Invert to an outer parameter bound. Bias comparisons to retain the
/// enclosure through rounding of normalized time and the unit Bézier.
fn parameter_bound(controls: [f64; 4], time: f64, lower: bool) -> f64 {
    if time <= 0. {
        return 0.;
    }
    if time >= 1. {
        return 1.;
    }
    let margin = 16. * f64::EPSILON;
    let target = if lower { time - margin } else { time + margin };
    let (mut lo, mut hi) = (0., 1.);
    for _ in 0..32 {
        let mid = f64::midpoint(lo, hi);
        if curve::bezier(controls, mid) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    if lower { lo } else { hi }
}

fn bezier_derivatives(
    [a, b]: [f64; 2],
    [outgoing, incoming]: [f64; 2],
    duration: f64,
    [w0, w1]: [f64; 2],
) -> (f64, f64) {
    let first = duration * outgoing * w0;
    let last = duration * incoming * w1;
    let middle = (b - a) - first - last;
    (
        3. * first.abs().max(middle.abs()).max(last.abs()),
        6. * (middle - first).abs().max((last - middle).abs()),
    )
}

#[cfg(test)]
mod tests {
    use super::ScalarSegment;

    #[test]
    fn dense_rounded_grid_can_clamp_inside_a_source_interval() {
        let begin = 536_870_912.;
        let rate = 220.;
        let key = begin + 41. / rate;
        let end = begin + 42. / rate;
        let dense = ScalarSegment::Dense {
            values: [-4.602_272_510_528_564_5, 4.602_272_510_528_564_5],
            key,
            rate,
            begin,
            offset: 41. / rate,
        };
        assert!(dense.derivatives(key, end).1.is_infinite());
        assert!(dense.derivatives(key, key + (end - key) * 0.5).1.abs() < f64::EPSILON);
    }

    #[test]
    fn weighted_time_derivative_floor_follows_the_subdivision() {
        let curve = ScalarSegment::Weighted {
            values: [0., 1.],
            slopes: [0.; 2],
            key: 2.,
            duration: 1.,
            weights: [1.; 2],
        };
        assert!(curve.derivatives(2., 3.).0.is_infinite());
        assert!(curve.derivatives(2.25, 2.5).0.is_infinite());
        let (rate, acceleration) = curve.derivatives(2.125, 2.25);
        assert!((1. ..2.).contains(&rate), "rate {rate}");
        assert!(acceleration.is_finite());
    }
}
