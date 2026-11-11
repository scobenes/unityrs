// Ported from UnityPy 1.25.2 (MIT): UnityPy/helpers/MeshHelper.py's MeshHandler.get_streams, read_vertex_data and assign_channel_vertex_data. See THIRD_PARTY_LICENSES.
//! The channels' and streams' layout, and each channel's data unpacked
//! into its attribute.
//!
//! **Streams** (`get_streams`, `helpers/MeshHelper.py:256-281`). From 5,
//! the handler derives the streams from the channels: as many as the
//! highest stream a channel names, plus one; each stream's stride is the
//! sum, over its channels of a non-zero dimension, of the dimension's low
//! four bits times the format's component size, and the streams follow
//! one another, each offset rounded up to 16. UnityPy visits every stream
//! and, within it, every channel, reading each channel's `dimension` and,
//! when it is not 0, its format, so a channel's missing field or a format
//! the version's enum lacks raises at the first stream, then channel, that
//! has one. [`get_streams`] reads each channel once and keeps the first
//! failure in that order, so it runs in the channels' count, not times the
//! streams'. At 4, the streams are the file's own (`StreamInfo`s).
//!
//! **Each channel** (`read_vertex_data`, `:323-393`). Nothing is read when
//! the vertex count is 0 or the data empty. A channel of dimension 0 is
//! skipped; any other is read when its bit is set in its stream's mask,
//! which UnityPy tests on the string `bin(channelMask)[::-1]`, so a channel
//! past the mask's highest bit reads `b` and then `0`, and one further on
//! raises `IndexError` ([`mask_bit`]); a derived stream's mask has the bit
//! of every channel of a non-zero dimension it holds. Below 2018 the colour
//! channel (2) of format 2 is read as 4 components whatever its
//! dimension (`:348-357`). The data is then gathered ([`crate::unpack`]),
//! and assigned by channel index ([`assign`]).
//!
//! **Work.** [`get_streams`] counts one unit a channel before its two
//! passes, and charges the streams it derives before it allocates them;
//! [`read_vertex_data`] counts one unit a channel, those of dimension 0 it
//! skips included, before its loop; and every field read counts its scans
//! (`view`'s module docs).

use std::sync::Arc;

use unityrs_core::limits::{Charge, Meter, MeteredVec};

use crate::Version;
use crate::error::{IndexKind, MeshError};
use crate::format::get_channel_dtype;
use crate::unpack::{Gather, VertexArray, unpack_channel};
use crate::view::{List, Obj};

/// The most streams a mesh can have: a channel's `stream` is a `UInt8`, so
/// a derived layout has at most 256, a fixed bound.
const MAX_STREAMS: usize = 256;

/// A stream derived by [`get_streams`]: its offset and stride, Python
/// `int`s (unbounded there, so a `u128` and a `u64` here), and implicitly
/// the mask of its channels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DerivedStream {
    pub(crate) offset: u128,
    pub(crate) stride: u64,
}

/// Where each channel's stream comes from.
#[derive(Debug)]
pub(crate) enum Streams<'a> {
    /// Derived from the channels (5 and up), at most [`MAX_STREAMS`], with
    /// the charge for their memory.
    Derived {
        streams: Vec<DerivedStream>,
        // Declared last: the charge is returned after the streams are freed.
        _charge: Charge,
    },
    /// The file's own `m_Streams` (4.x).
    File(List<'a>),
}

/// The attributes the channels fill, as the handler's fields are named.
#[derive(Debug, Default)]
pub(crate) struct Attributes {
    pub(crate) vertices: Option<VertexArray>,
    pub(crate) normals: Option<VertexArray>,
    pub(crate) tangents: Option<VertexArray>,
    pub(crate) colors: Option<VertexArray>,
    pub(crate) uv: [Option<VertexArray>; 8],
    pub(crate) bone_weights: Option<VertexArray>,
    pub(crate) bone_indices: Option<VertexArray>,
}

/// A required integer field: absent only on an `UnknownObject`, which
/// [`Obj::get`] has refused, so `None` is a value missing from the dict.
pub(crate) fn field(obj: &Obj<'_>, name: &'static str, max: u64) -> Result<u64, MeshError> {
    obj.uint(name, max)?.ok_or(MeshError::NoAttribute(name))
}

/// `get_streams(m_Channels, m_VertexCount)` (`helpers/MeshHelper.py:256-281`,
/// module docs).
pub(crate) fn get_streams(
    version: Version,
    channels: &List<'_>,
    vertex_count: u64,
    meter: &Arc<Meter>,
) -> Result<Streams<'static>, MeshError> {
    // Each channel is read once in each of the two loops below, in a fixed
    // amount of work.
    meter.count_decode(u64::try_from(channels.len()).unwrap_or(u64::MAX))?;
    // UnityPy MeshHelper.py:257: `max` reads every channel's `stream`.
    let mut highest: Option<u64> = None;
    for ch in channels.iter() {
        let s = field(&ch?, "stream", 255)?;
        highest = Some(highest.map_or(s, |h| h.max(s)));
    }
    let count = usize::try_from(highest.ok_or(MeshError::NoChannels)?).unwrap_or(0) + 1;
    // UnityPy MeshHelper.py:261-268: stream `s` visits each of its
    // channels in order; the first failure by (stream, channel) is the one
    // UnityPy raises.
    let mut strides = [0u64; MAX_STREAMS];
    let mut first: Option<(u64, MeshError)> = None;
    for ch in channels.iter() {
        let ch = ch?;
        let s = field(&ch, "stream", 255)?;
        let stride = (|| {
            let dim = field(&ch, "dimension", 255)?;
            if dim == 0 {
                return Ok(0);
            }
            let format = u8::try_from(field(&ch, "format", 255)?).unwrap_or(u8::MAX);
            let size = get_channel_dtype(version, format)?.size();
            Ok((dim & 0xF) * u64::from(size))
        })();
        match stride {
            Ok(add) => {
                if let Some(slot) = usize::try_from(s).ok().and_then(|i| strides.get_mut(i)) {
                    *slot = slot.saturating_add(add);
                }
            }
            // The channels are visited in order, so the first failure of a
            // stream is the earliest of its channels.
            Err(e) => {
                if first.as_ref().is_none_or(|(at, _)| s < *at) {
                    first = Some((s, e));
                }
            }
        }
    }
    if let Some((_, e)) = first {
        return Err(e);
    }
    // UnityPy MeshHelper.py:270-280: each stream after the last, at the
    // next multiple of 16.
    let mut streams = MeteredVec::<DerivedStream>::with_capacity(meter, count)?;
    let mut offset: u128 = 0;
    for &stride in strides.get(..count).unwrap_or(&[]) {
        streams.push(DerivedStream { offset, stride })?;
        offset = offset.saturating_add(u128::from(vertex_count) * u128::from(stride));
        offset = offset.saturating_add(15) & !15;
    }
    let (streams, charge) = streams.into_parts();
    Ok(Streams::Derived {
        streams,
        _charge: charge,
    })
}

/// `bin(channelMask)[::-1][chn] == "1"` (`helpers/MeshHelper.py:346-347`):
/// the bit for a channel below the mask's length in bits (1 for a mask of
/// 0), `b` then `0` for the next two, `IndexError` past them.
pub(crate) fn mask_bit(mask: u64, chn: usize) -> Result<bool, MeshError> {
    let bits = usize::try_from(mask.bit_width()).unwrap_or(0).max(1);
    if chn >= bits + 2 {
        return Err(MeshError::Index(IndexKind::Str));
    }
    Ok(chn < bits && (mask >> chn) & 1 == 1)
}

/// The vertex data a handler reads: its `m_VertexData`, and the bytes
/// `m_StreamData` put in place of its `m_DataSize`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct VertexData<'a> {
    pub(crate) obj: Obj<'a>,
    pub(crate) streamed: Option<&'a [u8]>,
}

/// `read_vertex_data(m_Channels, m_Streams)` (`helpers/MeshHelper.py:323-393`,
/// module docs): the vertex count it sets, if it reads at all.
pub(crate) fn read_vertex_data(
    version: Version,
    little: bool,
    vertex_data: VertexData<'_>,
    channels: &List<'_>,
    streams: &Streams<'_>,
    attrs: &mut Attributes,
    meter: &Arc<Meter>,
) -> Result<Option<u64>, MeshError> {
    let vd = vertex_data.obj;
    // UnityPy MeshHelper.py:331-332: no count, or no data, reads nothing.
    let count = field(&vd, "m_VertexCount", u64::from(u32::MAX))?;
    if count == 0 {
        return Ok(None);
    }
    let data = match vertex_data.streamed {
        Some(b) => b,
        None => vd
            .bytes("m_DataSize")?
            .ok_or(MeshError::NoAttribute("m_DataSize"))?,
    };
    if data.is_empty() {
        return Ok(None);
    }
    let vertex_count = u32::try_from(count).unwrap_or(u32::MAX);
    // One unit a channel, the ones skipped included.
    meter.count_decode(u64::try_from(channels.len()).unwrap_or(u64::MAX))?;
    for (chn, ch) in channels.iter().enumerate() {
        let ch = ch?;
        // UnityPy MeshHelper.py:338-339
        if field(&ch, "dimension", 255)? == 0 {
            continue;
        }
        let stream = field(&ch, "stream", 255)?;
        let stream = usize::try_from(stream).unwrap_or(usize::MAX);
        // UnityPy MeshHelper.py:338-347: the stream, and its mask's bit.
        let file_stream = match streams {
            Streams::Derived { .. } => None,
            Streams::File(list) => {
                let s = list
                    .get(stream)
                    .ok_or(MeshError::Index(IndexKind::List))??;
                let mask = field(&s, "channelMask", u64::from(u32::MAX))?;
                if !mask_bit(mask, chn)? {
                    continue;
                }
                Some(s)
            }
        };
        // UnityPy MeshHelper.py:348-357: below 2018 the colour channel of
        // format 2 is a new `ChannelInfo` of dimension 4, which reads the
        // channel's offset (and its stream, read above).
        let colour = version.major()? < 2018 && chn == 2 && field(&ch, "format", 255)? == 2;
        let (dimension, format) = if colour {
            field(&ch, "offset", 255)?;
            (4, 2)
        } else {
            (field(&ch, "dimension", 255)?, field(&ch, "format", 255)?)
        };
        // UnityPy MeshHelper.py:359-364
        let code = get_channel_dtype(version, u8::try_from(format).unwrap_or(u8::MAX))?;
        let size = code.size();
        let swap = little && size > 1;
        let channel_dimension = u32::try_from(dimension & 0xF).unwrap_or(0);
        // UnityPy MeshHelper.py:366-376: the stream's offset and stride,
        // then the channel's offset, each masked to 32 bits by `I`.
        let (stream_offset, stream_stride) = match (file_stream, streams) {
            (Some(s), _) => (
                field(&s, "offset", u64::from(u32::MAX))?,
                field(&s, "stride", u64::from(u32::MAX))?,
            ),
            (None, Streams::Derived { streams: v, .. }) => {
                let s = v.get(stream).copied().unwrap_or_default();
                (low32(s.offset), low32(u128::from(s.stride)))
            }
            (None, Streams::File(_)) => (0, 0),
        };
        let channel_offset = field(&ch, "offset", 255)?;
        let args = Gather {
            component_byte_size: size,
            vertex_count,
            stream_offset: u32::try_from(stream_offset).unwrap_or(0),
            stream_stride: u32::try_from(stream_stride).unwrap_or(0),
            channel_offset: u32::try_from(channel_offset).unwrap_or(0),
            channel_dimension,
            swap,
        };
        let array = unpack_channel(data, &args, code, meter)?;
        assign(version, chn, array, attrs)?;
    }
    Ok(Some(count))
}

/// An `int`'s low 32 bits, as Python's `I` format takes them.
fn low32(v: u128) -> u64 {
    u64::try_from(v & 0xFFFF_FFFF).unwrap_or(0)
}

/// `assign_channel_vertex_data(channel, data)` (`helpers/MeshHelper.py:395-449`):
/// from 2018, channels 0 to 13 are the position, normal, tangent, colour,
/// eight UVs, bone weights and bone indices; below, 0 to 7 are the
/// position, normal, colour, UV0, UV1, then UV2 from 5 (the tangent below
/// 5), UV3 and the tangent. Any other raises `ValueError`.
pub(crate) fn assign(
    version: Version,
    channel: usize,
    data: VertexArray,
    attrs: &mut Attributes,
) -> Result<(), MeshError> {
    let major = version.major()?;
    let slot = if major >= 2018 {
        match channel {
            0 => &mut attrs.vertices,
            1 => &mut attrs.normals,
            2 => &mut attrs.tangents,
            3 => &mut attrs.colors,
            4..=11 => uv(attrs, channel - 4),
            12 => &mut attrs.bone_weights,
            13 => &mut attrs.bone_indices,
            _ => return Err(unknown(channel)),
        }
    } else {
        match channel {
            0 => &mut attrs.vertices,
            1 => &mut attrs.normals,
            2 => &mut attrs.colors,
            3 => uv(attrs, 0),
            4 => uv(attrs, 1),
            5 if major >= 5 => uv(attrs, 2),
            5 | 7 => &mut attrs.tangents,
            6 => uv(attrs, 3),
            _ => return Err(unknown(channel)),
        }
    };
    *slot = Some(data);
    Ok(())
}

fn uv(attrs: &mut Attributes, i: usize) -> &mut Option<VertexArray> {
    let [u0, u1, u2, u3, u4, u5, u6, u7] = &mut attrs.uv;
    match i {
        0 => u0,
        1 => u1,
        2 => u2,
        3 => u3,
        4 => u4,
        5 => u5,
        6 => u6,
        _ => u7,
    }
}

fn unknown(channel: usize) -> MeshError {
    MeshError::UnknownChannel(u64::try_from(channel).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::mask_bit;
    use crate::error::{IndexKind, MeshError};

    #[test]
    fn the_mask_is_read_as_its_reversed_binary_string() {
        // bin(0b101)[::-1] == "101b0"
        assert!(mask_bit(0b101, 0).unwrap());
        assert!(!mask_bit(0b101, 1).unwrap());
        assert!(mask_bit(0b101, 2).unwrap());
        assert!(!mask_bit(0b101, 3).unwrap());
        assert!(!mask_bit(0b101, 4).unwrap());
        assert!(matches!(
            mask_bit(0b101, 5),
            Err(MeshError::Index(IndexKind::Str))
        ));
        // bin(0)[::-1] == "0b0"
        assert!(!mask_bit(0, 0).unwrap());
        assert!(!mask_bit(0, 2).unwrap());
        assert!(mask_bit(0, 3).is_err());
        assert!(mask_bit(u64::from(u32::MAX), 31).unwrap());
        assert!(!mask_bit(u64::from(u32::MAX), 33).unwrap());
        assert!(mask_bit(u64::from(u32::MAX), 34).is_err());
    }
}
