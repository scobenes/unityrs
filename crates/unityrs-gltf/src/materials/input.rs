//! Borrowed, object-mode material properties. No property copies or maps.
use super::{GltfError, Result};
use unityrs_core::{ObjectRef, Parsed, RawStr, Value, env::PPtr, limits::Meter};

pub(super) fn optional<'a>(
    p: &'a Parsed,
    v: &'a Value,
    name: &[u8],
    meter: &Meter,
) -> Result<Option<&'a Value>> {
    let Value::Dict(map) = v else {
        return Err(GltfError::Invalid("material object field"));
    };
    // Upper bound includes the clean-name scan of the typetree children.
    let rows = p
        .lists()
        .get(map.list() as usize)
        .map_or(0, |l| l.rows().len());
    meter.count_decode(
        (rows as u64 + map.entries().len() as u64).saturating_mul(name.len() as u64 + 1),
    )?;
    Ok(p.attr(map, name))
}
pub(super) fn attr<'a>(
    p: &'a Parsed,
    v: &'a Value,
    name: &[u8],
    meter: &Meter,
) -> Result<&'a Value> {
    optional(p, v, name, meter)?.ok_or(GltfError::Invalid("missing material field"))
}
pub(super) fn string(v: &Value) -> Result<&RawStr> {
    match v {
        Value::Str(s) => Ok(s),
        _ => Err(GltfError::Invalid("material string")),
    }
}
pub(super) fn number(v: &Value) -> Result<f64> {
    match v {
        Value::F32(x) => Ok(f64::from(f32::from_bits(*x))),
        Value::F64(x) => Ok(f64::from_bits(*x)),
        Value::Int(x) => i32::try_from(*x)
            .map(f64::from)
            .map_err(|_| GltfError::Invalid("material integer range")),
        Value::UInt(x) => u32::try_from(*x)
            .map(f64::from)
            .map_err(|_| GltfError::Invalid("material integer range")),
        _ => Err(GltfError::Invalid("material number")),
    }
}
pub(super) fn pointer(p: &Parsed, v: &Value, meter: &Meter) -> Result<PPtr> {
    Ok(PPtr {
        file_id: attr(p, v, b"m_FileID", meter)?
            .as_i64()
            .and_then(|x| i32::try_from(x).ok())
            .ok_or(GltfError::Invalid("material file id"))?,
        path_id: attr(p, v, b"m_PathID", meter)?
            .as_i64()
            .ok_or(GltfError::Invalid("material path id"))?,
    })
}
fn list(v: &Value) -> Result<&[Value]> {
    match v {
        Value::List(v) => Ok(v),
        Value::Tuple(v) => Ok(v),
        Value::Prims(v) if v.is_empty() => Ok(&[]),
        _ => Err(GltfError::Invalid("material property list")),
    }
}
pub(super) fn lookup<'a>(
    p: &'a Parsed,
    properties: &'a Value,
    key: &[u8],
    meter: &Meter,
) -> Result<Option<&'a Value>> {
    let items = list(properties)?;
    meter.count_decode((items.len() as u64).saturating_mul(key.len() as u64 + 1))?;
    for pair in items.iter().rev() {
        let [k, v] = list(pair)? else {
            return Err(GltfError::Invalid("material property pair"));
        };
        let k = if let Value::Str(s) = k {
            s
        } else {
            string(attr(p, k, b"name", meter)?)?
        };
        if k.as_bytes() == key {
            return Ok(Some(v));
        }
    }
    Ok(None)
}
/// A saved texture reference and Unity ST, still in Unity coordinates.
#[derive(Debug, Clone, Copy)]
pub struct TextureInput {
    pub pointer: PPtr,
    pub scale: [f64; 2],
    pub offset: [f64; 2],
}
const _: () = crate::send_sync::<TextureInput>();
/// A material's borrowed source facts. Property reads follow object-mode clean
/// names, last duplicate wins, including `FastPropertyName` keys.
pub struct MaterialInput<'a> {
    pub name: &'a RawStr,
    pub shader_name: &'a [u8],
    pub render_queue: i64,
    pub keywords: Option<&'a Value>,
    pub source: &'a ObjectRef,
    pub(super) parsed: &'a Parsed,
    pub(super) saved: &'a Value,
    pub(super) meter: &'a Meter,
}
const _: () = crate::send_sync::<MaterialInput<'static>>();
impl<'a> MaterialInput<'a> {
    pub(super) fn new(
        p: &'a Parsed,
        source: &'a ObjectRef,
        shader: &'a [u8],
        meter: &'a Meter,
    ) -> Result<Self> {
        let root = p.value();
        let render_queue = optional(p, root, b"m_CustomRenderQueue", meter)?
            .map_or(Ok(-1), |v| {
                v.as_i64().ok_or(GltfError::Invalid("material queue"))
            })?;
        Ok(Self {
            name: string(attr(p, root, b"m_Name", meter)?)?,
            shader_name: shader,
            render_queue,
            keywords: optional(p, root, b"m_ValidKeywords", meter)?.or(optional(
                p,
                root,
                b"m_ShaderKeywords",
                meter,
            )?),
            source,
            parsed: p,
            saved: attr(p, root, b"m_SavedProperties", meter)?,
            meter,
        })
    }
    /// Exact keyword membership in pre-2021 strings or 2021+ valid-keyword arrays.
    pub fn keyword(&self, key: &[u8]) -> Result<bool> {
        match self.keywords {
            None => Ok(false),
            Some(Value::Str(s)) => {
                self.meter.count_decode(s.len() as u64)?;
                Ok(s.as_bytes()
                    .split(u8::is_ascii_whitespace)
                    .any(|s| s == key))
            }
            Some(v) => {
                let values = list(v)?;
                self.meter
                    .count_decode(values.len() as u64 * (key.len() as u64 + 1))?;
                for value in values {
                    if string(value)?.as_bytes() == key {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }
    /// A property from one of `m_Floats`, `m_Ints`, `m_Colors` or `m_TexEnvs`.
    pub fn property(&self, table: &[u8], key: &[u8]) -> Result<Option<&'a Value>> {
        optional(self.parsed, self.saved, table, self.meter)?
            .map_or(Ok(None), |v| lookup(self.parsed, v, key, self.meter))
    }
    pub fn float(&self, key: &[u8]) -> Result<Option<f64>> {
        self.property(b"m_Floats", key)?
            .or(self.property(b"m_Ints", key)?)
            .map(number)
            .transpose()
    }
    pub fn color(&self, key: &[u8]) -> Result<Option<[f64; 4]>> {
        let Some(v) = self.property(b"m_Colors", key)? else {
            return Ok(None);
        };
        let mut out = [0.; 4];
        for (key, slot) in [b"r", b"g", b"b", b"a"].into_iter().zip(&mut out) {
            *slot = number(attr(self.parsed, v, key, self.meter)?)?;
        }
        Ok(Some(out))
    }
    pub fn texture(&self, key: &[u8]) -> Result<Option<TextureInput>> {
        let Some(v) = self.property(b"m_TexEnvs", key)? else {
            return Ok(None);
        };
        let mut scale = [1.; 2];
        let mut offset = [0.; 2];
        for (field, out) in [
            (b"m_Scale".as_slice(), &mut scale),
            (b"m_Offset", &mut offset),
        ] {
            if let Some(v) = optional(self.parsed, v, field, self.meter)? {
                for (key, slot) in [b"x", b"y"].into_iter().zip(out) {
                    *slot = number(attr(self.parsed, v, key, self.meter)?)?;
                }
            }
        }
        Ok(Some(TextureInput {
            pointer: pointer(
                self.parsed,
                attr(self.parsed, v, b"m_Texture", self.meter)?,
                self.meter,
            )?,
            scale,
            offset,
        }))
    }
}
