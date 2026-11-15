// Ported from Pillow 9.5.0 (MIT-CMU): src/libImaging/Convert.c and src/PIL/Image.py's convert. See LICENSE-PILLOW.
// Copyright (c) 1997-2005 by Secret Labs AB.
// Copyright (c) 1995-1997 by Fredrik Lundh.
//! Borrowed Pillow pixel storage, including modes outside the PNG image model.
use super::WriteError;
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};
use unityrs_imageio::{Image, Mode};

/// A Pillow image's `tobytes()` storage. Integer/float modes use native byte
/// order, `1` uses MSB-first bits with each row rounded up to a byte. Palette
/// entries are RGBA, permitting palette alpha and PA's separate alpha band.
#[derive(Clone, Copy, Debug)]
pub struct Source<'a> {
    width: u32,
    height: u32,
    mode: &'a str,
    bytes: &'a [u8],
    palette: Option<&'a [[u8; 4]; 256]>,
    auxiliary: Option<&'a [u8]>,
}
const _: () = crate::send_sync::<Source<'static>>();
impl<'a> Source<'a> {
    /// Validate the complete storage before any allocation or pixel access.
    pub fn new(
        width: u32,
        height: u32,
        mode: &'a str,
        bytes: &'a [u8],
        palette: Option<&'a [[u8; 4]; 256]>,
    ) -> Result<Self, WriteError> {
        let channels = match mode {
            "1" | "L" | "P" => 1,
            "LA" | "La" | "PA" | "I;16" | "I;16L" | "I;16B" | "I;16N" | "BGR;15" | "BGR;16" => 2,
            "RGB" | "LAB" | "HSV" | "YCbCr" | "BGR;24" => 3,
            "RGBA" | "RGBX" | "RGBa" | "CMYK" | "I" | "F" => 4,
            _ => return Err(WriteError::Unsupported("image mode")),
        };
        let row = if mode == "1" {
            u64::from(width).div_ceil(8)
        } else {
            u64::from(width) * channels
        };
        let len = row
            .checked_mul(u64::from(height))
            .ok_or(WriteError::Invalid("image size"))?;
        if len != bytes.len() as u64 {
            return Err(WriteError::Invalid("pixel length"));
        }
        if width > i32::MAX as u32
            || height > i32::MAX as u32
            || u64::from(width) * u64::from(height) > crate::MAX_PIXELS
        {
            return Err(WriteError::Invalid("image dimensions exceed bounds"));
        }
        Ok(Self {
            width,
            height,
            mode,
            bytes,
            palette,
            auxiliary: None,
        })
    }
    /// Preserve Pillow's otherwise unexported fourth storage byte of `RGB` or
    /// `LAB`. `frombytes` initializes it to 255; `Image.new` initializes it to
    /// zero. Raw RGBA packing and `ImageCms` copy it. Other modes ignore it.
    pub fn with_auxiliary(mut self, bytes: &'a [u8]) -> Result<Self, WriteError> {
        if bytes.len() as u64 != u64::from(self.width) * u64::from(self.height) {
            return Err(WriteError::Invalid("auxiliary band length"));
        }
        self.auxiliary = Some(bytes);
        Ok(self)
    }
    pub(super) const fn auxiliary(&self) -> Option<&'a [u8]> {
        self.auxiliary
    }
    /// Image width.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }
    /// Image height.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }
    /// Pillow's mode name.
    #[must_use]
    pub const fn mode(&self) -> &str {
        self.mode
    }

    pub(super) fn check_convertible(&self) -> Result<(), WriteError> {
        if matches!(self.mode, "BGR;15" | "BGR;16" | "BGR;24" | "La") {
            return Err(WriteError::Unsupported(
                "Pillow cannot convert image mode to RGBA",
            ));
        }
        Ok(())
    }
    pub(super) fn prepare(
        &self,
        channels: usize,
        alpha: bool,
        meter: &Arc<Meter>,
    ) -> Result<Prepared, WriteError> {
        self.check_convertible()?;
        let direct = if alpha {
            matches!(self.mode, "RGBA" | "LAB")
        } else if channels == 4 {
            matches!(self.mode, "RGB" | "RGBA")
        } else {
            matches!(self.mode, "RGB" | "RGBX" | "RGBA")
        };
        let lab_alpha = alpha && self.mode == "LAB";
        let mode = if lab_alpha || (direct && self.mode != "RGBA" && !alpha) {
            Mode::Rgb
        } else {
            Mode::Rgba
        };
        let len = self.width as usize * self.height as usize * mode.channels();
        meter.count_decode(len as u64)?;
        let mut bytes = MeteredVec::with_capacity(meter, len)?;
        for n in 0..self.width as usize * self.height as usize {
            if lab_alpha {
                let p = self
                    .bytes
                    .get(n * 3..n * 3 + 3)
                    .ok_or(WriteError::Invalid("LAB pixel"))?;
                for (i, &b) in p.iter().enumerate() {
                    bytes.push(if i == 0 { b } else { b ^ 128 })?;
                }
            } else {
                let rgba = self.rgba(n)?;
                bytes.extend_from_slice(
                    rgba.get(..mode.channels())
                        .ok_or(WriteError::Invalid("converted pixel"))?,
                )?;
            }
        }
        Ok(Prepared {
            width: self.width,
            height: self.height,
            mode,
            bytes,
            alpha_band: if lab_alpha { 1 } else { 3 },
        })
    }

    fn rgba(&self, n: usize) -> Result<[u8; 4], WriteError> {
        let stride = match self.mode {
            "L" | "P" | "1" => 1,
            "LA" | "PA" | "I;16" | "I;16L" | "I;16B" | "I;16N" => 2,
            "RGB" | "YCbCr" | "HSV" | "LAB" => 3,
            _ => 4,
        };
        let pixel = self.bytes.get(n * stride..n * stride + stride);
        let byte = |i| pixel.and_then(|p| p.get(i)).copied().unwrap_or(0);
        let grey = |v| [v, v, v, 255];
        Ok(match self.mode {
            "1" => {
                let x = n % self.width as usize;
                let y = n / self.width as usize;
                let value = self
                    .bytes
                    .get(y * (self.width as usize).div_ceil(8) + x / 8)
                    .copied()
                    .unwrap_or(0);
                grey(if value & (128 >> (x % 8)) == 0 {
                    0
                } else {
                    255
                })
            }
            "L" => grey(byte(0)),
            "LA" => [byte(0), byte(0), byte(0), byte(1)],
            "P" | "PA" => {
                let mut color = self.palette.map_or_else(
                    || grey(byte(0)),
                    |p| p.get(usize::from(byte(0))).copied().unwrap_or([0; 4]),
                );
                if self.mode == "PA" {
                    color[3] = byte(1);
                }
                color
            }
            "RGB" | "RGBX" => [byte(0), byte(1), byte(2), 255],
            "RGBA" => [byte(0), byte(1), byte(2), byte(3)],
            "RGBa" => {
                let a = u32::from(byte(3));
                let unpremultiply = |c| {
                    if a == 0 || a == 255 {
                        c
                    } else {
                        u8::try_from((u32::from(c) * 255 / a).min(255)).unwrap_or(255)
                    }
                };
                [
                    unpremultiply(byte(0)),
                    unpremultiply(byte(1)),
                    unpremultiply(byte(2)),
                    byte(3),
                ]
            }
            "CMYK" => {
                let k = 255 - u32::from(byte(3));
                let convert = |c| {
                    let t = u32::from(c) * k + 128;
                    u8::try_from(k - ((t + (t >> 8)) >> 8)).unwrap_or(0)
                };
                [convert(byte(0)), convert(byte(1)), convert(byte(2)), 255]
            }
            "I" => grey(
                u8::try_from(
                    i32::from_ne_bytes([byte(0), byte(1), byte(2), byte(3)]).clamp(0, 255),
                )
                .unwrap_or(0),
            ),
            "F" => {
                let v = f32::from_ne_bytes([byte(0), byte(1), byte(2), byte(3)]);
                grey(float_byte(v))
            }
            "I;16" | "I;16L" | "I;16B" | "I;16N" => {
                let raw = [byte(0), byte(1)];
                let v = match self.mode {
                    "I;16B" => u16::from_be_bytes(raw),
                    "I;16N" => u16::from_ne_bytes(raw),
                    _ => u16::from_le_bytes(raw),
                };
                grey(u8::try_from(v).unwrap_or(255))
            }
            "HSV" => hsv(byte(0), byte(1), byte(2)),
            "YCbCr" => super::ycbcr::convert(byte(0), byte(1), byte(2)),
            "LAB" => {
                let mut color = super::lab::convert(byte(0), byte(1), byte(2));
                color[3] = self
                    .auxiliary
                    .and_then(|b| b.get(n))
                    .copied()
                    .unwrap_or(255);
                color
            }
            _ => return Err(WriteError::Unsupported("image mode conversion")),
        })
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn float_byte(v: f32) -> u8 {
    v.clamp(0.0, 255.0) as u8
}
// C promotes expressions containing double literals, then assigns f/fs as floats.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]
fn hsv(h: u8, s: u8, v: u8) -> [u8; 4] {
    if s == 0 {
        return [v, v, v, 255];
    }
    let sector = f64::from(h) * 6.0 / 255.0;
    let i = sector.floor() as u32;
    let f = (sector - f64::from(i)) as f32;
    let fs = (f64::from(s) / 255.0) as f32;
    let p = (f64::from(v) * (1.0 - f64::from(fs))).round() as u8;
    let q = (f64::from(v) * (1.0 - f64::from(fs * f))).round() as u8;
    let t = (f64::from(v) * (1.0 - f64::from(fs) * (1.0 - f64::from(f)))).round() as u8;
    let [r, g, b] = match i % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    };
    [r, g, b, 255]
}
pub(super) struct Prepared {
    width: u32,
    height: u32,
    pub mode: Mode,
    bytes: MeteredVec<u8>,
    pub alpha_band: usize,
}
impl Prepared {
    pub fn image(&self) -> Result<Image<'_>, WriteError> {
        Image::new(self.width, self.height, self.mode, self.bytes.as_slice())
            .map_err(|_| WriteError::Invalid("prepared image"))
    }
}
