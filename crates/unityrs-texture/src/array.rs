// Ported from UnityPy 1.25.2 (MIT): classes/legacy_patch/Texture2DArray.py's slice extraction and enums/GraphicsFormat.py's map. See THIRD_PARTY_LICENSES.
//! Array slices (`classes/legacy_patch/Texture2DArray.py:12-43`) use integer
//! division and range stepping, retaining per-slice mip tails. Platform 0
//! is UnityPy's convention; Switch/Xbox arrays are explicitly Tier U.
//! Empty inline data uses the supplied resource (Tier B). The eight 8-bit
//! integer entries use the corresponding UNorm/SNorm channel counts (B/C),
//! as PARITY section 19.2 specifies. Signed byte conversion is the closed
//! form in section 17.1; no 16-bit decoder is used for those stored bytes.
use crate::{DecodeError, Format, Image, Plan};
use std::sync::Arc;
use unityrs_core::limits::{ChargeError, Meter, MeteredVec, heap_cost};
mod object;
pub use object::images;

/// Array metadata; `data_size` includes every slice's mip chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArrayFields {
    /// `m_Format`, a `GraphicsFormat` rather than `TextureFormat`.
    pub graphics_format: i32,
    /// `m_Width`.
    pub width: i32,
    /// `m_Height`.
    pub height: i32,
    /// `m_Depth`.
    pub depth: i32,
    /// `m_DataSize`.
    pub data_size: u64,
    /// The owning file's platform; Switch and Xbox360 are refused.
    pub platform: i32,
}
const _: () = crate::send_sync::<ArrayFields>();

/// Array mapping, layout or decode failure, without allocating an error message.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum ArrayError {
    /// A missing or wrongly typed object field.
    #[error("invalid Texture2DArray field {0}")]
    Field(&'static str),
    /// Object parsing failed.
    #[error(transparent)]
    Parse(#[from] unityrs_core::walk::ParseError),
    /// Resource resolution failed.
    #[error(transparent)]
    Resource(#[from] unityrs_core::Error),
    /// `GraphicsFormat`'s enum has no such value.
    #[error("unknown GraphicsFormat {0}")]
    UnknownGraphicsFormat(i32),
    /// Known enum member with no entry in UnityPy's map.
    #[error("unmapped GraphicsFormat {0}")]
    UnmappedGraphicsFormat(i32),
    /// No independent reference for either platform's array storage.
    #[error("unsupported Texture2DArray platform {0}")]
    UnsupportedPlatform(i32),
    /// Zero/negative depth, zero step, or host-size overflow.
    #[error("invalid Texture2DArray slice layout")]
    Layout,
    /// No decoder for the mapped format is linked yet.
    #[error("unsupported array TextureFormat {0}")]
    UnsupportedFormat(i32),
    /// A slice failed to decode.
    #[error(transparent)]
    Decode(#[from] DecodeError),
    /// A table or work charge was refused.
    #[error(transparent)]
    Charge(#[from] ChargeError),
}
const _: () = crate::send_sync::<ArrayError>();

/// UnityPy's original map, including its eight wrong 8-bit integer targets
/// (`enums/GraphicsFormat.py:179-277`). Call [`texture_format`] for decoding.
pub fn unitypy_texture_format(value: i32) -> Result<i32, ArrayError> {
    let format = match value {
        1 | 5 => 63,
        2 | 6 => 62,
        3 | 7 => 3,
        4 | 8 => 4,
        9..=12 => 75 + value - 9,
        13 | 21 | 29 => 9,
        14 | 22 | 30 => 72,
        15 | 23 | 31 => 73,
        16 | 24 | 32 => 74,
        17 | 25 | 33 => 79,
        18 | 26 | 34 => 80,
        19 | 27 | 35 => 81,
        20 | 28 | 36 => 82,
        56 | 58 => 8,
        57 | 59 => 14,
        96 | 97 => 10,
        98 | 99 => 11,
        100 | 101 => 12,
        102 => 26,
        104 => 27,
        106 => 24,
        108 | 109 => 25,
        110 | 111 => 30,
        112 | 113 => 32,
        114 | 115 => 31,
        116 | 117 => 33,
        118 => 34,
        119 | 120 => 45,
        121 | 122 => 46,
        123 | 124 => 47,
        125 => 41,
        126 => 42,
        127 => 43,
        128 => 44,
        129 | 130 => 54,
        131 | 132 => 55,
        133 | 134 => 56,
        135 | 136 => 57,
        137 | 138 => 58,
        139 | 140 => 59,
        141 => 21,
        _ => {
            return Err(
                if matches!(value, 0..=52 | 56..=86 | 90..=141 | 145..=151) {
                    ArrayError::UnmappedGraphicsFormat(value)
                } else {
                    ArrayError::UnknownGraphicsFormat(value)
                },
            );
        }
    };
    Ok(format)
}

/// Corrected mapping for the eight 8-bit integer graphics formats, otherwise
/// the original map. The correction's targets are `enums/GraphicsFormat.py:184-191`.
pub fn texture_format(value: i32) -> Result<i32, ArrayError> {
    match value {
        13 => Ok(63),
        14 => Ok(62),
        15 => Ok(3),
        16 => Ok(4),
        17..=20 => Ok(75 + value - 17),
        _ => unitypy_texture_format(value),
    }
}

/// Length-only array plan, with a bound for retained images and the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArrayPlan {
    /// Python range's count (also for a non-divisible data size).
    pub slice_count: usize,
    /// `m_DataSize // m_Depth`, including mip tails.
    pub slice_bytes: usize,
    /// Each slice's final image.
    pub image: Plan,
    /// Upper bound for retained output, table, and one decoder's scratch.
    pub peak_charge: u64,
}
const _: () = crate::send_sync::<ArrayPlan>();

fn slice_plan(format: i32, len: usize, w: i32, h: i32) -> Result<Plan, ArrayError> {
    // UnityPy export/Texture2DConverter.py:254-320 returns empty RGBA
    // before checking data or looking up a decoder. Mapping and slicing
    // still precede this in classes/legacy_patch/Texture2DArray.py:12-43.
    if w == 0 || h == 0 {
        return Ok(crate::plan(Format::Rgba32, len, w, h)?);
    }
    let f = Format::from_texture_format(format).ok_or(ArrayError::UnsupportedFormat(format))?;
    Ok(crate::plan(f, len, w, h)?)
}

/// Plan after resolving inline/resource data, without reading or allocating it.
pub fn plan(fields: &ArrayFields, data_len: usize) -> Result<ArrayPlan, ArrayError> {
    if matches!(fields.platform, crate::SWITCH | crate::XBOX360) {
        return Err(ArrayError::UnsupportedPlatform(fields.platform));
    }
    let format = texture_format(fields.graphics_format)?;
    let depth = u64::try_from(fields.depth)
        .ok()
        .filter(|n| *n > 0)
        .ok_or(ArrayError::Layout)?;
    let step = fields.data_size / depth;
    if step == 0 {
        return Err(ArrayError::Layout);
    }
    let count = fields.data_size.div_ceil(step);
    let end = (count - 1).checked_mul(step).ok_or(ArrayError::Layout)?;
    // Match slicing's clamping: even the final short slice must decode.
    let last_len = (data_len as u64).saturating_sub(end).min(step);
    let image = slice_plan(
        format,
        usize::try_from(last_len).map_err(|_| ArrayError::Layout)?,
        fields.width,
        fields.height,
    )?;
    let slice_count = usize::try_from(count).map_err(|_| ArrayError::Layout)?;
    let slice_bytes = usize::try_from(step).map_err(|_| ArrayError::Layout)?;
    let table = count
        .checked_mul(std::mem::size_of::<Image>() as u64)
        .ok_or(ArrayError::Layout)?;
    let peak_charge = image
        .charge()
        .checked_mul(count)
        .and_then(|n| n.checked_add(heap_cost(table)))
        .and_then(|n| n.checked_add(image.scratch))
        .ok_or(ArrayError::Layout)?;
    Ok(ArrayPlan {
        slice_count,
        slice_bytes,
        image,
        peak_charge,
    })
}

/// Decode each slice from inline data, falling back on a resolved resource
/// when inline is empty. Output table and all pixels are charged before allocation.
pub fn decode(
    fields: &ArrayFields,
    inline: &[u8],
    resource: Option<&[u8]>,
    meter: &Arc<Meter>,
) -> Result<MeteredVec<Image>, ArrayError> {
    let data = if inline.is_empty() {
        resource.unwrap_or(inline)
    } else {
        inline
    };
    let p = plan(fields, data.len())?;
    let format = texture_format(fields.graphics_format)?;
    meter.count_decode(p.slice_count as u64)?;
    let mut images = MeteredVec::with_capacity(meter, p.slice_count)?;
    for i in 0..p.slice_count {
        let start = i.checked_mul(p.slice_bytes).ok_or(ArrayError::Layout)?;
        let end = start.saturating_add(p.slice_bytes).min(data.len());
        let input = data.get(start..end).unwrap_or_default();
        let img = if fields.width == 0 || fields.height == 0 {
            // The same pre-decoder shortcut as slice_plan, including formats
            // without a linked decoder (Texture2DConverter.py:254-320).
            crate::decode(Format::Rgba32, input, fields.width, fields.height, meter)?
        } else {
            let f =
                Format::from_texture_format(format).ok_or(ArrayError::UnsupportedFormat(format))?;
            crate::decode(f, input, fields.width, fields.height, meter)?
        };
        images.push(img)?;
    }
    Ok(images)
}
