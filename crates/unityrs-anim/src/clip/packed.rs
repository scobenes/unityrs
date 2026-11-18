//! Field decoding for legacy compressed rotation. Its derivative convention
//! still requires off-key playback evidence (`docs/anim_reference.md`).
use crate::{AnimError, read::Dict};
use unityrs_core::{limits::MeteredVec, value::Value};

/// Independently decoded packed vectors. Slopes have no assigned key, unit,
/// or in/out interpretation.
#[derive(Debug)]
pub struct PackedRotation {
    pub(crate) times: MeteredVec<f64>,
    values: MeteredVec<[f64; 4]>,
    slopes: Option<MeteredVec<f64>>,
}

impl PackedRotation {
    #[must_use]
    pub fn times(&self) -> &[f64] {
        self.times.as_slice()
    }

    #[must_use]
    pub fn values(&self) -> &[[f64; 4]] {
        self.values.as_slice()
    }

    #[must_use]
    pub fn slopes(&self) -> Option<&[f64]> {
        self.slopes.as_ref().map(MeteredVec::as_slice)
    }
}

struct Bits<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Bits<'_> {
    fn take(&mut self, width: u32) -> Result<u32, AnimError> {
        let mut result = 0;
        let mut shifts = 0;
        while shifts < width {
            let byte = *self
                .bytes
                .get(self.position / 8)
                .ok_or(AnimError::UnsupportedClipLayout)?;
            let skip = self.position % 8;
            let available =
                8 - u32::try_from(skip).map_err(|_| AnimError::UnsupportedClipLayout)?;
            let take = (width - shifts).min(available);
            let bits = u32::from(byte >> skip) & ((1 << take) - 1);
            result |= bits << shifts;
            shifts += take;
            self.position = self
                .position
                .checked_add(take as usize)
                .ok_or(AnimError::SlotOverflow)?;
        }
        Ok(result)
    }
}

fn vector<'a>(
    dict: &Dict<'a, '_>,
    row: &'a Value,
    width: usize,
) -> Result<(usize, Bits<'a>), AnimError> {
    let count = dict.count(row, "m_NumItems")?;
    let data = dict
        .required(row, "m_Data")?
        .as_bytes()
        .ok_or(AnimError::Malformed("m_Data"))?;
    let required = count.checked_mul(width).ok_or(AnimError::SlotOverflow)?;
    if required.div_ceil(8) > data.len() {
        return Err(AnimError::Malformed("m_Data"));
    }
    dict.meter
        .count_decode((count as u64).saturating_mul(width as u64 + 1))?;
    Ok((
        count,
        Bits {
            bytes: data,
            position: 0,
        },
    ))
}

fn width(dict: &Dict<'_, '_>, row: &Value) -> Result<u32, AnimError> {
    let width = dict.count(row, "m_BitSize")?;
    u32::try_from(width)
        .ok()
        .filter(|&width| width <= 32)
        .ok_or(AnimError::Malformed("m_BitSize"))
}

pub(super) fn read(dict: &Dict<'_, '_>, row: &Value) -> Result<PackedRotation, AnimError> {
    let times = dict.required(row, "m_Times")?;
    let time_width = width(dict, times)?;
    let (count, mut bits) = vector(dict, times, time_width as usize)?;
    let (value_count, mut quats) = vector(dict, dict.required(row, "m_Values")?, 32)?;
    if count == 0 || count != value_count {
        return Err(AnimError::InvalidCurveKeys);
    }
    let mut times = MeteredVec::with_capacity(dict.meter, count)?;
    let mut values = MeteredVec::with_capacity(dict.meter, count)?;
    let mut accumulated = 0i64;
    for i in 0..count {
        let delta = i64::from(bits.take(time_width)?);
        if i != 0 && delta == 0 {
            return Err(AnimError::InvalidCurveKeys);
        }
        accumulated = accumulated
            .checked_add(delta)
            .ok_or(AnimError::InvalidCurveKeys)?;
        #[allow(
            clippy::cast_precision_loss,
            reason = "the reference rounds the integer sum to f64"
        )]
        let time = accumulated as f64 / 100.;
        if times
            .as_slice()
            .last()
            .is_some_and(|&previous| time <= previous)
        {
            return Err(AnimError::InvalidCurveKeys);
        }
        times.push(time)?;
        values.push(quaternion(&mut quats)?)?;
    }
    let slopes = dict
        .field(row, "m_Slopes")?
        .map(|slopes| read_slopes(dict, slopes))
        .transpose()?;
    Ok(PackedRotation {
        times,
        values,
        slopes,
    })
}

// Smallest-three packed quaternion: three flag bits followed by 9, 10 and
// 10 bits, in XYZW component order with one component reconstructed.
fn quaternion(bits: &mut Bits<'_>) -> Result<[f64; 4], AnimError> {
    let flags = bits.take(3)?;
    let omitted = (flags & 3) as usize;
    let short = (omitted + 1) % 4;
    let mut result = [0.; 4];
    for (i, out) in result.iter_mut().enumerate() {
        if i != omitted {
            let width = if i == short { 9 } else { 10 };
            let maximum = f64::from((1u32 << width) - 1);
            *out = 2. * f64::from(bits.take(width)?) / maximum - 1.;
        }
    }
    let square = 1. - result.into_iter().map(|x| x * x).sum::<f64>();
    if square < 0. {
        return Err(AnimError::InvalidQuaternion);
    }
    *result
        .get_mut(omitted)
        .ok_or(AnimError::InvalidQuaternion)? = if flags & 4 == 0 {
        square.sqrt()
    } else {
        -square.sqrt()
    };
    Ok(result)
}

fn read_slopes(dict: &Dict<'_, '_>, row: &Value) -> Result<MeteredVec<f64>, AnimError> {
    let width = width(dict, row)?;
    let (count, mut bits) = vector(dict, row, width as usize)?;
    let start = dict.float(row, "m_Start")?;
    let range = dict.float(row, "m_Range")?;
    if !start.is_finite() || !range.is_finite() || range < 0. || (width == 0 && range != 0.) {
        return Err(AnimError::Malformed("m_Slopes"));
    }
    let mut values = MeteredVec::with_capacity(dict.meter, count)?;
    let maximum = f64::from(u32::MAX.checked_shr(32 - width).unwrap_or(0));
    for _ in 0..count {
        let value = if width == 0 {
            start
        } else {
            start + (f64::from(bits.take(width)?) / maximum) * range
        };
        values.push(value)?;
    }
    Ok(values)
}

const _: () = crate::send_sync::<PackedRotation>();
