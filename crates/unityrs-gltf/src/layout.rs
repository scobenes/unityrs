//! Fallible layout and G8 analysis; no payload copies are retained here.
use crate::{Attribute, GltfError, Kind, Options, Scene, normalise, validate};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};

/// Why a primitive or attribute was omitted under spec-valid policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    NonFinitePosition,
    InvalidNormal,
    InvalidTangent,
}
const _: () = crate::send_sync::<Reason>();
/// One primitive decision, with optional output indices and original source indices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Issue {
    pub mesh: Option<usize>,
    pub primitive: Option<usize>,
    pub source_mesh: usize,
    pub source_primitive: usize,
    pub reason: Reason,
}
const _: () = crate::send_sync::<Issue>();
/// Accessor changes count differing f32 scalar bits once per emitted accessor.
/// Rotation changes count differing f64 scalar bits once per node.
#[derive(Debug)]
pub struct Report {
    pub normal_components: u64,
    /// Differing f64 rotation components, counted once per node.
    pub rotation_components: u64,
    pub joint_components: u64,
    pub tangent_components: u64,
    pub weight_components: u64,
    pub zero_weight_rows: u64,
    pub issues: MeteredVec<Issue>,
}
const _: () = crate::send_sync::<Report>();
/// A BIN range; all starts are multiples of four, padding excluded from length.
#[derive(Debug, Clone, Copy)]
pub struct BufferView {
    pub byte_offset: u32,
    pub byte_length: u32,
    pub(crate) target: Option<u32>,
}
const _: () = crate::send_sync::<BufferView>();
#[derive(Debug)]
pub(crate) struct AccessorLayout {
    pub id: Option<usize>,
    pub view: Option<usize>,
    pub sparse: Option<(usize, usize)>,
    pub min: [f32; 16],
    pub max: [f32; 16],
    pub valid: bool,
    pub analysed: bool,
    pub joint_weights: Option<usize>,
    pub changes: u64,
    pub zeros: u64,
    pub joint_changes: u64,
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct PrimitiveLayout {
    pub keep: bool,
    pub normal: bool,
    pub tangent: bool,
}
#[derive(Debug)]
pub(crate) struct MeshLayout {
    pub id: Option<usize>,
    pub primitives: MeteredVec<PrimitiveLayout>,
}
/// Charged metadata only; inspect views without constructing the binary chunk.
#[derive(Debug)]
pub struct Layout {
    pub views: MeteredVec<BufferView>,
    /// Logical BIN length. The chunk length adds zero to three padding bytes.
    pub bin_length: u32,
    pub report: Report,
    pub(crate) accessors: MeteredVec<AccessorLayout>,
    pub(crate) accessor_order: MeteredVec<usize>,
    pub(crate) meshes: MeteredVec<MeshLayout>,
    pub(crate) rotations: MeteredVec<[f64; 4]>,
    pub(crate) image_views: MeteredVec<usize>,
}
const _: () = crate::send_sync::<Layout>();
impl Layout {
    pub(crate) fn view(&mut self, len: usize, target: Option<u32>) -> Result<usize, GltfError> {
        let offset = align(self.bin_length)?;
        let length = u32::try_from(len).map_err(|_| validate::invalid("BIN length exceeds u32"))?;
        self.bin_length = offset
            .checked_add(length)
            .ok_or_else(|| validate::invalid("BIN length overflow"))?;
        let id = self.views.len();
        self.views.push(BufferView {
            byte_offset: offset,
            byte_length: length,
            target,
        })?;
        Ok(id)
    }
}
pub(crate) fn align(n: u32) -> Result<u32, GltfError> {
    n.checked_add(3)
        .map(|v| v & !3)
        .ok_or_else(|| validate::invalid("GLB alignment overflow"))
}
/// Validate references, analyse normalisation, and assign ordered aligned views.
/// A non-finite position removes only its referencing primitive. Meshes left
/// empty are omitted and their nodes retain their transform and metadata.
#[allow(clippy::too_many_lines)]
pub fn layout(s: &Scene, opts: &Options, meter: &Arc<Meter>) -> Result<Layout, GltfError> {
    validate::model(s, *opts, meter)?;
    let mut l = Layout {
        views: MeteredVec::new(meter),
        bin_length: 0,
        report: Report {
            normal_components: 0,
            rotation_components: 0,
            joint_components: 0,
            tangent_components: 0,
            weight_components: 0,
            zero_weight_rows: 0,
            issues: MeteredVec::new(meter),
        },
        accessors: MeteredVec::with_capacity(meter, s.accessors.len())?,
        accessor_order: MeteredVec::new(meter),
        meshes: MeteredVec::with_capacity(meter, s.meshes.len())?,
        image_views: MeteredVec::new(meter),
        rotations: MeteredVec::with_capacity(meter, s.nodes.len())?,
    };
    meter.count_decode(s.nodes.len() as u64 * 32)?;
    for n in s.nodes.as_slice() {
        let mut rotation = n.rotation;
        if opts.normalise {
            let norm = rotation.iter().map(|v| v * v).sum::<f64>();
            let mut length = norm.sqrt();
            if !length.is_finite() || norm < f64::MIN_POSITIVE {
                // Scaling first keeps finite extreme rotations representable.
                let scale = rotation.iter().map(|v| v.abs()).fold(0., f64::max);
                for value in &mut rotation {
                    *value /= scale;
                }
                length = rotation.iter().map(|v| v * v).sum::<f64>().sqrt();
            }
            for (value, original) in rotation.iter_mut().zip(n.rotation) {
                *value = (*value / length).clamp(-1., 1.);
                l.report.rotation_components += u64::from(value.to_bits() != original.to_bits());
            }
        }
        l.rotations.push(rotation)?;
    }
    meter.count_decode(s.accessors.len() as u64)?;
    for _ in s.accessors.as_slice() {
        l.accessors.push(AccessorLayout {
            id: None,
            view: None,
            sparse: None,
            min: [0.; 16],
            max: [0.; 16],
            valid: true,
            analysed: false,
            joint_weights: None,
            changes: 0,
            zeros: 0,
            joint_changes: 0,
        })?;
    }
    let mut mesh_id = 0;
    for (mi, m) in s.meshes.as_slice().iter().enumerate() {
        let mut ml = MeshLayout {
            id: None,
            primitives: MeteredVec::with_capacity(meter, m.primitives.len())?,
        };
        let mut primitive_id = 0;
        for (pi, p) in m.primitives.as_slice().iter().enumerate() {
            meter.count_decode(p.attributes.len() as u64 + 1)?;
            let issue_start = l.report.issues.len();
            let mut pl = PrimitiveLayout {
                keep: true,
                normal: true,
                tangent: true,
            };
            for &(kind, id) in p.attributes.as_slice() {
                if matches!(
                    kind,
                    Attribute::Position | Attribute::Normal | Attribute::Tangent
                ) {
                    analyse(s, &mut l, id, *opts, meter)?;
                }
                if l.accessors.as_slice().get(id).is_none_or(|a| !a.valid) {
                    let reason = match kind {
                        Attribute::Position => {
                            pl.keep = false;
                            Reason::NonFinitePosition
                        }
                        Attribute::Normal => {
                            pl.normal = false;
                            pl.tangent = false;
                            Reason::InvalidNormal
                        }
                        Attribute::Tangent => {
                            pl.tangent = false;
                            Reason::InvalidTangent
                        }
                        _ => return Err(validate::invalid("invalid attribute")),
                    };
                    l.report.issues.push(Issue {
                        mesh: None,
                        primitive: None,
                        source_mesh: mi,
                        source_primitive: pi,
                        reason,
                    })?;
                }
            }
            meter.count_decode(p.targets.len() as u64 * 3)?;
            for t in p.targets.as_slice() {
                if t.iter().enumerate().all(|(slot, id)| {
                    id.is_none() || (slot == 1 && !pl.normal) || (slot == 2 && !pl.tangent)
                }) {
                    pl.keep = false;
                }
            }
            if pl.keep
                && opts.normalise
                && let Some((_, joint)) = p
                    .attributes
                    .as_slice()
                    .iter()
                    .find(|(k, _)| *k == Attribute::Joints)
            {
                let weights = p
                    .attributes
                    .as_slice()
                    .iter()
                    .find(|(k, _)| *k == Attribute::Weights)
                    .map(|(_, id)| *id)
                    .ok_or_else(|| validate::invalid("joint weights"))?;
                let al = l
                    .accessors
                    .as_mut_slice()
                    .get_mut(*joint)
                    .ok_or_else(|| validate::invalid("joint accessor"))?;
                if let Some(previous) = al.joint_weights
                    && previous != weights
                {
                    let a = validate::accessor(s, *joint, Kind::Joints)?;
                    let first = validate::accessor(s, previous, Kind::Weights)?;
                    let second = validate::accessor(s, weights, Kind::Weights)?;
                    meter.count_decode(a.count as u64 * 80)?;
                    for row in 0..a.count {
                        if normalise::joints(a, first, row)? != normalise::joints(a, second, row)? {
                            return Err(validate::invalid(
                                "shared joints have conflicting zero-weight repairs",
                            ));
                        }
                    }
                }
                if al.joint_weights.is_none() {
                    al.joint_weights = Some(weights);
                    let a = validate::accessor(s, *joint, Kind::Joints)?;
                    let w = validate::accessor(s, weights, Kind::Weights)?;
                    meter.count_decode(a.count as u64 * 40)?;
                    for row in 0..a.count {
                        let repaired = normalise::joints(a, w, row)?;
                        for (col, v) in repaired.into_iter().enumerate() {
                            if a.data.integer(row * 4 + col) != Some(v) {
                                al.joint_changes += 1;
                            }
                        }
                    }
                }
            }
            if pl.keep && ml.id.is_none() {
                ml.id = Some(mesh_id);
                mesh_id += 1;
            }
            if pl.keep {
                meter.count_decode((l.report.issues.len() - issue_start) as u64)?;
                for issue in l
                    .report
                    .issues
                    .as_mut_slice()
                    .get_mut(issue_start..)
                    .ok_or_else(|| validate::invalid("issue range"))?
                {
                    issue.mesh = ml.id;
                    issue.primitive = Some(primitive_id);
                }
                primitive_id += 1;
            }
            ml.primitives.push(pl)?;
        }
        l.meshes.push(ml)?;
    }
    validate::skin_attributes(s, &l, *opts, meter)?;
    // First reach: meshes/primitives, semantic attribute order, indices,
    // morph slots, then inverse binds and images. Unreferenced data is omitted.
    for (m, mi) in s.meshes.as_slice().iter().enumerate() {
        for (pi, p) in mi.primitives.as_slice().iter().enumerate() {
            meter.count_decode(p.attributes.len() as u64 + p.targets.len() as u64 * 3 + 1)?;
            let pl = l
                .meshes
                .as_slice()
                .get(m)
                .and_then(|m| m.primitives.as_slice().get(pi))
                .copied()
                .ok_or_else(|| validate::invalid("primitive layout"))?;
            if !pl.keep {
                continue;
            }
            for &(kind, id) in p.attributes.as_slice() {
                if (kind == Attribute::Normal && !pl.normal)
                    || (kind == Attribute::Tangent && !pl.tangent)
                {
                    continue;
                }
                place(s, &mut l, id, *opts, meter)?;
            }
            if let Some(id) = p.indices {
                place(s, &mut l, id, *opts, meter)?;
            }
            for t in p.targets.as_slice() {
                for (slot, id) in t.iter().enumerate() {
                    if (slot == 1 && !pl.normal) || (slot == 2 && !pl.tangent) {
                        continue;
                    }
                    if let Some(id) = id {
                        place(s, &mut l, *id, *opts, meter)?;
                    }
                }
            }
        }
    }
    for skin in s.skins.as_slice() {
        meter.count_decode(1)?;
        if let Some(id) = skin.inverse_bind_matrices {
            place(s, &mut l, id, *opts, meter)?;
        }
    }
    for image in s.images.as_slice() {
        meter.count_decode(1)?;
        let view = l.view(image.bytes.len(), None)?;
        l.image_views.push(view)?;
    }
    Ok(l)
}

fn place(
    s: &Scene,
    l: &mut Layout,
    id: usize,
    opts: Options,
    meter: &Meter,
) -> Result<(), GltfError> {
    analyse(s, l, id, opts, meter)?;
    let al = l
        .accessors
        .as_slice()
        .get(id)
        .ok_or_else(|| validate::invalid("accessor layout"))?;
    if al.id.is_some() {
        return Ok(());
    }
    if !al.valid {
        return Err(validate::invalid("invalid referenced accessor"));
    }
    let a = s
        .accessors
        .as_slice()
        .get(id)
        .ok_or_else(|| validate::invalid("accessor index"))?;
    match a.kind {
        Kind::Normal => l.report.normal_components += al.changes,
        Kind::Tangent => l.report.tangent_components += al.changes,
        Kind::Weights => {
            l.report.weight_components += al.changes;
            l.report.zero_weight_rows += al.zeros;
        }
        Kind::Joints => l.report.joint_components += al.joint_changes,
        _ => {}
    }
    let bytes = a
        .data
        .len()
        .checked_mul(a.data.bytes())
        .ok_or_else(|| validate::invalid("accessor byte length"))?;
    let (view, sparse) = if let Some(indices) = &a.sparse_indices {
        if indices.is_empty() {
            (None, None)
        } else {
            let count = indices
                .len()
                .checked_mul(4)
                .ok_or_else(|| validate::invalid("sparse byte length"))?;
            let index_view = l.view(count, None)?;
            let values_view = l.view(bytes, None)?;
            (None, Some((index_view, values_view)))
        }
    } else {
        let target = match a.kind {
            Kind::Indices => Some(34963),
            Kind::InverseBind => None,
            k if k.morph() => None,
            _ => Some(34962),
        };
        (Some(l.view(bytes, target)?), None)
    };
    let al = l
        .accessors
        .as_mut_slice()
        .get_mut(id)
        .ok_or_else(|| validate::invalid("accessor layout"))?;
    al.id = Some(l.accessor_order.len());
    al.view = view;
    al.sparse = sparse;
    l.accessor_order.push(id)?;
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn analyse(
    s: &Scene,
    l: &mut Layout,
    id: usize,
    opts: Options,
    meter: &Meter,
) -> Result<(), GltfError> {
    let al = l
        .accessors
        .as_mut_slice()
        .get_mut(id)
        .ok_or_else(|| validate::invalid("accessor layout"))?;
    if al.analysed {
        return Ok(());
    }
    let a = s
        .accessors
        .as_slice()
        .get(id)
        .ok_or_else(|| validate::invalid("accessor index"))?;
    let rows = a.sparse_indices.as_ref().map_or(a.count, MeteredVec::len);
    meter.count_decode(
        (rows as u64)
            .saturating_mul(a.kind.width() as u64)
            .saturating_mul(16),
    )?;
    let implicit = a.sparse_indices.as_ref().is_some_and(|v| v.len() < a.count);
    let mut info = AccessorLayout {
        id: None,
        view: None,
        sparse: None,
        min: if implicit {
            [0.; 16]
        } else {
            [f32::INFINITY; 16]
        },
        max: if implicit {
            [0.; 16]
        } else {
            [f32::NEG_INFINITY; 16]
        },
        valid: true,
        analysed: true,
        joint_weights: None,
        changes: 0,
        zeros: 0,
        joint_changes: 0,
    };
    let mut changes = 0;
    let mut zeros = 0;
    for r in 0..rows {
        let raw = normalise::row(a, r, false)?;
        let out = normalise::row(a, r, opts.normalise)?;
        if opts.normalise {
            if a.kind == Kind::InverseBind {
                let [
                    _,
                    _,
                    _,
                    row_x,
                    _,
                    _,
                    _,
                    row_y,
                    _,
                    _,
                    _,
                    row_z,
                    _,
                    _,
                    _,
                    row_w,
                ] = out;
                if row_x != 0. || row_y != 0. || row_z != 0. || row_w.to_bits() != 1f32.to_bits() {
                    return Err(validate::invalid("non-affine inverse bind matrix"));
                }
            }
            if matches!(a.kind, Kind::Normal | Kind::Tangent) && normalise::invalid_direction(&raw)
            {
                info.valid = false;
            }
            if a.kind == Kind::Weights && raw.iter().take(4).all(|x| !x.is_finite() || *x <= 0.) {
                zeros += 1;
            }
        }
        for (j, (&x, &old)) in out.iter().zip(&raw).take(a.kind.width()).enumerate() {
            if opts.normalise && a.kind == Kind::Color && !(0. ..=1.).contains(&x) {
                return Err(validate::invalid("color component outside [0,1]"));
            }
            if !x.is_finite() {
                if opts.normalise
                    && !matches!(a.kind, Kind::Position | Kind::Normal | Kind::Tangent)
                {
                    return Err(validate::invalid("non-finite accessor"));
                }
                info.valid = false;
            }
            if x.to_bits() != old.to_bits() {
                changes += 1;
            }
            if x.is_finite() {
                if let Some(min) = info.min.get_mut(j) {
                    *min = min.min(x);
                }
                if let Some(max) = info.max.get_mut(j) {
                    *max = max.max(x);
                }
            }
        }
    }
    // Raw BIN may contain non-finites; JSON bounds remain finite.
    for value in info.min.iter_mut().chain(&mut info.max) {
        if !value.is_finite() {
            *value = 0.;
        }
    }
    if !opts.normalise {
        info.valid = true;
    }
    info.changes = changes;
    info.zeros = zeros;
    info.joint_weights = al.joint_weights;
    info.joint_changes = al.joint_changes;
    *al = info;
    Ok(())
}
