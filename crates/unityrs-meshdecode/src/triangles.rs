// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/MeshHelper.py's MeshHandler.get_triangles. See THIRD_PARTY_LICENSES.
//! `get_triangles` (`helpers/MeshHelper.py:630-683`): each submesh's
//! triangles from the index buffer.
//!
//! A submesh starts at `firstByte // 2` indices, halved again for 32-bit
//! indices, and spans `indexCount` of them:
//!
//! - **Triangles** (`topology` 0): every third index from the start, each
//!   with the slice of three that begins there, `buffer[i:i+3]`. Python's
//!   slice stops at the buffer's end, so past it the tuples are shorter,
//!   then empty: [`SubMeshTriangles`] holds the whole ones, the one cut
//!   short, if any, and the count of empty ones, never allocating those.
//! - **Strips** (below 4, or `topology` 1): a triangle at each index but
//!   the last two, each from the three indices there; one with two equal
//!   indices is skipped, and every odd one from the start swaps its first
//!   two. The submesh's `indexCount` is then set to three per triangle
//!   kept ([`SubMeshTriangles::index_count`]): UnityPy writes it into the
//!   `SubMesh`, so a second call reads the new count, where this crate
//!   reads its input and changes nothing. A strip whose indices run past
//!   the buffer raises `ValueError` "not enough values to unpack".
//! - **Quads** (`topology` 2): two triangles per four indices, `(a, b, c)`
//!   and `(a, c, d)`, in a list of `indexCount // 2`: a count that is not
//!   a multiple of 4 overruns it, `IndexError` "list assignment index out
//!   of range", once every quad before has unpacked.
//! - Anything else, lines, points, no `topology` at all from 4 on, or any
//!   other value: `ValueError` "Failed getting triangles. Submesh topology
//!   is lines or points.".
//!
//! **Bounds.** The submeshes are counted as decode work on the caller's
//! meter, one unit each, before the loop over them; then each submesh's
//! triangles are charged to it, and a list's tuples or a strip's or quads'
//! loop counted, before the submesh is read (`docs/plans/extract.md` E59),
//! and every field read counts its scans ([`unityrs_core::objmode`]). Empty
//! tuples cost nothing, so a count of 2^32 past a short buffer is one
//! number.

use std::sync::Arc;

use unityrs_core::limits::{Charge, Meter, MeteredVec};

use crate::Version;
use crate::error::{IndexKind, MeshError};
use crate::format::MeshTopology;
use crate::vertex::field;
use unityrs_core::objmode::{Obj, SUB_MESH};

/// `get_triangles()`'s list: one [`SubMeshTriangles`] per submesh.
#[derive(Debug)]
pub struct Triangles {
    submeshes: Vec<SubMeshTriangles>,
    // Declared last: the charge is returned after the list is freed.
    _charge: Charge,
}

const _: () = crate::send_sync::<Triangles>();

impl Triangles {
    /// Each submesh's triangles, in the source's order.
    #[must_use]
    pub fn submeshes(&self) -> &[SubMeshTriangles] {
        &self.submeshes
    }
}

/// One submesh's list of index tuples (module docs): the whole triangles,
/// then at most one tuple a triangle list's buffer cut short, then empty
/// tuples.
#[derive(Debug)]
pub struct SubMeshTriangles {
    triangles: Vec<[u32; 3]>,
    cut: [u32; 2],
    cut_len: u8,
    empty: u64,
    index_count: u64,
    // Declared last: the charge is returned after the triangles are freed.
    _charge: Charge,
}

const _: () = crate::send_sync::<SubMeshTriangles>();

impl SubMeshTriangles {
    /// The tuples of three, first in the list.
    #[must_use]
    pub fn triangles(&self) -> &[[u32; 3]] {
        &self.triangles
    }

    /// The tuple of one or two indices after them, where a triangle list's
    /// slice met the buffer's end; empty if there is none.
    #[must_use]
    pub fn cut(&self) -> &[u32] {
        self.cut
            .get(..usize::from(self.cut_len))
            .unwrap_or_default()
    }

    /// How many empty tuples end the list (a triangle list's slices past
    /// the buffer's end).
    #[must_use]
    pub fn empty(&self) -> u64 {
        self.empty
    }

    /// The list's length.
    #[must_use]
    pub fn len(&self) -> u64 {
        u64::try_from(self.triangles.len())
            .unwrap_or(u64::MAX)
            .saturating_add(u64::from(self.cut_len > 0))
            .saturating_add(self.empty)
    }

    /// Whether the list is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The submesh's `indexCount` after the call: a strip's is three per
    /// triangle kept, any other's is unchanged.
    #[must_use]
    pub fn index_count(&self) -> u64 {
        self.index_count
    }
}

/// `get_triangles()` on what `process` left: the source, the version, the
/// index size and the index buffer (module docs).
pub(crate) fn get_triangles(
    src: &Obj<'_>,
    version: Version,
    use_16bit: bool,
    buffer: Option<&[u32]>,
    meter: &Arc<Meter>,
) -> Result<Triangles, MeshError> {
    // UnityPy MeshHelper.py:631-632
    let buffer = buffer.ok_or(MeshError::Assertion(""))?;
    let subs = src
        .list("m_SubMeshes", &SUB_MESH)?
        .ok_or(MeshError::Assertion(""))?;
    let mut out = MeteredVec::<SubMeshTriangles>::with_capacity(meter, subs.len())?;
    // One unit a submesh, those of no indices included.
    meter.count_decode(u64::try_from(subs.len()).unwrap_or(u64::MAX))?;
    for sub in subs.iter() {
        let sub = sub?;
        // UnityPy MeshHelper.py:637-642
        let mut first = field(&sub, "firstByte", u64::from(u32::MAX))? / 2;
        if !use_16bit {
            first /= 2;
        }
        let count = field(&sub, "indexCount", u64::from(u32::MAX))?;
        let topology = sub.int32("topology")?;
        let is = |t: MeshTopology| topology == Some(t.value());
        let one = if is(MeshTopology::Triangles) {
            list(buffer, first, count, meter)?
        } else if version.major()? < 4 || is(MeshTopology::TriangleStrip) {
            strip(buffer, first, count, meter)?
        } else if is(MeshTopology::Quads) {
            quads(buffer, first, count, meter)?
        } else {
            return Err(MeshError::Topology);
        };
        out.push(one)?;
    }
    let (submeshes, charge) = out.into_parts();
    Ok(Triangles {
        submeshes,
        _charge: charge,
    })
}

/// `buffer[i]`, the index at a position known to be in it.
fn at(buffer: &[u32], i: u64) -> u32 {
    usize::try_from(i)
        .ok()
        .and_then(|i| buffer.get(i))
        .copied()
        .unwrap_or(0)
}

fn len64(buffer: &[u32]) -> u64 {
    u64::try_from(buffer.len()).unwrap_or(u64::MAX)
}

/// A triangle list (`helpers/MeshHelper.py:646-649`).
fn list(
    buffer: &[u32],
    first: u64,
    count: u64,
    meter: &Arc<Meter>,
) -> Result<SubMeshTriangles, MeshError> {
    let n = len64(buffer);
    let tuples = count.div_ceil(3);
    let whole = if n >= first.saturating_add(3) {
        tuples.min((n - first) / 3)
    } else {
        0
    };
    // Charged, then counted: a refused charge counts no work.
    let mut triangles =
        MeteredVec::<[u32; 3]>::with_capacity(meter, usize::try_from(whole).unwrap_or(usize::MAX))?;
    meter.count_decode(whole.saturating_mul(12))?;
    for k in 0..whole {
        let i = first + 3 * k;
        triangles.push([at(buffer, i), at(buffer, i + 1), at(buffer, i + 2)])?;
    }
    let mut cut = [0u32; 2];
    let mut cut_len = 0u8;
    let mut rest = tuples - whole;
    if rest > 0 {
        let i = first.saturating_add(3 * whole);
        if i < n {
            // Fewer than three are left, or `whole` would count it.
            let left = n - i;
            cut = [at(buffer, i), if left > 1 { at(buffer, i + 1) } else { 0 }];
            cut_len = u8::try_from(left.min(2)).unwrap_or(2);
            rest -= 1;
        }
    }
    let (triangles, charge) = triangles.into_parts();
    Ok(SubMeshTriangles {
        triangles,
        cut,
        cut_len,
        empty: rest,
        index_count: count,
        _charge: charge,
    })
}

/// A strip (`helpers/MeshHelper.py:651-666`).
fn strip(
    buffer: &[u32],
    first: u64,
    count: u64,
    meter: &Arc<Meter>,
) -> Result<SubMeshTriangles, MeshError> {
    let end = len64(buffer);
    let steps = count.saturating_sub(2);
    // UnityPy MeshHelper.py:655: each step unpacks three; the first that
    // meets the buffer's end raises, whatever the steps before it kept.
    if steps > 0 && first.saturating_add(count) > end {
        let failing = first.max(end.saturating_sub(2));
        let got = u8::try_from(end.saturating_sub(failing)).unwrap_or(0);
        return Err(MeshError::Unpack { expected: 3, got });
    }
    // Charged, then counted: a refused charge counts no work. Every step's
    // triangle is charged, the degenerate ones it drops included.
    let mut triangles =
        MeteredVec::<[u32; 3]>::with_capacity(meter, usize::try_from(steps).unwrap_or(usize::MAX))?;
    meter.count_decode(steps.saturating_mul(12))?;
    for k in 0..steps {
        let pos = first + k;
        let (a, b, c) = (at(buffer, pos), at(buffer, pos + 1), at(buffer, pos + 2));
        if a == b || a == c || b == c {
            continue;
        }
        triangles.push(if k & 1 == 1 { [b, a, c] } else { [a, b, c] })?;
    }
    let kept = u64::try_from(triangles.len()).unwrap_or(u64::MAX);
    let (triangles, charge) = triangles.into_parts();
    Ok(SubMeshTriangles {
        triangles,
        cut: [0; 2],
        cut_len: 0,
        empty: 0,
        // UnityPy MeshHelper.py:666
        index_count: kept.saturating_mul(3),
        _charge: charge,
    })
}

/// Quads (`helpers/MeshHelper.py:668-676`).
fn quads(
    buffer: &[u32],
    first: u64,
    count: u64,
    meter: &Arc<Meter>,
) -> Result<SubMeshTriangles, MeshError> {
    let end = len64(buffer);
    let quads = count.div_ceil(4);
    // UnityPy MeshHelper.py:673: the first quad that meets the buffer's
    // end raises.
    let unpacked = if end >= first.saturating_add(4) {
        (end - first - 4) / 4 + 1
    } else {
        0
    };
    if unpacked < quads {
        let failing = first.saturating_add(4 * unpacked);
        let got = u8::try_from(end.saturating_sub(failing)).unwrap_or(0);
        return Err(MeshError::Unpack { expected: 4, got });
    }
    // UnityPy MeshHelper.py:674-675: a last quad past `indexCount // 2`
    // triangles.
    if !count.is_multiple_of(4) {
        return Err(MeshError::Index(IndexKind::ListAssignment));
    }
    let len = quads.saturating_mul(2);
    // Charged, then counted: a refused charge counts no work.
    let mut triangles =
        MeteredVec::<[u32; 3]>::with_capacity(meter, usize::try_from(len).unwrap_or(usize::MAX))?;
    meter.count_decode(len.saturating_mul(12))?;
    for q in 0..quads {
        let pos = first + 4 * q;
        let (a, b, c, d) = (
            at(buffer, pos),
            at(buffer, pos + 1),
            at(buffer, pos + 2),
            at(buffer, pos + 3),
        );
        triangles.push([a, b, c])?;
        triangles.push([a, c, d])?;
    }
    let (triangles, charge) = triangles.into_parts();
    Ok(SubMeshTriangles {
        triangles,
        cut: [0; 2],
        cut_len: 0,
        empty: 0,
        index_count: count,
        _charge: charge,
    })
}
