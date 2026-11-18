//! Explicit runtime key records. Source election precedes key decoding so
//! curves shadowed by the selected rotation representation stay metadata.
use super::{ClipField, ClipSource, PackedRotation, packed};
use crate::{
    AnimError, TransformProperty,
    curve::{EulerOrder, hermite, normalize, weighted, work_for_search},
    read::{Dict, flag, int},
};
use unityrs_core::{
    RawStr,
    limits::{Meter, MeteredVec},
    value::Value,
};

#[derive(Debug)]
pub(crate) struct Explicit {
    pub source: ClipSource,
    pub property: Property,
    pub path: RawStr,
    pub data: Data,
}

#[derive(Debug)]
pub(crate) enum Property {
    Transform(TransformProperty),
    Component(TransformProperty, usize),
    RotationComponent,
    Morph(RawStr),
}

impl Property {
    fn rank(&self) -> (u8, Option<usize>, &[u8]) {
        use TransformProperty as T;
        match self {
            Self::Transform(T::Translation) => (0, None, b""),
            Self::Component(T::Translation, axis) => (0, Some(*axis), b""),
            Self::Transform(T::Scale) => (1, None, b""),
            Self::Component(T::Scale, axis) => (1, Some(*axis), b""),
            Self::Transform(T::Quaternion | T::Euler)
            | Self::RotationComponent
            | Self::Component(_, _) => (2, None, b""),
            Self::Morph(attribute) => (3, None, attribute.as_bytes()),
        }
    }

    fn arity(&self) -> usize {
        match self {
            Self::Transform(TransformProperty::Quaternion) => 4,
            Self::Transform(_) => 3,
            Self::Morph(_) | Self::Component(_, _) | Self::RotationComponent => 1,
        }
    }
}

#[derive(Debug)]
pub(crate) enum Data {
    Curve(Curve),
    Packed(PackedRotation),
}

#[derive(Debug)]
pub(crate) struct Candidate<'a> {
    pub property: Property,
    pub path: RawStr,
    priority: u8,
    source: ClipSource,
    row: &'a Value,
}

impl Candidate<'_> {
    fn key(&self) -> (u8, &[u8], Option<usize>, &[u8]) {
        let (family, component, attribute) = self.property.rank();
        (family, self.path.as_bytes(), component, attribute)
    }
}

#[derive(Debug)]
pub(crate) struct Curve {
    pub order: Option<EulerOrder>,
    keys: MeteredVec<Key>,
    width: usize,
    pre: i32,
    post: i32,
}

#[derive(Debug)]
struct Key {
    time: f64,
    value: [f64; 4],
    incoming: [f64; 4],
    outgoing: [f64; 4],
    in_weight: Option<[f64; 4]>,
    out_weight: Option<[f64; 4]>,
}

impl Explicit {
    pub(crate) fn bounds(&self) -> Result<(f64, f64), AnimError> {
        let endpoints = match &self.data {
            Data::Curve(curve) => curve
                .keys
                .as_slice()
                .first()
                .zip(curve.keys.as_slice().last())
                .map(|(left, right)| (left.time, right.time)),
            Data::Packed(packed) => packed
                .times()
                .first()
                .zip(packed.times().last())
                .map(|(&left, &right)| (left, right)),
        };
        endpoints.ok_or(AnimError::InvalidCurveKeys)
    }
}

fn clamp(raw: i32) -> Result<(), AnimError> {
    // Curve infinity codes in the serialized layout. The clip's m_WrapMode
    // uses a separate enum and does not override per-curve boundary behavior.
    if raw == 2 {
        Ok(())
    } else {
        Err(AnimError::UnsupportedCurveWrap(raw))
    }
}

impl Curve {
    pub(crate) fn key_times(&self) -> impl Iterator<Item = f64> + '_ {
        self.keys.as_slice().iter().map(|key| key.time)
    }

    pub(crate) fn width(&self) -> usize {
        self.width
    }

    pub(crate) fn segment(
        &self,
        axis: usize,
        time: f64,
        left_side: bool,
        meter: &Meter,
    ) -> Result<crate::ScalarSegment, AnimError> {
        use crate::ScalarSegment as S;
        let keys = self.keys.as_slice();
        meter.count_decode(work_for_search(keys.len()) + 8)?;
        let next = keys.partition_point(|key| key.time < time || (!left_side && key.time <= time));
        let Some(left) = next.checked_sub(1).and_then(|index| keys.get(index)) else {
            if keys.first().is_some_and(|key| time < key.time) {
                clamp(self.pre)?;
            }
            let first = keys.first().ok_or(AnimError::InvalidCurveKeys)?;
            return Ok(S::Constant(component(&first.value, axis)?));
        };
        let a = component(&left.value, axis)?;
        let Some(right) = keys.get(next) else {
            if time > left.time {
                clamp(self.post)?;
            }
            return Ok(S::Constant(a));
        };
        let values = [a, component(&right.value, axis)?];
        let slopes = [
            component(&left.outgoing, axis)?,
            component(&right.incoming, axis)?,
        ];
        let key = left.time;
        let duration = right.time - key;
        Ok(if slopes.iter().any(|slope| slope.is_infinite()) {
            S::Held(a)
        } else if left.out_weight.is_some() || right.in_weight.is_some() {
            let weights = [
                handle(left.out_weight.as_ref(), axis)?,
                handle(right.in_weight.as_ref(), axis)?,
            ];
            S::Weighted {
                values,
                slopes,
                key,
                duration,
                weights,
            }
        } else {
            S::Hermite {
                values,
                slopes,
                key,
                duration,
            }
        })
    }

    pub(crate) fn at(&self, time: f64, meter: &Meter) -> Result<[f64; 4], AnimError> {
        let keys = self.keys.as_slice();
        meter.count_decode(work_for_search(keys.len()))?;
        let next = keys.partition_point(|key| key.time <= time);
        let Some(left) = next.checked_sub(1).and_then(|index| keys.get(index)) else {
            clamp(self.pre)?;
            return keys
                .first()
                .map(|key| key.value)
                .ok_or(AnimError::InvalidCurveKeys);
        };
        if time <= left.time {
            return Ok(left.value);
        }
        let Some(right) = keys.get(next) else {
            clamp(self.post)?;
            return Ok(left.value);
        };

        let h = right.time - left.time;
        let u = (time - left.time) / h;
        let mut result = [0.; 4];
        meter.count_decode((self.width as u64) * 32)?;
        for (i, out) in result.iter_mut().enumerate().take(self.width) {
            let a = component(&left.value, i)?;
            let b = component(&right.value, i)?;
            let outgoing = component(&left.outgoing, i)?;
            let incoming = component(&right.incoming, i)?;
            *out = if outgoing.is_infinite() || incoming.is_infinite() {
                a
            } else if left.out_weight.is_some() || right.in_weight.is_some() {
                let w0 = handle(left.out_weight.as_ref(), i)?;
                let w1 = handle(right.in_weight.as_ref(), i)?;
                weighted([a, b], [outgoing, incoming], [w0, w1], h, u, meter)?
            } else {
                hermite(a, b, outgoing, incoming, h, u)
            };
        }
        Ok(result)
    }
}

fn component(values: &[f64; 4], index: usize) -> Result<f64, AnimError> {
    values
        .get(index)
        .copied()
        .ok_or(AnimError::InvalidCurveKeys)
}

fn handle(values: Option<&[f64; 4]>, index: usize) -> Result<f64, AnimError> {
    values.map_or(Ok(1. / 3.), |values| component(values, index))
}

fn text(dict: &Dict<'_, '_>, row: &Value, name: &'static str) -> Result<RawStr, AnimError> {
    match dict.required(row, name)? {
        Value::Str(s) => Ok(s.clone()),
        _ => Err(AnimError::Malformed(name)),
    }
}

fn fields(
    dict: &Dict<'_, '_>,
    row: &Value,
    name: &'static str,
    width: usize,
) -> Result<[f64; 4], AnimError> {
    if width == 1 {
        return Ok([dict.float(row, name)?, 0., 0., 0.]);
    }
    let data = dict.required(row, name)?;
    let mut values = [0.; 4];
    for (out, axis) in values.iter_mut().zip(["x", "y", "z", "w"]).take(width) {
        *out = dict.float(data, axis)?;
    }
    Ok(values)
}

fn weight(
    dict: &Dict<'_, '_>,
    row: &Value,
    name: &'static str,
    width: usize,
    active: bool,
) -> Result<Option<[f64; 4]>, AnimError> {
    let values = fields(dict, row, name, width)?;
    if active
        && values
            .into_iter()
            .take(width)
            .any(|x| !(0. ..=1.).contains(&x))
    {
        return Err(AnimError::UnsupportedTangentMode);
    }
    Ok(active.then_some(values))
}

fn key(dict: &Dict<'_, '_>, row: &Value, width: usize, modern: bool) -> Result<Key, AnimError> {
    let time = dict.float(row, "time")?;
    let value = fields(dict, row, "value", width)?;
    let incoming = fields(dict, row, "inSlope", width)?;
    let outgoing = fields(dict, row, "outSlope", width)?;
    if !time.is_finite() || value.iter().any(|x| !x.is_finite()) {
        return Err(AnimError::InvalidCurveKeys);
    }
    if incoming.iter().chain(&outgoing).any(|x| x.is_nan()) {
        return Err(AnimError::UnsupportedTangentMode);
    }
    // Component derivatives are defined for the ordinary quaternion layout.
    // Weighted/stepped quaternion tangents need their own playback evidence.
    if width == 4 {
        normalize(value)?;
        if incoming.iter().chain(&outgoing).any(|x| x.is_infinite()) {
            return Err(AnimError::UnsupportedTangentMode);
        }
    }

    let mode = dict.field(row, "weightedMode")?;
    let (in_weight, out_weight) = match (modern, mode) {
        (true, Some(mode)) => {
            let mode = int(mode, "weightedMode")?;
            if !(0..=3).contains(&mode) || (width == 4 && mode != 0) {
                return Err(AnimError::UnsupportedTangentMode);
            }
            (
                weight(dict, row, "inWeight", width, mode & 1 != 0)?,
                weight(dict, row, "outWeight", width, mode & 2 != 0)?,
            )
        }
        (false, None)
            if dict.field(row, "inWeight")?.is_none()
                && dict.field(row, "outWeight")?.is_none() =>
        {
            (None, None)
        }
        _ => return Err(AnimError::UnsupportedTangentMode),
    };
    Ok(Key {
        time,
        value,
        incoming,
        outgoing,
        in_weight,
        out_weight,
    })
}

fn curve(
    dict: &Dict<'_, '_>,
    row: &Value,
    property: &Property,
    version: (u16, u16),
) -> Result<Curve, AnimError> {
    if property.arity() == 1 {
        if dict.pointer(dict.required(row, "script")?)?.path_id != 0 {
            return Err(AnimError::UnsupportedBindingTarget);
        }
        if dict
            .field(row, "flags")?
            .is_some_and(|value| value.as_i64() != Some(0))
        {
            return Err(AnimError::UnsupportedClipLayout);
        }
    }
    let data = dict.required(row, "curve")?;
    let pre = dict.int(data, "m_PreInfinity")?;
    let post = dict.int(data, "m_PostInfinity")?;
    let order = if matches!(property, Property::Transform(TransformProperty::Euler)) {
        let raw = dict
            .field(data, "m_RotationOrder")?
            .map(|v| int(v, "m_RotationOrder"))
            .transpose()?;
        Some(EulerOrder::read(raw)?)
    } else {
        None
    };
    let rows = dict.list(dict.required(data, "m_Curve")?, "m_Curve")?;
    if rows.is_empty() {
        return Err(AnimError::InvalidCurveKeys);
    }
    let mut keys = MeteredVec::<Key>::with_capacity(dict.meter, rows.len())?;
    let width = property.arity();
    for row in rows {
        let key = key(dict, row, width, version >= (2018, 0))?;
        if keys
            .as_slice()
            .last()
            .is_some_and(|last| last.time >= key.time)
        {
            return Err(AnimError::InvalidCurveKeys);
        }
        keys.push(key)?;
    }
    Ok(Curve {
        order,
        keys,
        width,
        pre,
        post,
    })
}

fn float_property(dict: &Dict<'_, '_>, row: &Value) -> Result<Option<Property>, AnimError> {
    use TransformProperty as T;
    let attribute = text(dict, row, "attribute")?;
    let class = dict.int(row, "classID")?;
    let property = match (class, attribute.as_bytes()) {
        (137, name) if name.starts_with(b"blendShape.") => Property::Morph(attribute),
        (4 | 224, name) => {
            let (kind, suffix) = if let Some(suffix) = name.strip_prefix(b"m_LocalPosition.") {
                (T::Translation, suffix)
            } else if let Some(suffix) = name.strip_prefix(b"m_LocalScale.") {
                (T::Scale, suffix)
            } else if name.starts_with(b"m_LocalRotation.") || name.starts_with(b"localEulerAngles")
            {
                return Ok(Some(Property::RotationComponent));
            } else {
                return Ok(None);
            };
            let axis = match suffix {
                b"x" => 0,
                b"y" => 1,
                b"z" => 2,
                _ => return Err(AnimError::UnsupportedBindingTarget),
            };
            Property::Component(kind, axis)
        }
        _ => return Ok(None),
    };
    Ok(Some(property))
}

#[derive(Default)]
pub(crate) struct Metadata {
    pub present: bool,
    pub span: Option<(f64, f64)>,
}

impl Metadata {
    fn read(&mut self, dict: &Dict<'_, '_>, row: &Value, pointer: bool) -> Result<(), AnimError> {
        let raw = dict.required(row, "curve")?;
        let data = if pointer {
            raw
        } else {
            dict.required(raw, "m_Curve")?
        };
        let keys = dict.list(data, "curve")?;
        let mut previous = None;
        for key in keys {
            let time = dict.float(key, "time")?;
            if !time.is_finite() || previous.is_some_and(|last| last >= time) {
                return Err(AnimError::InvalidCurveKeys);
            }
            if pointer {
                dict.pointer(dict.required(key, "value")?)?;
            } else {
                dict.float(key, "value")?;
            }
            previous = Some(time);
            self.present = true;
            add_bounds(&mut self.span, (time, time));
        }
        Ok(())
    }
}

pub(super) fn add_bounds(span: &mut Option<(f64, f64)>, (start, stop): (f64, f64)) {
    *span = Some(span.map_or((start, stop), |(lo, hi)| (lo.min(start), hi.max(stop))));
}

pub(crate) fn visit_runtime<'a>(
    dict: &Dict<'a, '_>,
    mut metadata: Option<&mut Metadata>,
    mut visit: impl FnMut(Candidate<'a>) -> Result<(), AnimError>,
) -> Result<(), AnimError> {
    use ClipField as F;
    use TransformProperty as T;
    let compressed = dict
        .root("m_Compressed")?
        .map(|v| flag(v, "m_Compressed"))
        .transpose()?;
    if compressed.is_none() && !dict.rows(F::CompressedRotation.name())?.is_empty() {
        return Err(AnimError::UnsupportedClipLayout);
    }
    for (field, property, priority) in [
        (F::CompressedRotation, Some(T::Quaternion), 0),
        (F::Rotation, Some(T::Quaternion), 1),
        (F::Euler, Some(T::Euler), 2),
        (F::Position, Some(T::Translation), 0),
        (F::Scale, Some(T::Scale), 0),
        (F::Float, None, 3),
    ] {
        if field == F::CompressedRotation && compressed != Some(true) {
            continue;
        }
        for (ordinal, row) in dict.rows(field.name())?.iter().enumerate() {
            let property = match property {
                Some(property) => Property::Transform(property),
                None => {
                    if let Some(property) = float_property(dict, row)? {
                        property
                    } else {
                        if let Some(metadata) = metadata.as_deref_mut() {
                            metadata.read(dict, row, false)?;
                        }
                        continue;
                    }
                }
            };
            let path = text(
                dict,
                row,
                if field == F::CompressedRotation {
                    "m_Path"
                } else {
                    "path"
                },
            )?;
            visit(Candidate {
                property,
                path,
                priority,
                source: ClipSource { field, ordinal },
                row,
            })?;
        }
    }
    if let Some(metadata) = metadata {
        for row in dict.rows(F::PPtr.name())? {
            metadata.read(dict, row, true)?;
        }
    }
    Ok(())
}

pub(super) fn decode(
    dict: &Dict<'_, '_>,
    version: (u16, u16),
) -> Result<(MeteredVec<Explicit>, Metadata), AnimError> {
    let mut candidates = MeteredVec::new(dict.meter);
    let mut metadata = Metadata::default();
    let mut max_bytes = 0u64;
    visit_runtime(dict, Some(&mut metadata), |candidate| {
        let (_, _, attribute) = candidate.property.rank();
        max_bytes = max_bytes
            .max((candidate.path.as_bytes().len() as u64).saturating_add(attribute.len() as u64));
        candidates.push(candidate)?;
        Ok(())
    })?;
    let count = candidates.len() as u64;
    dict.meter.count_decode(
        count
            .saturating_mul(work_for_search(candidates.len()))
            .saturating_mul(max_bytes.saturating_add(8)),
    )?;
    candidates
        .as_mut_slice()
        .sort_unstable_by(|a, b| a.key().cmp(&b.key()).then(a.priority.cmp(&b.priority)));
    let mut tracks = MeteredVec::new(dict.meter);
    for group in candidates.as_slice().chunk_by(|a, b| a.key() == b.key()) {
        let Some((chosen, others)) = group.split_first() else {
            continue;
        };
        let property = match &chosen.property {
            Property::Transform(p) => Property::Transform(*p),
            Property::Component(p, axis) => Property::Component(*p, *axis),
            Property::RotationComponent => return Err(AnimError::UnsupportedBindingTarget),
            Property::Morph(attribute) => Property::Morph(attribute.clone()),
        };
        if others
            .first()
            .is_some_and(|other| other.priority == chosen.priority)
        {
            return Err(AnimError::AmbiguousPlaybackSource);
        }
        let data = if chosen.source.field == ClipField::CompressedRotation {
            Data::Packed(packed::read(dict, chosen.row)?)
        } else {
            Data::Curve(curve(dict, chosen.row, &property, version)?)
        };
        let track = Explicit {
            source: chosen.source,
            property,
            path: chosen.path.clone(),
            data,
        };
        add_bounds(&mut metadata.span, track.bounds()?);
        tracks.push(track)?;
    }
    Ok((tracks, metadata))
}
