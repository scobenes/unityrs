// Ported from Pillow 9.5.0 (MIT-CMU): src/PIL/BmpImagePlugin.py,
// src/PIL/TgaImagePlugin.py, src/PIL/PpmImagePlugin.py,
// src/PIL/PcxImagePlugin.py, src/PIL/DdsImagePlugin.py and
// src/libImaging/PcxEncode.c. Copyright (c) 1995-2003 Fredrik Lundh,
// 1997-2003 Secret Labs AB, and Pillow contributors. See LICENSE-PILLOW.
#![allow(
    clippy::cast_possible_truncation,
    reason = "header sizes are range checked; mode channels and palette indices fit their fields"
)]

use crate::{Image, Mode};

use super::{Exception, Refusal, Sink};

fn ushort(value: u32) -> Result<u16, Refusal> {
    u16::try_from(value).map_err(|_| {
        Refusal::new(
            Exception::StructError,
            "ushort format requires 0 <= number <= 65535",
        )
    })
}

fn ulong(value: u128) -> Result<u32, Refusal> {
    u32::try_from(value).map_err(|_| {
        Refusal::new(
            Exception::StructError,
            "'I' format requires 0 <= number <= 4294967295",
        )
    })
}

// Pillow's encoder.setimage performs these checks after its format header.
fn raw_size(image: &Image<'_>, bits: u32) -> Result<(), Refusal> {
    if image.width() == 0 || image.height() == 0 {
        return Err(Refusal::new(
            Exception::SystemError,
            "tile cannot extend outside image",
        ));
    }
    if image.width() > (i32::MAX as u32 / bits).saturating_sub(7) {
        return Err(Refusal::new(Exception::MemoryError, ""));
    }
    Ok(())
}

fn palette(image: &Image<'_>, out: &mut Sink, bgr: bool, four: bool) {
    for index in 0..256_u16 {
        let base = usize::from(index) * 3;
        let gray = index as u8;
        let fallback = [gray, gray, gray];
        let rgb = image
            .palette()
            .and_then(|p| p.get(base..base + 3))
            .unwrap_or(&fallback);
        if bgr {
            for &value in rgb.iter().rev() {
                out.byte(value);
            }
        } else {
            out.put(rgb);
        }
        if four {
            // getpalette("RGB", "BGRX") uses zero as the reserved byte.
            out.byte(0);
        }
    }
}

fn packed_row(row: &[u8], mode: Mode, out: &mut Sink, alpha: bool) {
    match mode {
        Mode::Rgb => {
            for pixel in row.as_chunks::<3>().0 {
                for &value in pixel.iter().rev() {
                    out.byte(value);
                }
            }
        }
        Mode::Rgba => {
            for [r, g, b, a] in row.as_chunks::<4>().0 {
                out.put(&[*b, *g, *r]);
                if alpha {
                    out.byte(*a);
                }
            }
        }
        _ => out.put(row),
    }
}

pub(crate) fn bmp(image: &Image<'_>, out: &mut Sink, dib: bool) -> Result<(), Refusal> {
    if image.mode() == Mode::La {
        return Err(Refusal::new(
            Exception::OSError,
            "cannot write mode LA as BMP",
        ));
    }
    let bits = image.mode().channels() as u32 * 8;
    let colors = if matches!(image.mode(), Mode::L | Mode::P) {
        256_u32
    } else {
        0
    };
    let row = u128::from(image.width()) * u128::from(bits / 8);
    let stride = (row + 3) & !3;
    let image_size = stride * u128::from(image.height());
    let offset = 14 + 40 + colors * 4;
    if !dib {
        let size = u128::from(offset) + image_size;
        if size > u128::from(u32::MAX) {
            return Err(Refusal::new(
                Exception::ValueError,
                "File size is too large for the BMP format",
            ));
        }
        out.put(b"BM");
        out.le32(size as u32);
        out.le32(0);
        out.le32(offset);
    }
    let image_size = ulong(image_size)?;
    out.le32(40);
    out.le32(image.width());
    out.le32(image.height());
    out.le16(1);
    out.le16(bits as u16);
    out.le32(0);
    out.le32(image_size);
    out.le32(3780);
    out.le32(3780);
    out.le32(colors);
    out.le32(colors);
    if image.mode() == Mode::L {
        // Unlike BGRX palette packing, Pillow repeats L into all four bytes.
        for gray in 0..=255_u8 {
            out.put(&[gray; 4]);
        }
    } else if image.mode() == Mode::P {
        palette(image, out, true, true);
    }
    raw_size(image, bits)?;
    for row in image.pixels().chunks_exact(image.row_bytes()).rev() {
        packed_row(row, image.mode(), out, true);
        out.zeros(((stride - row.len() as u128) & 3) as usize);
    }
    Ok(())
}

pub(crate) fn tga(image: &Image<'_>, out: &mut Sink) -> Result<(), Refusal> {
    let width = ushort(image.width())?;
    let height = ushort(image.height())?;
    let indexed = image.mode() == Mode::P;
    let kind = match image.mode() {
        Mode::P => 1,
        Mode::Rgb | Mode::Rgba => 2,
        Mode::L | Mode::La => 3,
    };
    let bits = image.mode().channels() as u32 * 8;
    out.byte(0);
    out.byte(u8::from(indexed));
    out.byte(kind);
    out.le16(0);
    out.le16(if indexed { 256 } else { 0 });
    out.byte(if indexed { 24 } else { 0 });
    out.le16(0);
    out.le16(0);
    out.le16(width);
    out.le16(height);
    out.byte(bits as u8);
    out.byte(if matches!(image.mode(), Mode::La | Mode::Rgba) {
        8
    } else {
        0
    });
    if indexed {
        palette(image, out, true, false);
    }
    raw_size(image, bits)?;
    for row in image.pixels().chunks_exact(image.row_bytes()).rev() {
        packed_row(row, image.mode(), out, true);
    }
    out.zeros(8);
    out.put(b"TRUEVISION-XFILE.\0");
    Ok(())
}

pub(crate) fn ppm(image: &Image<'_>, out: &mut Sink) -> Result<(), Refusal> {
    match image.mode() {
        Mode::L => out.put(b"P5\n"),
        Mode::Rgb | Mode::Rgba => out.put(b"P6\n"),
        Mode::La => {
            return Err(Refusal::new(
                Exception::OSError,
                "cannot write mode LA as PPM",
            ));
        }
        Mode::P => {
            return Err(Refusal::new(
                Exception::OSError,
                "cannot write mode P as PPM",
            ));
        }
    }
    out.text(format_args!("{} {}\n255\n", image.width(), image.height()));
    let bits = if image.mode() == Mode::L { 8 } else { 24 };
    raw_size(image, bits)?;
    if image.mode() == Mode::Rgba {
        for [r, g, b, _] in image.pixels().as_chunks::<4>().0 {
            out.put(&[*r, *g, *b]);
        }
    } else {
        out.put(image.pixels());
    }
    Ok(())
}

fn run(out: &mut Sink, value: u8, count: u8) {
    if count == 1 && value < 0xc0 {
        out.byte(value);
    } else if count > 0 {
        out.put(&[0xc0 | count, value]);
    }
}

fn pcx_plane(row: &[u8], planes: usize, plane: usize, out: &mut Sink) {
    let mut previous = None;
    let mut count = 0;
    for value in row.iter().skip(plane).step_by(planes).copied() {
        if previous == Some(value) && count < 63 {
            count += 1;
        } else {
            if let Some(previous) = previous {
                run(out, previous, count);
            }
            previous = Some(value);
            count = 1;
        }
    }
    if let Some(previous) = previous {
        run(out, previous, count);
    }
}

pub(crate) fn pcx(image: &Image<'_>, out: &mut Sink) -> Result<(), Refusal> {
    match image.mode() {
        Mode::Rgba => {
            return Err(Refusal::new(
                Exception::ValueError,
                "Cannot save RGBA images as PCX",
            ));
        }
        Mode::La => {
            return Err(Refusal::new(
                Exception::ValueError,
                "Cannot save LA images as PCX",
            ));
        }
        _ => {}
    }
    let xmax = image.width().checked_sub(1).ok_or_else(|| {
        Refusal::new(
            Exception::StructError,
            "ushort format requires 0 <= number <= 65535",
        )
    })?;
    let ymax = image.height().checked_sub(1).ok_or_else(|| {
        Refusal::new(
            Exception::StructError,
            "ushort format requires 0 <= number <= 65535",
        )
    })?;
    let xmax = ushort(xmax)?;
    let ymax = ushort(ymax)?;
    let stride = ushort(image.width() + image.width() % 2)?;
    let width = ushort(image.width())?;
    let height = ushort(image.height())?;
    let planes = if image.mode() == Mode::Rgb { 3 } else { 1 };
    out.put(&[10, 5, 1, 8]);
    out.le16(0);
    out.le16(0);
    out.le16(xmax);
    out.le16(ymax);
    out.le16(100);
    out.le16(100);
    out.zeros(24);
    out.put(&[255; 24]);
    out.byte(0);
    out.byte(planes as u8);
    out.le16(stride);
    out.le16(1);
    out.le16(width);
    out.le16(height);
    out.zeros(54);
    raw_size(image, planes as u32 * 8)?;
    for row in image.pixels().chunks_exact(image.row_bytes()) {
        // PcxEncode.c's do/while exits before the last plane at width one.
        let written_planes = if width == 1 && planes == 3 { 2 } else { planes };
        for plane in 0..written_planes {
            pcx_plane(row, planes, plane, out);
            if width % 2 != 0 {
                out.byte(0);
            }
        }
    }
    if image.mode() == Mode::P {
        out.byte(12);
        palette(image, out, false, false);
    } else if image.mode() == Mode::L {
        out.byte(12);
        for gray in 0..=255_u8 {
            out.put(&[gray; 3]);
        }
    }
    Ok(())
}

pub(crate) fn dds(image: &Image<'_>, out: &mut Sink) -> Result<(), Refusal> {
    if image.mode() == Mode::P {
        return Err(Refusal::new(
            Exception::OSError,
            "cannot write mode P as DDS",
        ));
    }
    let alpha = matches!(image.mode(), Mode::La | Mode::Rgba);
    let luminance = matches!(image.mode(), Mode::L | Mode::La);
    // The pinned plugin advertises three/four channels even for L/LA.
    let bitcount = if alpha { 32 } else { 24 };
    let pitch = ulong(u128::from(image.width()) * (bitcount / 8))?;
    out.put(b"DDS ");
    out.le32(124);
    out.le32(0x100f);
    out.le32(image.height());
    out.le32(image.width());
    out.le32(pitch);
    out.le32(0);
    out.le32(0);
    out.zeros(44);
    out.le32(32);
    out.le32(if luminance { 0x20000 } else { 0x40 } | u32::from(alpha));
    out.le32(0);
    out.le32(bitcount as u32);
    out.le32(0x00ff_0000);
    out.le32(0xff00);
    out.le32(0xff);
    out.le32(if alpha { 0xff00_0000 } else { 0 });
    out.le32(0x1000);
    out.zeros(16);
    raw_size(image, image.mode().channels() as u32 * 8)?;
    packed_row(image.pixels(), image.mode(), out, true);
    Ok(())
}
