// Ported from UnityPy 1.25.2 (MIT): UnityPyBoost/Mesh.cpp's unpack_vertexdata (the C++ extension shipped in UnityPy 1.25.2), and the struct.iter_unpack of helpers/MeshHelper.py's read_vertex_data. See THIRD_PARTY_LICENSES.
//! The strided gather: one channel's components out of the vertex data,
//! as UnityPyBoost copies them (`unpack_vertexdata`,
//! `UnityPyBoost/Mesh.cpp:23-210`), then unpacked as `read_vertex_data`
//! unpacks them (`struct.iter_unpack`, `helpers/MeshHelper.py:392`).
//!
//! **UnityPyBoost's arithmetic.** The arguments are C `uint32_t`s (Python's
//! `I` format masks an `int` to its low 32 bits, `UnityPyBoost/Mesh.cpp:36`),
//! and every offset is computed in them, so it wraps. Its one check is the
//! largest offset read, computed so (`:50-56`), against the data's length:
//! past it, `ValueError` "Vertex data access out of bounds"
//! ([`MeshError::OutOfBounds`]). Within it, the copy (`:66-75`) reads each
//! component at its own wrapped offset, and its output buffer is the
//! wrapped product of the count, the dimension and the component's size
//! (`:47`). Where nothing wraps that is a plain gather, and this is it,
//! exactly. Where the check passes but a component's wrapped offset lies
//! past the data, or the product wraps, UnityPyBoost reads or writes
//! outside its buffers, undefined behaviour: [`MeshError::WrappedAccess`]
//! refuses it before any output is returned (`PARITY.md` section 19.3): a
//! wrapped product before anything is read, a component past the data when
//! the gather reaches it, the components before it read into the output
//! that is then dropped. A stride that wraps to a step backwards
//! (`0xFFFFFFFC`) reads in bounds, and is read as UnityPyBoost reads it.
//!
//! **The swap and the unpack.** UnityPyBoost swaps each component of 2 or
//! 4 bytes when asked (`Mesh.cpp:77-96`, which the handler asks for
//! little-endian data, `helpers/MeshHelper.py:363`), and the handler reads
//! the result big-endian (`>`), so a swapped component is read
//! little-endian and an unswapped one big-endian. `struct`'s codes give
//! Python `int`s and `float`s, whose values here are written from the IEEE
//! 754 formats, matching `CPython` 3.11's results (the oracle's): `f`, a
//! binary32 widened to binary64 as x86's `cvtss2sd` widens it (a NaN keeps
//! its payload with its quiet bit set, which the oracle shows; [`widen`]),
//! and `e`, a binary16 widened exactly, its NaN the platform's `NAN` with
//! the half's sign and no payload ([`half`]). An oracle moved past Python
//! 3.11 needs these checked again: 3.14 changes how `PyFloat_Unpack4`
//! treats a NaN (gh-130317), which may change the bits [`widen`] matches
//! (`PARITY.md` section 19.3's row on the mesh handler's later phases' parts). A format of
//! no bytes (a dimension whose low four bits are 0) is `struct.error`
//! "cannot iteratively unpack with a struct of length 0".
//!
//! **Normalised** (`PARITY.md` section 19.1, Tier C): where the handler
//! asks, a channel in a `UNorm`, `SNorm` or byte colour format is read as
//! `float`s by its [`Norm`] rule, where `struct.iter_unpack` gives UnityPy
//! the integers (`crate::format`'s module docs).
//!
//! **Bounds.** The output's `f64` or `i64` values (or bytes) are charged to
//! the caller's meter, and then counted as decode work on it, before
//! anything is read (`docs/plans/extract.md` E59), so a refused charge
//! counts no work.

use std::sync::Arc;

use unityrs_core::limits::{Charge, Meter, MeteredVec};

use crate::error::{MeshError, StructError};
use crate::format::{Norm, StructCode};

/// One attribute: `len()` tuples of `dimension()` components, as
/// `struct.iter_unpack` (or the copy of a `Mesh`'s skin) gives them, the
/// values flat in tuple order. A compressed mesh's arrays are
/// `PackedBitVector.reshape`'s lists, whose last tuple may be cut short:
/// it is a tuple here too, of the components it holds.
#[derive(Debug)]
pub struct VertexArray {
    dimension: usize,
    values: Values,
    // Declared last: the charge is returned after the values are freed.
    _charge: Charge,
}

const _: () = crate::send_sync::<VertexArray>();

/// An attribute's values: Python `float`s, as `f64`, or `int`s, flat in
/// tuple order.
///
/// Not `Clone`: the values are charged to the meter that `process` was
/// given, for as long as the [`VertexArray`] holding them lives, and a copy
/// would hold memory no charge covers.
#[derive(Debug, PartialEq)]
#[non_exhaustive]
pub enum Values {
    /// `float`s (struct codes `f` and `e`; a skin's weights).
    Float(Vec<f64>),
    /// `int`s (the integer codes; a skin's bone indices).
    Int(Vec<i64>),
    /// Each value a `float` or an `int`, as Python holds it: a compressed
    /// mesh's normals, tangents and bone weights, whose `zeros` are `int`s
    /// beside the `float`s unpacked ([`crate::compressed`]).
    Mixed(Vec<Component>),
}

const _: () = crate::send_sync::<Values>();

impl VertexArray {
    /// An array of `values`, `dimension` to a tuple, holding the charge
    /// for their memory.
    pub(crate) fn new(dimension: usize, values: Values, charge: Charge) -> Self {
        Self {
            dimension,
            values,
            _charge: charge,
        }
    }

    /// The components of each tuple.
    #[must_use]
    pub fn dimension(&self) -> usize {
        self.dimension
    }

    /// The number of values, flat.
    fn values_len(&self) -> usize {
        match &self.values {
            Values::Float(v) => v.len(),
            Values::Int(v) => v.len(),
            Values::Mixed(v) => v.len(),
        }
    }

    /// The number of tuples, a last one cut short counted: Python's
    /// `len()` of the list UnityPy holds. No values at dimension 0 is no
    /// tuple.
    #[must_use]
    pub fn len(&self) -> usize {
        if self.dimension == 0 {
            return 0;
        }
        self.values_len().div_ceil(self.dimension)
    }

    /// The components tuple `tuple` holds: [`Self::dimension`], or fewer
    /// for a last tuple cut short; `None` past the tuples.
    #[must_use]
    pub fn tuple_len(&self, tuple: usize) -> Option<usize> {
        if tuple >= self.len() {
            return None;
        }
        let from = tuple.checked_mul(self.dimension)?;
        Some((self.values_len() - from).min(self.dimension))
    }

    /// Whether it holds no tuple.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The values, flat.
    #[must_use]
    pub fn values(&self) -> &Values {
        &self.values
    }

    /// Component `component` of tuple `tuple`, as a Python `float` or
    /// `int`, whichever form holds it; `None` past the tuples
    /// ([`Self::len`]) or past the tuple's components
    /// ([`Self::tuple_len`]: a last tuple cut short holds fewer), where
    /// Python's index raises `IndexError`.
    #[must_use]
    pub fn component(&self, tuple: usize, component: usize) -> Option<Component> {
        if component >= self.tuple_len(tuple)? {
            return None;
        }
        let at = tuple.checked_mul(self.dimension)?.checked_add(component)?;
        match &self.values {
            Values::Float(v) => v.get(at).copied().map(Component::Float),
            Values::Int(v) => v.get(at).copied().map(Component::Int),
            Values::Mixed(v) => v.get(at).copied(),
        }
    }
}

/// One component of a [`VertexArray`]'s tuple ([`VertexArray::component`]).
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum Component {
    /// A Python `float`.
    Float(f64),
    /// A Python `int`.
    Int(i64),
}

const _: () = crate::send_sync::<Component>();

impl Component {
    /// The value as a Python `float`: an `int` converted as `float(i)`
    /// converts it, to the nearest `f64`, ties to even.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // Python's own rounding (doc above).
    pub fn as_f64(self) -> f64 {
        match self {
            Self::Float(f) => f,
            Self::Int(i) => i as f64,
        }
    }

    /// Python's truth value: `0`, `0.0` and `-0.0` are false, every other
    /// value (a NaN included) true.
    #[must_use]
    pub fn is_truthy(self) -> bool {
        match self {
            Self::Float(f) => f != 0.0,
            Self::Int(i) => i != 0,
        }
    }
}

/// The arguments of `UnityPyBoost.unpack_vertexdata` after Python's
/// conversion to C (`UnityPyBoost/Mesh.cpp:26-43`): each a `uint32_t`, the
/// component size an `int` that the arithmetic converts to one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Gather {
    /// `componentByteSize`: 1, 2 or 4 from a struct code.
    pub component_byte_size: u32,
    /// `m_VertexCount`.
    pub vertex_count: u32,
    /// `m_StreamOffset`.
    pub stream_offset: u32,
    /// `m_StreamStride`.
    pub stream_stride: u32,
    /// `m_ChannelOffset`.
    pub channel_offset: u32,
    /// `m_ChannelDimension`: the channel's dimension, its low four bits.
    pub channel_dimension: u32,
    /// `swap`: swap each component of 2 or 4 bytes.
    pub swap: bool,
}

const _: () = crate::send_sync::<Gather>();

impl Gather {
    /// The output's length, and a check that every component read lies in
    /// `len` bytes of data (module docs): the bounds check UnityPyBoost
    /// makes, then the reads and the buffer it would make past it.
    fn check(&self, len: usize) -> Result<u64, MeshError> {
        let cbs = self.component_byte_size;
        let dim = self.channel_dimension;
        // UnityPy Mesh.cpp:50: every term a `uint32_t`, so it wraps.
        let max_access = self
            .vertex_count
            .wrapping_sub(1)
            .wrapping_mul(self.stream_stride)
            .wrapping_add(self.channel_offset)
            .wrapping_add(self.stream_offset)
            .wrapping_add(cbs.wrapping_mul(dim.wrapping_sub(1)))
            .wrapping_add(cbs);
        if u64::from(max_access) > u64::try_from(len).unwrap_or(u64::MAX) {
            return Err(MeshError::OutOfBounds);
        }
        // UnityPy Mesh.cpp:47: the buffer is the wrapped product; past it
        // the copy writes outside it.
        let wrapped = self.vertex_count.wrapping_mul(dim).wrapping_mul(cbs);
        let exact = u128::from(self.vertex_count) * u128::from(dim) * u128::from(cbs);
        if exact != u128::from(wrapped) {
            return Err(MeshError::WrappedAccess);
        }
        Ok(u64::from(wrapped))
    }

    /// The offset of vertex `v`'s component `d`, wrapped as
    /// UnityPyBoost's `uint32_t`s wrap (`UnityPy Mesh.cpp:68`, `:71`).
    fn offset(&self, v: u32, d: u32) -> u32 {
        self.stream_offset
            .wrapping_add(self.channel_offset)
            .wrapping_add(self.stream_stride.wrapping_mul(v))
            .wrapping_add(self.component_byte_size.wrapping_mul(d))
    }

    /// Visits each component's bytes in output order, after the checks;
    /// a component outside the data is [`MeshError::WrappedAccess`].
    fn each(
        &self,
        data: &[u8],
        mut visit: impl FnMut(&[u8]) -> Result<(), MeshError>,
    ) -> Result<(), MeshError> {
        let cbs = usize::try_from(self.component_byte_size).unwrap_or(usize::MAX);
        for v in 0..self.vertex_count {
            for d in 0..self.channel_dimension {
                let at = usize::try_from(self.offset(v, d)).unwrap_or(usize::MAX);
                let src = at
                    .checked_add(cbs)
                    .and_then(|end| data.get(at..end))
                    .ok_or(MeshError::WrappedAccess)?;
                visit(src)?;
            }
        }
        Ok(())
    }
}

/// UnityPyBoost's output: the gathered components, each of 2 or 4 bytes
/// swapped when asked.
#[derive(Debug)]
pub struct ComponentBytes {
    bytes: Vec<u8>,
    // Declared last: the charge is returned after the bytes are freed.
    _charge: Charge,
}

const _: () = crate::send_sync::<ComponentBytes>();

impl ComponentBytes {
    /// The bytes.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}

/// `UnityPyBoost.unpack_vertexdata(data, ...)` (`UnityPyBoost/Mesh.cpp:23-210`):
/// the gather, its check and its swap (module docs), the output charged to
/// `meter` before it is allocated and counted as decode work.
pub fn unpack_vertexdata(
    data: &[u8],
    args: &Gather,
    meter: &Arc<Meter>,
) -> Result<ComponentBytes, MeshError> {
    let length = args.check(data.len())?;
    let n = usize::try_from(length).map_err(|_| MeshError::WrappedAccess)?;
    // Charged, then counted: a refused charge counts no work.
    let mut out = MeteredVec::<u8>::with_capacity(meter, n)?;
    meter.count_decode(length)?;
    let swap = args.swap && matches!(args.component_byte_size, 2 | 4);
    // UnityPy Mesh.cpp:66-75 copies each component. A component of no bytes
    // (a size no struct code has, so only a direct caller's) copies nothing
    // wherever it points: with no output there is nothing to read, and the
    // loop, up to 2^32 x 15 steps of no work, does not run.
    if length == 0 {
        let (bytes, charge) = out.into_parts();
        return Ok(ComponentBytes {
            bytes,
            _charge: charge,
        });
    }
    args.each(data, |src| {
        // UnityPy Mesh.cpp:77-96: each component of 2 or 4 bytes reversed.
        if swap {
            for &b in src.iter().rev() {
                out.push(b)?;
            }
        } else {
            out.extend_from_slice(src)?;
        }
        Ok(())
    })?;
    let (bytes, charge) = out.into_parts();
    Ok(ComponentBytes {
        bytes,
        _charge: charge,
    })
}

/// The gather and `struct.iter_unpack(f">{dimension}{code}", ...)` in one
/// pass (`helpers/MeshHelper.py:366-392`), with no buffer between: the
/// values of one channel, charged to `meter` before they are allocated.
/// With `norm`, each integer is normalised to a `float` by its rule
/// (`crate::format`'s module docs, Tier C), where UnityPy keeps the
/// integer.
pub(crate) fn unpack_channel(
    data: &[u8],
    args: &Gather,
    code: StructCode,
    norm: Option<Norm>,
    meter: &Arc<Meter>,
) -> Result<VertexArray, MeshError> {
    let length = args.check(data.len())?;
    let dimension = usize::try_from(args.channel_dimension).unwrap_or(usize::MAX);
    // UnityPy MeshHelper.py:392: a format of no bytes refuses to iterate,
    // whatever the buffer (UnityPyBoost has returned `b""`).
    if dimension == 0 {
        return Err(MeshError::Struct(StructError::ZeroLength));
    }
    let n = usize::try_from(u64::from(args.vertex_count) * u64::from(args.channel_dimension))
        .map_err(|_| MeshError::WrappedAccess)?;
    // The swapped bytes read big-endian are the source read little-endian
    // (module docs); a component of one byte is never swapped. Each output
    // is charged, then counted: a refused charge counts no work.
    let little = args.swap;
    if code.is_float() || norm.is_some() {
        let mut out = MeteredVec::<f64>::with_capacity(meter, n)?;
        meter.count_decode(length)?;
        args.each(data, |src| {
            out.push(match norm {
                // PARITY.md section 19.1: UnityPy keeps the integer.
                Some(rule) if !code.is_float() => rule.value(code, int(code, src, little)),
                _ => float(code, src, little),
            })?;
            Ok(())
        })?;
        let (v, charge) = out.into_parts();
        Ok(VertexArray::new(dimension, Values::Float(v), charge))
    } else {
        let mut out = MeteredVec::<i64>::with_capacity(meter, n)?;
        meter.count_decode(length)?;
        args.each(data, |src| {
            out.push(int(code, src, little))?;
            Ok(())
        })?;
        let (v, charge) = out.into_parts();
        Ok(VertexArray::new(dimension, Values::Int(v), charge))
    }
}

/// A component's bytes in little-endian order: as they are when read
/// little-endian, reversed when read big-endian.
fn le(src: &[u8], little: bool) -> [u8; 4] {
    let mut a = [0u8; 4];
    if little {
        a.iter_mut().zip(src).for_each(|(o, b)| *o = *b);
    } else {
        a.iter_mut()
            .zip(src.iter().rev())
            .for_each(|(o, b)| *o = *b);
    }
    a
}

/// An integer code's value (`B b H h I i`).
fn int(code: StructCode, src: &[u8], little: bool) -> i64 {
    let [a, b, c, d] = le(src, little);
    match code {
        StructCode::I8 => i64::from(i8::from_le_bytes([a])),
        StructCode::U16 => i64::from(u16::from_le_bytes([a, b])),
        StructCode::I16 => i64::from(i16::from_le_bytes([a, b])),
        StructCode::U32 => i64::from(u32::from_le_bytes([a, b, c, d])),
        StructCode::I32 => i64::from(i32::from_le_bytes([a, b, c, d])),
        // `B`; the float codes never come here.
        StructCode::U8 | StructCode::F16 | StructCode::F32 => i64::from(a),
    }
}

/// A float code's value (`f`, `e`).
fn float(code: StructCode, src: &[u8], little: bool) -> f64 {
    let [a, b, c, d] = le(src, little);
    match code {
        StructCode::F16 => half(u16::from_le_bytes([a, b])),
        _ => widen(u32::from_le_bytes([a, b, c, d])),
    }
}

/// A C `float`'s bits widened to `double` as x86's `cvtss2sd` widens them:
/// exactly, and a NaN with its payload kept and its quiet bit set (module
/// docs). A typetree `float`'s value is widened the same way, by
/// UnityPyBoost's `PyFloat_FromDouble` of it.
pub(crate) fn widen(bits: u32) -> f64 {
    let f = f32::from_bits(bits);
    if f.is_nan() {
        let sign = u64::from(bits >> 31) << 63;
        let payload = u64::from((bits & 0x007f_ffff) | 0x0040_0000);
        f64::from_bits(sign | 0x7ff0_0000_0000_0000 | (payload << 29))
    } else {
        f64::from(f)
    }
}

/// An IEEE 754 binary16's bits as the binary64 of the same value: the
/// sign kept, a normal half's exponent rebiased from 15 to 1023 with its
/// ten fraction bits at the top of the 52, a subnormal (and zero) its
/// fraction times 2^-24, which binary64 holds exactly, and the infinities
/// kept. A NaN is the quiet NaN `0x7ff8000000000000` with the half's sign
/// and no payload, matching `CPython` 3.11's `struct` `e`, which returns the
/// platform's `NAN` for one.
pub(crate) fn half(bits: u16) -> f64 {
    let sign = u64::from(bits >> 15) << 63;
    let exponent = u64::from((bits >> 10) & 0x1f);
    let fraction = bits & 0x03ff;
    let magnitude = match exponent {
        0 => (f64::from(fraction) * 2f64.powi(-24)).to_bits(),
        0x1f if fraction == 0 => f64::INFINITY.to_bits(),
        0x1f => 0x7ff8_0000_0000_0000,
        _ => ((exponent + 1023 - 15) << 52) | (u64::from(fraction) << 42),
    };
    f64::from_bits(sign | magnitude)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use unityrs_core::limits::{Charge, Meter};

    use super::{Component, Gather, Values, VertexArray, half, unpack_vertexdata, widen};
    use crate::error::MeshError;

    fn args(vc: u32, soff: u32, stride: u32, choff: u32, dim: u32, cbs: u32) -> Gather {
        Gather {
            component_byte_size: cbs,
            vertex_count: vc,
            stream_offset: soff,
            stream_stride: stride,
            channel_offset: choff,
            channel_dimension: dim,
            swap: true,
        }
    }

    #[test]
    fn halves_and_floats_widen_as_the_oracle_does() {
        // The oracle's bits (Python 3.11 on x86-64).
        assert_eq!(half(0x7c00), f64::INFINITY);
        assert_eq!(half(0x7c01).to_bits(), 0x7ff8_0000_0000_0000);
        assert_eq!(half(0x7e00).to_bits(), 0x7ff8_0000_0000_0000);
        assert_eq!(half(0xfc01).to_bits(), 0xfff8_0000_0000_0000);
        assert_eq!(half(0x3c00), 1.0);
        assert_eq!(half(0x0001), 2f64.powi(-24));
        assert_eq!(half(0x8000).to_bits(), (-0.0f64).to_bits());
        assert_eq!(widen(0x7f80_0001).to_bits(), 0x7ff8_0000_2000_0000);
        assert_eq!(widen(0xffa0_0005).to_bits(), 0xfffc_0000_a000_0000);
        assert_eq!(widen(0x7fc0_0000).to_bits(), 0x7ff8_0000_0000_0000);
    }

    #[test]
    fn every_half_widens_to_its_value() {
        // Against the half's value computed apart, (1 + f/1024) x 2^(e-15),
        // for every finite half.
        for bits in 0..=u16::MAX {
            let (e, f) = (i32::from((bits >> 10) & 0x1f), f64::from(bits & 0x3ff));
            if e == 0x1f {
                continue;
            }
            let x = if e == 0 {
                f / 1024.0 * 2f64.powi(-14)
            } else {
                (1.0 + f / 1024.0) * 2f64.powi(e - 15)
            };
            let want = if bits & 0x8000 != 0 { -x } else { x };
            assert_eq!(half(bits).to_bits(), want.to_bits(), "{bits:#06x}");
        }
    }

    #[test]
    fn a_backward_stride_reads_as_unitypyboost_reads_it() {
        // Stride -4: vertices at 8, 4, 0. The check's sum wraps to 12.
        let data: Vec<u8> = (0..12).collect();
        let a = args(3, 8, 0xFFFF_FFFC, 0, 1, 4);
        let out = unpack_vertexdata(&data, &a, &Meter::unlimited()).unwrap();
        assert_eq!(out.as_slice(), &[11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0]);
    }

    #[test]
    fn a_wrapped_offset_past_the_data_is_refused() {
        // The offset 0xFFFFFFF0 passes the wrapped check of 16 bytes, and
        // UnityPyBoost would read 4 GiB past its buffer.
        let data = [0u8; 16];
        let a = args(8, 0xFFFF_FFF0, 4, 0, 1, 4);
        let e = unpack_vertexdata(&data, &a, &Meter::unlimited()).unwrap_err();
        assert!(matches!(e, MeshError::WrappedAccess), "{e:?}");
        let a = args(8, 0, 4, 0, 1, 4);
        let e = unpack_vertexdata(&data, &a, &Arc::clone(&Meter::unlimited())).unwrap_err();
        assert!(matches!(e, MeshError::OutOfBounds), "{e:?}");
    }

    fn array(dimension: usize, values: Values) -> VertexArray {
        VertexArray::new(dimension, values, Charge::zero(&Meter::unlimited()))
    }

    #[test]
    fn a_component_is_a_python_float_or_int_by_the_form_that_holds_it() {
        let floats = array(2, Values::Float(vec![0.5, -1.0, 2.0, 3.5]));
        assert_eq!(floats.len(), 2);
        assert_eq!(floats.component(0, 1), Some(Component::Float(-1.0)));
        assert_eq!(floats.component(1, 0), Some(Component::Float(2.0)));
        let ints = array(3, Values::Int(vec![1, -2, 3, i64::MAX, 0, -7]));
        assert_eq!(ints.component(1, 0), Some(Component::Int(i64::MAX)));
        assert_eq!(ints.component(0, 2), Some(Component::Int(3)));
        // `float(i)`: the nearest f64, ties to even.
        assert_eq!(
            Component::Int(i64::MAX).as_f64(),
            9_223_372_036_854_775_808.0
        );
        assert_eq!(
            Component::Int((1 << 53) + 1).as_f64(),
            9_007_199_254_740_992.0
        );
        assert_eq!(Component::Float(-0.25).as_f64(), -0.25);
    }

    #[test]
    fn a_component_past_the_tuples_or_its_tuple_is_none() {
        let a = array(2, Values::Float(vec![1.0, 2.0, 3.0]));
        // Three values of dimension 2: a whole tuple and one cut short, as
        // `reshape` gives them.
        assert_eq!(a.len(), 2);
        assert_eq!(
            (a.tuple_len(0), a.tuple_len(1), a.tuple_len(2)),
            (Some(2), Some(1), None)
        );
        assert_eq!(a.component(0, 2), None);
        assert_eq!(a.component(0, 1), Some(Component::Float(2.0)));
        assert_eq!(a.component(1, 0), Some(Component::Float(3.0)));
        assert_eq!(a.component(1, 1), None);
        assert_eq!(a.component(2, 0), None);
        assert_eq!(a.component(usize::MAX, 1), None);
        assert_eq!(a.component(usize::MAX / 2 + 1, 0), None);
        let empty = array(0, Values::Int(vec![1, 2]));
        assert_eq!((empty.len(), empty.component(0, 0)), (0, None));
    }

    #[test]
    fn truth_is_python_s() {
        assert!(!Component::Float(0.0).is_truthy());
        assert!(!Component::Float(-0.0).is_truthy());
        assert!(Component::Float(f64::NAN).is_truthy());
        assert!(Component::Float(f64::MIN_POSITIVE / 2.0).is_truthy());
        assert!(!Component::Int(0).is_truthy());
        assert!(Component::Int(-1).is_truthy());
    }
}
