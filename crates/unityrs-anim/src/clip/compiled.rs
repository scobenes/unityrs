//! Typed streamed, dense and constant storage in consecutive slot order.
use super::ClipTiming;
use crate::{
    AnimError, BindingKind, BindingTable, SlotBinding, SlotCounts,
    curve::{lerp, work_for_search},
    read::Dict,
};
use unityrs_core::{
    env::PPtr,
    limits::{Meter, MeteredVec},
    value::{PrimArray, Value},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageKind {
    Streamed,
    Dense,
    Constant,
}

/// Typed serialized samples. Object references remain scoped to the clip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SlotValue {
    Float(f64),
    Integer(i32),
    Object(PPtr),
    /// The managed-reference encoding is retained for a later consumer.
    ManagedReferenceBits(u32),
}

impl SlotValue {
    fn decode(
        bits: u32,
        binding: BindingKind,
        slot: usize,
        mapping: &[PPtr],
    ) -> Result<Self, AnimError> {
        match binding {
            BindingKind::Integer => Ok(Self::Integer(i32::from_ne_bytes(bits.to_ne_bytes()))),
            BindingKind::ManagedReference => Ok(Self::ManagedReferenceBits(bits)),
            BindingKind::PPtr => {
                let index = f64::from(f32::from_bits(bits));
                if !index.is_finite()
                    || index < 0.
                    || index.fract() != 0.
                    || index > f64::from(u32::MAX)
                {
                    return Err(AnimError::InvalidPPtrIndex(slot));
                }
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let pointer = mapping
                    .get(index as usize)
                    .ok_or(AnimError::InvalidPPtrIndex(slot))?;
                Ok(Self::Object(*pointer))
            }
            _ => {
                let value = f64::from(f32::from_bits(bits));
                if !value.is_finite() {
                    return Err(AnimError::InvalidSampleValue);
                }
                Ok(Self::Float(value))
            }
        }
    }

    pub(crate) fn continuous(self) -> Result<f64, AnimError> {
        match self {
            Self::Float(x) => Ok(x),
            _ => Err(AnimError::InvalidSampleValue),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StreamKey {
    pub time: f64,
    /// Raw coefficient words in cubic-to-constant order.
    pub coefficients: [u32; 4],
    pub value: SlotValue,
}

#[derive(Debug)]
pub struct StreamSlot {
    seed: Option<PPtr>,
    keys: MeteredVec<StreamKey>,
}

impl StreamSlot {
    #[must_use]
    pub fn seed(&self) -> Option<PPtr> {
        self.seed
    }

    #[must_use]
    pub fn keys(&self) -> &[StreamKey] {
        self.keys.as_slice()
    }

    fn at(&self, slot: usize, sample: f64, meter: &Meter) -> Result<SlotValue, AnimError> {
        let keys = self.keys();
        meter.count_decode(work_for_search(keys.len()) + 12)?;
        let next = keys.partition_point(|key| key.time <= sample);
        let Some(left) = next.checked_sub(1).and_then(|i| keys.get(i)) else {
            return self
                .seed
                .map(SlotValue::Object)
                .or_else(|| keys.first().map(|key| key.value))
                .ok_or(AnimError::MissingStreamKey(slot));
        };
        if next == keys.len() || sample <= left.time || !matches!(left.value, SlotValue::Float(_)) {
            return Ok(left.value);
        }
        let [c3, c2, c1, c0] = left.coefficients.map(|x| f64::from(f32::from_bits(x)));
        let d = sample - left.time;
        Ok(SlotValue::Float(((c3 * d + c2) * d + c1) * d + c0))
    }
}

#[derive(Debug)]
pub(crate) struct Dense {
    size: usize,
    begin: f64,
    rate: f64,
    grid: MeteredVec<f64>,
    values: MeteredVec<SlotValue>,
}

impl Dense {
    fn at(&self, slot: usize, sample: f64, meter: &Meter) -> Result<SlotValue, AnimError> {
        let grid = self.grid.as_slice();
        meter.count_decode(work_for_search(grid.len()) + 12)?;
        let next = grid.partition_point(|&time| time <= sample);
        let frame = next.saturating_sub(1);
        let get = |i: usize| {
            i.checked_mul(self.size)
                .and_then(|index| index.checked_add(slot))
                .and_then(|index| self.values.as_slice().get(index).copied())
                .ok_or(AnimError::InvalidDenseGrid)
        };
        let first = get(frame)?;
        let time = *grid.get(frame).ok_or(AnimError::InvalidDenseGrid)?;
        if let SlotValue::Float(a) = first
            && next < grid.len()
            && sample > time
        {
            let b = get(next)?.continuous()?;
            return Ok(SlotValue::Float(lerp(
                a,
                b,
                ((sample - time) * self.rate).min(1.),
            )));
        }
        Ok(first)
    }
}

/// Decoded, checked slot data and the compiled source interval.
#[derive(Debug)]
pub struct CompiledClip {
    pub(crate) timing: ClipTiming,
    pub(crate) slots: MeteredVec<SlotBinding>,
    streams: MeteredVec<StreamSlot>,
    dense: Dense,
    constants: MeteredVec<SlotValue>,
}

impl CompiledClip {
    #[must_use]
    pub fn slot_bindings(&self) -> &[SlotBinding] {
        self.slots.as_slice()
    }

    #[must_use]
    pub fn counts(&self) -> SlotCounts {
        SlotCounts {
            streamed: self.streams.len(),
            dense: self.dense.size,
            constant: self.constants.len(),
        }
    }

    #[must_use]
    pub fn streamed(&self) -> &[StreamSlot] {
        self.streams.as_slice()
    }

    #[must_use]
    pub fn constants(&self) -> &[SlotValue] {
        self.constants.as_slice()
    }

    /// Frame-major samples, including held metadata payloads.
    #[must_use]
    pub fn dense_values(&self) -> &[SlotValue] {
        self.dense.values.as_slice()
    }

    /// Number of frames, grid begin and grid sample rate.
    #[must_use]
    pub fn dense_grid(&self) -> (usize, f64, f64) {
        (self.dense.grid.len(), self.dense.begin, self.dense.rate)
    }

    pub(crate) fn storage(&self, slot: usize) -> StorageKind {
        if slot < self.streams.len() {
            StorageKind::Streamed
        } else if slot - self.streams.len() < self.dense.size {
            StorageKind::Dense
        } else {
            StorageKind::Constant
        }
    }

    /// Inspect one slot at a source time, retaining the typed metadata value.
    /// Object references are returned unresolved and events remain inert.
    pub fn slot_value(
        &self,
        slot: usize,
        source: f64,
        meter: &Meter,
    ) -> Result<SlotValue, AnimError> {
        if !self.timing.contains(source) {
            return Err(AnimError::InvalidSchedule);
        }
        if let Some(stream) = self.streams.as_slice().get(slot) {
            return stream.at(slot, source, meter);
        }
        let slot = slot - self.streams.len();
        if slot < self.dense.size {
            return self.dense.at(slot, source, meter);
        }
        meter.count_decode(1)?;
        self.constants
            .as_slice()
            .get(slot - self.dense.size)
            .copied()
            .ok_or(AnimError::SlotOverflow)
    }
}

fn refuse_humanoid(dict: &Dict<'_, '_>, muscle: &Value) -> Result<(), AnimError> {
    let Some(value) = dict.field(muscle, "m_IndexArray")? else {
        return Err(AnimError::UnsupportedClipLayout);
    };
    let values = match value {
        Value::Prims(PrimArray::I32(values)) => values.as_ref(),
        Value::List(values) if values.is_empty() => &[],
        _ => return Err(AnimError::UnsupportedClipLayout),
    };
    dict.meter.count_decode(values.len() as u64)?;
    if values.iter().any(|&slot| slot != -1) {
        return Err(AnimError::UnsupportedHumanoidPlayback);
    }
    Ok(())
}

#[allow(
    clippy::too_many_lines,
    reason = "validate clock and layout before decoding their sample arrays"
)]
pub(super) fn decode(
    dict: &Dict<'_, '_>,
    muscle: &Value,
    table: Option<&BindingTable>,
    rate: f64,
) -> Result<Option<CompiledClip>, AnimError> {
    let Some(clip) = dict.field(muscle, "m_Clip")? else {
        return Err(AnimError::UnsupportedClipLayout);
    };
    let clip = dict.field(clip, "data")?.unwrap_or(clip);
    let stream = dict.required(clip, "m_StreamedClip")?;
    // The split count introduced in newer trees needs fixture evidence for
    // the relationship between the two counts and the serialized slots.
    if dict
        .field(stream, "discreteCurveCount")?
        .is_some_and(|value| value.as_u64() != Some(0))
    {
        return Err(AnimError::UnsupportedClipLayout);
    }
    let words = dict.words(stream, "data", false)?;
    let dense = dict.required(clip, "m_DenseClip")?;
    let dense_bits = dict.words(dense, "m_SampleArray", true)?;
    let constant_bits = dict
        .field(clip, "m_ConstantClip")?
        .map_or(Ok(&[][..]), |constant| dict.words(constant, "data", true))?;
    let frame_count = dict.count(dense, "m_FrameCount")?;
    let counts = SlotCounts {
        streamed: dict.count(stream, "curveCount")?,
        dense: dict.count(dense, "m_CurveCount")?,
        constant: constant_bits.len(),
    };
    if counts.dense.checked_mul(frame_count) != Some(dense_bits.len())
        || (counts.dense > 0 && frame_count == 0)
    {
        return Err(AnimError::InvalidDenseGrid);
    }
    if counts == SlotCounts::default()
        && table.is_none_or(|table| table.bindings().is_empty())
        && dict
            .field(clip, "m_Binding")?
            .is_none_or(|_| table.is_some())
    {
        decode_stream(dict, 0, words, &[], &[])?;
        return Ok(None);
    }

    refuse_humanoid(dict, muscle)?;
    let table = table.ok_or(AnimError::UnsupportedBindingLayout)?;
    let slots = table.slots(counts, dict.meter)?;
    let timing = ClipTiming::new(
        dict.float(muscle, "m_StartTime")?,
        dict.float(muscle, "m_StopTime")?,
        rate,
    )?;
    let mut kinds = MeteredVec::new(dict.meter);
    for slot in slots.as_slice() {
        dict.meter.count_decode(u64::from(slot.width))?;
        for _ in 0..slot.width {
            kinds.push(slot.kind)?;
        }
    }
    let kinds = kinds.as_slice();
    let mapping = table.pptr_mapping();
    let streams = decode_stream(dict, counts.streamed, words, kinds, mapping)?;
    let begin = dict.float(dense, "m_BeginTime")?;
    let dense_rate = dict.float(dense, "m_SampleRate")?;
    if !begin.is_finite() || !dense_rate.is_finite() || (counts.dense > 0 && dense_rate <= 0.) {
        return Err(AnimError::InvalidDenseGrid);
    }
    let mut grid = MeteredVec::new(dict.meter);
    if counts.dense > 0 {
        dict.meter.count_decode(frame_count as u64)?;
        grid.reserve_exact(frame_count)?;
        let frames = u32::try_from(frame_count).map_err(|_| AnimError::InvalidDenseGrid)?;
        for i in 0..frames {
            let time = begin + f64::from(i) / dense_rate;
            if !time.is_finite()
                || grid
                    .as_slice()
                    .last()
                    .is_some_and(|&previous| time <= previous)
            {
                return Err(AnimError::InvalidDenseGrid);
            }
            grid.push(time)?;
        }
    }
    let mut dense_values = MeteredVec::with_capacity(dict.meter, dense_bits.len())?;
    let mut constants = MeteredVec::with_capacity(dict.meter, constant_bits.len())?;
    for (i, &bits) in dense_bits.iter().enumerate() {
        let slot = counts.streamed + i % counts.dense;
        dense_values.push(decode_value(bits, slot, kinds, mapping)?)?;
    }
    for (i, &bits) in constant_bits.iter().enumerate() {
        let slot = counts.streamed + counts.dense + i;
        constants.push(decode_value(bits, slot, kinds, mapping)?)?;
    }
    Ok(Some(CompiledClip {
        timing,
        slots,
        streams,
        dense: Dense {
            size: counts.dense,
            begin,
            rate: dense_rate,
            grid,
            values: dense_values,
        },
        constants,
    }))
}

fn decode_value(
    bits: u32,
    slot: usize,
    kinds: &[BindingKind],
    mapping: &[PPtr],
) -> Result<SlotValue, AnimError> {
    SlotValue::decode(
        bits,
        *kinds.get(slot).ok_or(AnimError::SlotOverflow)?,
        slot,
        mapping,
    )
}

fn decode_stream(
    dict: &Dict<'_, '_>,
    count: usize,
    mut words: &[u32],
    kinds: &[BindingKind],
    mapping: &[PPtr],
) -> Result<MeteredVec<StreamSlot>, AnimError> {
    let mut slots = MeteredVec::with_capacity(dict.meter, count)?;
    for _ in 0..count {
        slots.push(StreamSlot {
            seed: None,
            keys: MeteredVec::new(dict.meter),
        })?;
    }
    let mut previous = None;
    let mut first = true;
    while !words.is_empty() {
        let [bits, count, rest @ ..] = words else {
            return Err(AnimError::InvalidStreamFrame);
        };
        let entries = (*count as usize)
            .checked_mul(5)
            .ok_or(AnimError::InvalidStreamFrame)?;
        let (keys, rest) = rest
            .split_at_checked(entries)
            .ok_or(AnimError::InvalidStreamFrame)?;
        words = rest;
        let time = f32::from_bits(*bits);
        if time == f32::INFINITY {
            if !keys.is_empty() || !words.is_empty() {
                return Err(AnimError::InvalidStreamFrame);
            }
            break;
        }
        if !time.is_finite() || previous.is_some_and(|last| last >= time) {
            return Err(AnimError::InvalidStreamFrame);
        }
        let seed = first && *bits == f32::MIN.to_bits();
        let time = f64::from(time);
        for &[slot, c3, c2, c1, c0] in keys.as_chunks::<5>().0 {
            let slot = slot as usize;
            let stream = slots
                .as_mut_slice()
                .get_mut(slot)
                .ok_or(AnimError::InvalidStreamFrame)?;
            let value = decode_value(c0, slot, kinds, mapping)?;
            if seed {
                if let SlotValue::Object(pointer) = value
                    && stream.seed.replace(pointer).is_none()
                {
                    continue;
                }
                return Err(AnimError::InvalidStreamFrame);
            }
            let coefficients = [c3, c2, c1, c0];
            if stream
                .keys
                .as_slice()
                .last()
                .is_some_and(|last| last.time >= time)
                || (matches!(value, SlotValue::Float(_))
                    && coefficients
                        .iter()
                        .any(|&bits| !f32::from_bits(bits).is_finite()))
            {
                return Err(AnimError::InvalidStreamFrame);
            }
            stream.keys.push(StreamKey {
                time,
                coefficients,
                value,
            })?;
        }
        first = false;
        previous = Some(f32::from_bits(*bits));
    }
    for (i, slot) in slots.as_slice().iter().enumerate() {
        if slot.keys.is_empty() && slot.seed.is_none() {
            return Err(AnimError::MissingStreamKey(i));
        }
    }
    Ok(slots)
}

const _: () = crate::send_sync::<StorageKind>();
const _: () = crate::send_sync::<SlotValue>();
const _: () = crate::send_sync::<StreamKey>();
const _: () = crate::send_sync::<StreamSlot>();
const _: () = crate::send_sync::<CompiledClip>();
