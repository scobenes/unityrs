// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/MeshHelper.py's MeshHandler.decompress_compressed_mesh and normalize. See THIRD_PARTY_LICENSES.
//! `decompress_compressed_mesh()` (`helpers/MeshHelper.py:468-628`): a
//! Mesh's `m_CompressedMesh`, unpacked ([`crate::packed`]) into the
//! handler's arrays, in UnityPy's order, each over what the vertex data set.
//!
//! 1. The vertex count, `m_Vertices.m_NumItems // 3`, always; then the
//!    positions when it has items, 3 a tuple (the last cut short when the
//!    count is not a multiple of 3, as `reshape` cuts it).
//! 2. The UVs when `m_UV` has items: with a non-zero `m_UVInfo`, each of
//!    the 8 channels whose four bits hold 4 (`kUVChannelExists`), of
//!    `1 + (bits & 3)` components, `count * dim` items from an offset that
//!    grows by each channel's items (Tier C, below); otherwise channel 0,
//!    2 a tuple, and channel 1 after it when the vector holds 4 items a
//!    vertex.
//! 3. Below 5, `m_BindPoses`: each matrix's 16 floats, row by row
//!    ([`crate::Processed::bind_pose`]).
//! 4. The normals: pairs `(x, y)`, `z = sqrt(1 - x*x - y*y)`, or, where
//!    that is negative, `(x, y, 0)` normalised ([`normalize`]); `z`
//!    negated (`* -1`) where the sign is 0. One per vertex, as `zip`
//!    stops at the shortest of the pairs, the signs and the vertex count;
//!    the vertices past it stay `int` zeros.
//! 5. The tangents, the same with a pair of signs each: `z` negated
//!    (unary `-`) where the first is 0, `w` 1.0 where the second is above
//!    0, else -1.0.
//! 6. From 5, `m_FloatColors`, 4 a tuple. 7. The skin, below. 8. The
//!    triangles' indices, which replace the index buffer. 9. The packed
//!    colours, RGBA bytes from the top, each divided by 255.
//!
//! **Types.** Python's: the zeros of `zeros` are `int`s, so the normals,
//! tangents and bone weights are [`Values::Mixed`]; the bone indices `int`s.
//!
//! **The skin** (`:573-612`). Weights in 31sts, read with a bone index
//! each until a vertex's sum reaches 31; a vertex whose three weights stop
//! short gets a fourth weight and a fourth bone index. A vertex past the
//! vertex count is `IndexError`, a fourth bone index past the indices
//! `StopIteration`, and the loop stops where either list does (`zip`).
//!
//! **Tier C** (`PARITY.md` section 19.1, `docs/DESIGN.md` section 3).
//! UnityPy sets the UV offset (`uvSrcOffset = uvDim * m_VertexCount`,
//! `:504`), so a third channel starts where the second did; here it adds.
//! UnityPy gives a fourth weight of `1 - sum`, `sum` counted in 31sts
//! (`:604`), an `int` far below zero; here it is `(31 - sum) / 31`.
//! `normalize` squares by `v**2` (`:64`), `CPython`'s `float_pow` through
//! the C library's `pow`, which is not correctly rounded: the oracle's
//! glibc 2.36 rounds the last bit otherwise than `v * v` for about one
//! value in a thousand, and another host's `pow` may round otherwise
//! again, so UnityPy's value is the host's; here the square is the
//! correctly rounded IEEE 754 product `v * v` (equal wherever the square
//! is exact or `pow` rounds correctly). A finite component whose square
//! passes the largest `float` is `OverflowError`, as there. Each
//! divergence is pinned by `tests/compressed.rs`, and masked in
//! `tests/oracle.rs`'s replay.
//!
//! **Bounds.** Every array is charged to the caller's meter before it is
//! allocated, the unpacked items as [`crate::packed`] charges them, and
//! each loop counts a unit an element of its output before it runs.

use std::sync::Arc;

use unityrs_core::limits::{Charge, Meter, MeteredVec};

use crate::Version;
use crate::error::{IndexKind, MeshError};
use crate::packed::{add, div, mul, size, sub, unpack_floats, unpack_ints};
use crate::unpack::{Component, Values, VertexArray};
use crate::vertex::{Attributes, field};
use crate::view::{COMPRESSED_MESH, Obj, PACKED_BIT_VECTOR};

/// What `decompress_compressed_mesh` leaves beside the arrays: the vertex
/// count, and the triangles' indices when it unpacked them.
#[derive(Debug)]
pub(crate) struct Decompressed {
    pub(crate) vertex_count: u64,
    pub(crate) triangles: Option<(Vec<u32>, Charge)>,
}

/// The `PackedBitVector` `name` of `cm`: an `AttributeError` where object
/// mode built an `UnknownObject` without it.
fn vector<'a>(cm: &Obj<'a>, name: &'static str) -> Result<Obj<'a>, MeshError> {
    cm.class(name, &PACKED_BIT_VECTOR)?
        .ok_or(MeshError::NoAttribute(name))
}

/// `m_NumItems`.
fn items(v: &Obj<'_>) -> Result<u64, MeshError> {
    field(v, "m_NumItems", u64::from(u32::MAX))
}

/// `x and x.m_NumItems > 0` of an optional vector: the vector, when so.
fn held<'a>(cm: &Obj<'a>, name: &'static str) -> Result<Option<Obj<'a>>, MeshError> {
    match cm.class(name, &PACKED_BIT_VECTOR)? {
        Some(v) if items(&v)? > 0 => Ok(Some(v)),
        _ => Ok(None),
    }
}

/// `unpack_floats(v, start, count, shape=(dim,))`.
fn floats(
    v: &Obj<'_>,
    start: u64,
    count: Option<u64>,
    dim: usize,
    meter: &Arc<Meter>,
) -> Result<VertexArray, MeshError> {
    let (values, charge) = unpack_floats(v, start, count, meter)?.into_parts();
    Ok(VertexArray::new(dim, Values::Float(values), charge))
}

/// `zeros(rows, dim)`, flat: `int` zeros, charged, then counted.
fn zeros(rows: u64, dim: u64, meter: &Arc<Meter>) -> Result<MeteredVec<Component>, MeshError> {
    let n = rows.saturating_mul(dim);
    let mut out = MeteredVec::<Component>::with_capacity(meter, size(n))?;
    meter.count_decode(n)?;
    for _ in 0..n {
        out.push(Component::Int(0))?;
    }
    Ok(out)
}

/// Element `i` of `v`'s `pair`-long tuples (`reshape`'s slice): the slice,
/// cut short at the end.
fn pair<T>(v: &[T], i: usize) -> &[T] {
    let at = i.saturating_mul(2);
    v.get(at..v.len().min(at.saturating_add(2))).unwrap_or(&[])
}

/// `x, y = pair`: `ValueError` where it is cut short.
fn two<T: Copy>(v: &[T]) -> Result<(T, T), MeshError> {
    match v {
        [x, y] => Ok((*x, *y)),
        _ => Err(MeshError::Unpack {
            expected: 2,
            got: u8::try_from(v.len()).unwrap_or(u8::MAX),
        }),
    }
}

/// `v**2` of a `float`, correctly rounded (module docs, Tier C): a NaN or
/// an infinity as `float_pow`'s special cases give it, a finite square
/// past the largest `float` `OverflowError`.
fn square(v: f64) -> Result<f64, MeshError> {
    if v.is_nan() {
        return Ok(v);
    }
    let s = v * v;
    if s.is_infinite() && v.is_finite() {
        return Err(MeshError::Overflow);
    }
    Ok(s)
}

/// `normalize(x, y, 0)` (`helpers/MeshHelper.py:63-68`), the third
/// component the `int` 0: `sum` adds the squares in order to an `int` 0,
/// so the length is `sqrt(x*x + y*y)`; past `1e-5` each component times
/// `1.0 / length`, else three `int` zeros.
pub(crate) fn normalize(x: f64, y: f64) -> Result<[Component; 3], MeshError> {
    // UnityPy MeshHelper.py:64: `0 + x**2` is `x**2`; `+ 0**2` adds 0.0.
    let sx = square(x)?;
    let sy = square(y)?;
    let length = add(add(sx, sy), 0.0).sqrt();
    if length > 0.00001 {
        let inv = div(1.0, length);
        return Ok([
            Component::Float(mul(x, inv)),
            Component::Float(mul(y, inv)),
            Component::Float(mul(0.0, inv)),
        ]);
    }
    Ok([Component::Int(0); 3])
}

/// `1 - x * x - y * y`.
fn zsqr(x: f64, y: f64) -> f64 {
    sub(sub(1.0, mul(x, x)), mul(y, y))
}

/// `decompress_compressed_mesh()` (module docs): the arrays into `attrs`.
pub(crate) fn decompress_compressed_mesh(
    version: Version,
    mesh: &Obj<'_>,
    attrs: &mut Attributes,
    meter: &Arc<Meter>,
) -> Result<Decompressed, MeshError> {
    let cm = mesh
        .class("m_CompressedMesh", &COMPRESSED_MESH)?
        .ok_or_else(|| MeshError::malformed("m_CompressedMesh is None"))?;
    // UnityPy MeshHelper.py:476-479
    let vertices = vector(&cm, "m_Vertices")?;
    let count = items(&vertices)? / 3;
    if items(&vertices)? > 0 {
        attrs.vertices = Some(floats(&vertices, 0, None, 3, meter)?);
    }
    // UnityPy MeshHelper.py:482-513
    let uv = vector(&cm, "m_UV")?;
    if items(&uv)? > 0 {
        uvs(&cm, &uv, count, attrs, meter)?;
    }
    let major = version.major()?;
    // UnityPy MeshHelper.py:516-525
    if major < 5
        && let Some(poses) = held(&cm, "m_BindPoses")?
    {
        attrs.bind_pose = Some(floats(&poses, 0, None, 16, meter)?);
    }
    // UnityPy MeshHelper.py:528-544
    let normals = vector(&cm, "m_Normals")?;
    if items(&normals)? > 0 {
        attrs.normals = Some(decompress_normals(&cm, &normals, count, meter)?);
    }
    // UnityPy MeshHelper.py:547-565
    let tangents = vector(&cm, "m_Tangents")?;
    if items(&tangents)? > 0 {
        attrs.tangents = Some(decompress_tangents(&cm, &tangents, count, meter)?);
    }
    // UnityPy MeshHelper.py:568-571
    if major >= 5
        && let Some(colors) = held(&cm, "m_FloatColors")?
    {
        attrs.colors = Some(floats(&colors, 0, None, 4, meter)?);
    }
    // UnityPy MeshHelper.py:573-612
    let weights = vector(&cm, "m_Weights")?;
    if items(&weights)? > 0 {
        let (w, i) = decompress_skin(&cm, &weights, count, meter)?;
        attrs.bone_weights = Some(w);
        attrs.bone_indices = Some(i);
    }
    // UnityPy MeshHelper.py:615-616
    let mut triangles = None;
    let packed = vector(&cm, "m_Triangles")?;
    if items(&packed)? > 0 {
        triangles = Some(unpack_ints(&packed, 0, None, meter)?.into_parts());
    }
    // UnityPy MeshHelper.py:618-628
    if let Some(colors) = held(&cm, "m_Colors")? {
        attrs.colors = Some(packed_colors(&colors, meter)?);
    }
    Ok(Decompressed {
        vertex_count: count,
        triangles,
    })
}

/// The UVs (`helpers/MeshHelper.py:482-513`; module docs, step 2).
fn uvs(
    cm: &Obj<'_>,
    uv: &Obj<'_>,
    count: u64,
    attrs: &mut Attributes,
    meter: &Arc<Meter>,
) -> Result<(), MeshError> {
    match cm.uint("m_UVInfo", u64::from(u32::MAX))? {
        Some(info) if info != 0 => {
            let mut offset = 0u64;
            for (ch, slot) in attrs.uv.iter_mut().enumerate() {
                let bits = (info >> (4 * ch)) & 0xF;
                if bits & 4 != 0 {
                    let dim = 1 + (bits & 3);
                    let n = count.saturating_mul(dim);
                    *slot = Some(floats(uv, offset, Some(n), size(dim), meter)?);
                    // Tier C (module docs): UnityPy assigns, `uvSrcOffset = uvDim * m_VertexCount`.
                    offset = offset.saturating_add(n);
                }
            }
        }
        _ => {
            let n = count.saturating_mul(2);
            attrs.uv[0] = Some(floats(uv, 0, Some(n), 2, meter)?);
            if items(uv)? >= count.saturating_mul(4) {
                attrs.uv[1] = Some(floats(uv, n, Some(n), 2, meter)?);
            }
        }
    }
    Ok(())
}

/// The normals (`helpers/MeshHelper.py:528-544`; module docs, step 4).
fn decompress_normals(
    cm: &Obj<'_>,
    normals: &Obj<'_>,
    count: u64,
    meter: &Arc<Meter>,
) -> Result<VertexArray, MeshError> {
    let data = unpack_floats(normals, 0, None, meter)?;
    let signs = unpack_ints(&vector(cm, "m_NormalSigns")?, 0, None, meter)?;
    let mut out = zeros(count, 3, meter)?;
    let data = data.as_slice();
    let rows = data.len().div_ceil(2).min(signs.len()).min(size(count));
    meter.count_decode(u64::try_from(rows).unwrap_or(u64::MAX))?;
    for (i, (&sign, row)) in signs
        .as_slice()
        .iter()
        .zip(out.as_mut_slice().as_chunks_mut::<3>().0)
        .enumerate()
        .take(rows)
    {
        let (x, y) = two(pair(data, i))?;
        let zsqr = zsqr(x, y);
        let mut v = if zsqr >= 0.0 {
            [
                Component::Float(x),
                Component::Float(y),
                Component::Float(zsqr.sqrt()),
            ]
        } else {
            normalize(x, y)?
        };
        if sign == 0 {
            // UnityPy MeshHelper.py:543: `*= -1`, which keeps a NaN as it is.
            v[2] = match v[2] {
                Component::Float(z) if z.is_nan() => Component::Float(z),
                Component::Float(z) => Component::Float(-z),
                Component::Int(z) => Component::Int(z.saturating_neg()),
            };
        }
        *row = v;
    }
    let (values, charge) = out.into_parts();
    Ok(VertexArray::new(3, Values::Mixed(values), charge))
}

/// The tangents (`helpers/MeshHelper.py:547-565`; module docs, step 5).
fn decompress_tangents(
    cm: &Obj<'_>,
    tangents: &Obj<'_>,
    count: u64,
    meter: &Arc<Meter>,
) -> Result<VertexArray, MeshError> {
    let data = unpack_floats(tangents, 0, None, meter)?;
    let signs = unpack_ints(&vector(cm, "m_TangentSigns")?, 0, None, meter)?;
    let mut out = zeros(count, 4, meter)?;
    let (data, signs) = (data.as_slice(), signs.as_slice());
    let rows = data
        .len()
        .div_ceil(2)
        .min(signs.len().div_ceil(2))
        .min(size(count));
    meter.count_decode(u64::try_from(rows).unwrap_or(u64::MAX))?;
    for (i, row) in out
        .as_mut_slice()
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .enumerate()
        .take(rows)
    {
        // The loop's target unpacks the signs before the body the pair.
        let (sign_z, sign_w) = two(pair(signs, i))?;
        let (x, y) = two(pair(data, i))?;
        let zsqr = zsqr(x, y);
        let [x, y, mut z] = if zsqr >= 0.0 {
            [
                Component::Float(x),
                Component::Float(y),
                Component::Float(zsqr.sqrt()),
            ]
        } else {
            normalize(x, y)?
        };
        if sign_z == 0 {
            // UnityPy MeshHelper.py:561-562: unary `-`, a NaN's sign too.
            z = match z {
                Component::Float(z) => Component::Float(-z),
                Component::Int(z) => Component::Int(z.saturating_neg()),
            };
        }
        let w = if sign_w > 0 { 1.0 } else { -1.0 };
        *row = [x, y, z, Component::Float(w)];
    }
    let (values, charge) = out.into_parts();
    Ok(VertexArray::new(4, Values::Mixed(values), charge))
}

/// `IndexError` "list index out of range": a skin's vertex past the count.
fn past() -> MeshError {
    MeshError::Index(IndexKind::List)
}

/// The skin (`helpers/MeshHelper.py:573-612`; module docs): the bone
/// weights, then the bone indices.
fn decompress_skin(
    cm: &Obj<'_>,
    weights: &Obj<'_>,
    count: u64,
    meter: &Arc<Meter>,
) -> Result<(VertexArray, VertexArray), MeshError> {
    let weights = unpack_ints(weights, 0, None, meter)?;
    let indices = unpack_ints(&vector(cm, "m_BoneIndices")?, 0, None, meter)?;
    let mut bw = zeros(count, 4, meter)?;
    let n = size(count.saturating_mul(4));
    let mut bi = MeteredVec::<i64>::with_capacity(meter, n)?;
    meter.count_decode(u64::try_from(n).unwrap_or(u64::MAX))?;
    for _ in 0..n {
        bi.push(0)?;
    }
    meter.count_decode(u64::try_from(weights.len()).unwrap_or(u64::MAX))?;
    let (weights_out, indices_out) = (bw.as_mut_slice(), bi.as_mut_slice());
    let mut next = indices.as_slice().iter();
    let (mut vertex, mut j, mut sum) = (0usize, 0usize, 0u64);
    for &weight in weights.as_slice() {
        // `zip` takes the weight, then stops where the indices end.
        let Some(&index) = next.next() else { break };
        let at = vertex.saturating_mul(4).saturating_add(j);
        // UnityPy MeshHelper.py:587-588
        *weights_out.get_mut(at).ok_or_else(past)? = Component::Float(f64::from(weight) / 31.0);
        *indices_out.get_mut(at).ok_or_else(past)? = i64::from(index);
        j += 1;
        sum += u64::from(weight);
        if sum >= 31 {
            vertex += 1;
            (j, sum) = (0, 0);
        } else if j == 3 {
            let at = vertex.saturating_mul(4).saturating_add(3);
            // Tier C (module docs): UnityPy's `1 - sum`, an `int`. Here
            // `sum` is below 31.
            let fourth = f64::from(u8::try_from(31 - sum).unwrap_or(0)) / 31.0;
            *weights_out.get_mut(at).ok_or_else(past)? = Component::Float(fourth);
            // UnityPy MeshHelper.py:605
            let &index = next.next().ok_or(MeshError::StopIteration)?;
            *indices_out.get_mut(at).ok_or_else(past)? = i64::from(index);
            vertex += 1;
            (j, sum) = (0, 0);
        }
    }
    let (w, wc) = bw.into_parts();
    let (i, ic) = bi.into_parts();
    Ok((
        VertexArray::new(4, Values::Mixed(w), wc),
        VertexArray::new(4, Values::Int(i), ic),
    ))
}

/// The packed colours (`helpers/MeshHelper.py:618-628`): each RGBA
/// integer's bytes from the top, divided by 255.
fn packed_colors(colors: &Obj<'_>, meter: &Arc<Meter>) -> Result<VertexArray, MeshError> {
    let rgba = unpack_ints(colors, 0, None, meter)?;
    let n = rgba.len().saturating_mul(4);
    let mut out = MeteredVec::<f64>::with_capacity(meter, n)?;
    meter.count_decode(u64::try_from(n).unwrap_or(u64::MAX))?;
    for &c in rgba.as_slice() {
        for byte in c.to_be_bytes() {
            out.push(f64::from(byte) / 255.0)?;
        }
    }
    let (values, charge) = out.into_parts();
    Ok(VertexArray::new(4, Values::Float(values), charge))
}

#[cfg(test)]
mod tests {
    use super::normalize;
    use crate::error::MeshError;
    use crate::unpack::Component::{Float, Int};

    /// `PARITY.md` section 15.3's `normalize` row, as `test_rows_meshes.py`
    /// pins UnityPy's: a product with the reciprocal, and `int` zeros at a
    /// length of 1e-5 or less.
    #[test]
    fn normalize_multiplies_by_the_reciprocal() {
        assert_eq!(
            normalize(3.0, 4.0).unwrap(),
            [Float(0.600_000_000_000_000_1), Float(0.8), Float(0.0)]
        );
        assert_eq!(
            normalize(2e-5, 0.0).unwrap(),
            [Float(0.999_999_999_999_999_9), Float(0.0), Float(0.0)]
        );
        assert_eq!(normalize(1e-5, 0.0).unwrap(), [Int(0); 3]);
        assert_eq!(normalize(f64::NAN, 1.0).unwrap(), [Int(0); 3]);
        assert!(matches!(normalize(1.0, 1e200), Err(MeshError::Overflow)));
        let [x, y, z] = normalize(f64::INFINITY, 0.0).unwrap();
        assert!(matches!(x, Float(v) if v.is_nan()));
        assert_eq!((y, z), (Float(0.0), Float(0.0)));
    }
}
