// Ported from UnityPy 1.25.2 (MIT): UnityPy/export/MeshRendererExporter.py's export_material. See THIRD_PARTY_LICENSES.
//! PARITY sections 15.4 and 19.1: lowercase `ColorRGBA` components and dict
//! texture items are the corrected oracle. Defaults and ordering are Tier A.
use super::{MeteredWrite, view};
use crate::{
    MeshError,
    pyfmt::{PyFmtBuf, f4},
};
use std::sync::Arc;
use unityrs_core::{
    Parsed, RawStr, SerializedFile, Value,
    env::Environment,
    limits::{Meter, MeteredVec},
    objmode::{Class, Obj},
};

const MATERIAL: Class = Class {
    types: &["Material"],
    required: &["m_Name", "m_SavedProperties", "m_Shader"],
    optional: &[],
    slotted: false,
};
const SHEET: Class = Class {
    types: &["UnityPropertySheet"],
    required: &["m_Colors", "m_Floats", "m_TexEnvs"],
    optional: &["m_Ints"],
    slotted: true,
};
/// Borrowed object-mode material and resolution context. No allocation.
pub struct MaterialView<'a> {
    pub(super) p: &'a Parsed,
    pub(super) name: &'a RawStr,
    saved: Obj<'a>,
    pub(super) from: &'a Arc<SerializedFile>,
    pub(super) env: &'a Environment,
    pub(super) meter: &'a Arc<Meter>,
}
const _: () = crate::send_sync::<MaterialView<'static>>();
impl<'a> MaterialView<'a> {
    pub fn new(
        p: &'a Parsed,
        from: &'a Arc<SerializedFile>,
        env: &'a Environment,
        meter: &'a Arc<Meter>,
    ) -> Result<Self, MeshError> {
        let root = Obj::root(p, &MATERIAL, meter)?;
        Ok(Self {
            p,
            name: root.string("m_Name")?.ok_or(MeshError::Field("m_Name"))?,
            saved: root
                .class("m_SavedProperties", &SHEET)?
                .ok_or(MeshError::Field("m_SavedProperties"))?,
            from,
            env,
            meter,
        })
    }
    fn properties(&self, name: &'static str) -> Result<&'a Value, MeshError> {
        Ok(self.saved.get(name)?.ok_or(MeshError::Field(name))?.1)
    }
    fn color(&self, name: &[u8], default: f64) -> Result<[f64; 3], MeshError> {
        let Some(v) = view::lookup(self.p, self.properties("m_Colors")?, name, self.meter)? else {
            return Ok([default; 3]);
        };
        let mut out = [0.; 3];
        for (n, slot) in ["r", "g", "b"].into_iter().zip(&mut out) {
            *slot = view::number(view::attr(self.p, v, n, self.meter)?)?;
        }
        Ok(out)
    }
    fn float(&self, name: &[u8], default: f64) -> Result<f64, MeshError> {
        view::lookup(self.p, self.properties("m_Floats")?, name, self.meter)?
            .map_or(Ok(default), view::number)
    }
    pub(super) fn textures(&self) -> Result<MeteredVec<(&'a RawStr, &'a Value)>, MeshError> {
        self.texture_entries(true)
    }
    pub(super) fn texture_entries(
        &self,
        deduplicate: bool,
    ) -> Result<MeteredVec<(&'a RawStr, &'a Value)>, MeshError> {
        // Dict construction keeps first-key order, last value. Null texture
        // pointers still replace earlier ones. FastPropertyName uses .name.
        let items = view::list(self.properties("m_TexEnvs")?)?;
        let mut out: MeteredVec<(&RawStr, &Value)> =
            MeteredVec::with_capacity(self.meter, items.len())?;
        self.meter.count_decode(items.len() as u64)?;
        for item in items {
            let (k, v) = view::pair(item)?;
            let k = view::key(self.p, k, self.meter)?;
            self.meter
                .count_decode((out.len() as u64).saturating_mul(k.as_bytes().len() as u64 + 1))?;
            if deduplicate
                && let Some((_, old)) = out.as_mut_slice().iter_mut().find(|(key, _)| *key == k)
            {
                *old = v;
            } else {
                out.push((k, v))?;
            }
        }
        Ok(out)
    }
    pub(super) fn texture(
        &self,
        key: &RawStr,
        value: &Value,
    ) -> Result<Option<(unityrs_core::ObjectRef, RawStr)>, MeshError> {
        let ptr = view::pointer(
            self.p,
            view::attr(self.p, value, "m_Texture", self.meter)?,
            self.meter,
        )?;
        if !ptr.is_set() {
            return Ok(None);
        }
        let object = self.env.deref(self.from, ptr)?;
        let parsed = object.parse_as_dict(false)?;
        let name = view::string(view::attr(&parsed, parsed.value(), "m_Name", self.meter)?)?;
        let name = if name.is_empty() {
            key.clone()
        } else {
            name.clone()
        };
        Ok(Some((object, name)))
    }
}
/// Writes MTL text with Python's defaults and precision, no trailing newline.
pub fn export_material(m: &MaterialView<'_>, out: &mut dyn MeteredWrite) -> Result<(), MeshError> {
    let diffuse = m.color(b"_Color", 0.8)?;
    let ambient = m.color(b"_SColor", 0.2)?;
    let specular = m.color(b"_SpecularColor", 0.2)?;
    let shininess = m.float(b"_Shininess", 20.)?;
    let transparency = m.float(b"_Transparency", 0.)?;
    out.write_bytes(b"newmtl ")?;
    out.write_bytes(m.name.as_bytes())?;
    let mut buf = PyFmtBuf::default();
    for (label, values) in [
        (b"\nKa".as_slice(), ambient),
        (b"\nKd", diffuse),
        (b"\nKs", specular),
    ] {
        out.write_bytes(label)?;
        for x in values {
            out.write_bytes(b" ")?;
            out.write_bytes(f4(x, &mut buf).as_bytes())?;
        }
    }
    out.write_bytes(b"\nTr ")?;
    out.write_bytes(f4(transparency, &mut buf).as_bytes())?;
    out.write_bytes(b"\nNs ")?;
    out.write_bytes(f4(shininess, &mut buf).as_bytes())?;
    for &(key, value) in m.textures()?.as_slice() {
        if let Some((_, name)) = m.texture(key, value)? {
            let key = key.as_bytes();
            m.meter.count_decode(key.len() as u64)?;
            let labels: &[&[u8]] = if key == b"_MainTex" {
                &[b"map_Kd"]
            } else if key == b"_BumpMap" {
                &[b"map_bump", b"bump"]
            } else if key.windows(8).any(|w| w == b"Specular") {
                &[b"map_Ks"]
            } else {
                &[]
            };
            for label in labels {
                out.write_bytes(b"\n")?;
                out.write_bytes(label)?;
                out.write_bytes(b" ")?;
                out.write_bytes(name.as_bytes())?;
                out.write_bytes(b".png")?;
            }
        }
    }
    Ok(())
}
