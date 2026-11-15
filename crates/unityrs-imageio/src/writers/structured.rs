// Ported from Pillow 9.5.0 (MIT-CMU): src/PIL/SgiImagePlugin.py,
// src/PIL/ImImagePlugin.py, src/PIL/TiffImagePlugin.py,
// src/PIL/EpsImagePlugin.py, src/libImaging/Pack.c and EpsEncode.c.
// Copyright (c) 1995-2006 by Fredrik Lundh and Secret Labs AB.
// SgiImagePlugin.py: Copyright (c) 2016 by Mickael Bonfill.
// Copyright (c) 2008 by Karsten Hiddemann.
// Copyright (c) 1997 by Secret Labs AB.
// Copyright (c) 1995 by Fredrik Lundh.
// See LICENSE-PILLOW for the permission notice.
#![allow(
    clippy::cast_possible_truncation,
    reason = "SGI dimensions are checked, mode channels and palette indices fit their fields"
)]

use super::{Exception, Refusal, Sink};
use crate::{Image, Mode};

/// POSIX basename and splitext, including Python's leading-dot rule.
fn filename_parts(filename: &str) -> (&str, &str) {
    let basename = filename.rsplit('/').next().unwrap_or("");
    if let Some(dot) = basename.rfind('.')
        && basename
            .get(..dot)
            .is_some_and(|s| s.chars().any(|c| c != '.'))
    {
        return (
            basename.get(..dot).unwrap_or(""),
            basename.get(dot..).unwrap_or(""),
        );
    }
    (basename, "")
}

fn raw_tile(image: &Image<'_>) -> Result<(), Refusal> {
    nonempty(image)?;
    let bits = image.mode().channels() as u32 * 8;
    if image.width() > i32::MAX as u32 / bits - 7 {
        return Err(Refusal::new(Exception::MemoryError, ""));
    }
    Ok(())
}

fn nonempty(image: &Image<'_>) -> Result<(), Refusal> {
    if image.width() == 0 || image.height() == 0 {
        return Err(Refusal::new(
            Exception::SystemError,
            "tile cannot extend outside image",
        ));
    }
    Ok(())
}

pub(super) fn sgi(image: &Image<'_>, out: &mut Sink, filename: &str) -> Result<(), Refusal> {
    if !matches!(image.mode(), Mode::L | Mode::Rgb | Mode::Rgba) {
        return Err(Refusal::new(
            Exception::ValueError,
            "Unsupported SGI image mode",
        ));
    }
    if image.width() > u16::MAX.into() || image.height() > u16::MAX.into() {
        return Err(Refusal::new(
            Exception::StructError,
            "'H' format requires 0 <= number <= 65535",
        ));
    }
    let dimensions = if image.mode() == Mode::L {
        if image.height() == 1 { 1 } else { 2 }
    } else {
        3
    };
    out.be16(474);
    out.byte(0);
    out.byte(1);
    out.be16(dimensions);
    out.be16(image.width() as u16);
    out.be16(image.height() as u16);
    out.be16(image.mode().channels() as u16);
    out.be32(0);
    out.be32(255);
    out.zeros(4);
    let (name, _) = filename_parts(filename);
    let mut count = 0;
    for byte in name.bytes().filter(u8::is_ascii).take(79) {
        out.byte(byte);
        count += 1;
    }
    out.zeros(80 - count);
    out.be32(0);
    out.zeros(404);
    // SGI writes each entire band, bottom row first. Unlike raw tiles,
    // Pillow's channel.tobytes() permits an image with an empty side.
    let row_bytes = image.row_bytes();
    if row_bytes != 0 {
        for channel in 0..image.mode().channels() {
            for row in image.pixels().chunks_exact(row_bytes).rev() {
                for pixel in row.chunks_exact(image.mode().channels()) {
                    if let Some(value) = pixel.get(channel) {
                        out.byte(*value);
                    }
                }
            }
        }
    }
    Ok(())
}

pub(super) fn im(image: &Image<'_>, out: &mut Sink, filename: &str) -> Result<(), Refusal> {
    let image_type = match image.mode() {
        Mode::L | Mode::P => "Greyscale",
        Mode::La => "LA",
        Mode::Rgb => "RGB",
        Mode::Rgba => "RGBA",
    };
    out.text(format_args!("Image type: {image_type} image\r\n"));
    if !filename.is_empty() {
        let (name, ext) = filename_parts(filename);
        let ext_len = ext.chars().count();
        let kept = if ext_len <= 92 {
            name.chars().count().min(92 - ext_len)
        } else {
            // Python's name[:negative] removes that many trailing chars.
            name.chars().count().saturating_sub(ext_len - 92)
        };
        if name
            .chars()
            .take(kept)
            .chain(ext.chars())
            .any(|c| !c.is_ascii())
        {
            return Err(Refusal::new(
                Exception::UnicodeEncodeError,
                "'ascii' codec can't encode characters in filename",
            ));
        }
        out.put(b"Name: ");
        for c in name.chars().take(kept).chain(ext.chars()) {
            out.byte(c as u8);
        }
        out.put(b"\r\n");
    }
    out.text(format_args!(
        "Image size (x*y): {}*{}\r\nFile size (no of images): 1\r\n",
        image.width(),
        image.height()
    ));
    if image.mode() == Mode::P {
        out.put(b"Lut: 1\r\n");
    }
    out.zeros(511_usize.saturating_sub(out.len()));
    out.byte(26);
    if image.mode() == Mode::P {
        palette(image, out, false);
    }
    raw_tile(image)?;
    // The ;L raw packers planarize each row, not the whole image.
    for row in image.pixels().chunks_exact(image.row_bytes()).rev() {
        for channel in 0..image.mode().channels() {
            for pixel in row.chunks_exact(image.mode().channels()) {
                if let Some(value) = pixel.get(channel) {
                    out.byte(*value);
                }
            }
        }
    }
    Ok(())
}

fn palette(image: &Image<'_>, out: &mut Sink, words: bool) {
    // RGB;L is three 256-entry planes; TIFF scales each byte by 256.
    for channel in 0..3 {
        for index in 0..256 {
            let value = image
                .palette()
                .and_then(|p| p.get(index * 3 + channel))
                .copied()
                .unwrap_or(index as u8);
            if words {
                out.le16(u16::from(value) * 256);
            } else {
                out.byte(value);
            }
        }
    }
}

fn entry(out: &mut Sink, tag: u16, kind: u16, count: u32, value: u32) {
    out.le16(tag);
    out.le16(kind);
    out.le32(count);
    out.le32(value);
}

pub(super) fn tiff(image: &Image<'_>, out: &mut Sink) -> Result<(), Refusal> {
    let channels = image.mode().channels() as u32;
    let extra = matches!(image.mode(), Mode::La | Mode::Rgba);
    let indexed = image.mode() == Mode::P;
    let entries = 9 + u16::from(channels > 1) + u16::from(extra) + u16::from(indexed);
    let directory_end = 8 + 2 + u32::from(entries) * 12 + 4;
    let bits_len = if channels > 2 { channels * 2 } else { 0 };
    let data_start = directory_end + bits_len + if indexed { 1536 } else { 0 };
    let length = u32::try_from(image.pixels().len()).map_err(|_| {
        Refusal::new(
            Exception::StructError,
            "'L' format requires 0 <= number <= 4294967295",
        )
    })?;
    out.put(b"II\x2a\0\x08\0\0\0");
    out.le16(entries);
    entry(out, 256, 4, 1, image.width());
    entry(out, 257, 4, 1, image.height());
    let bits = if channels == 1 {
        8
    } else if channels == 2 {
        8 | (8 << 16)
    } else {
        directory_end
    };
    entry(out, 258, 3, channels, bits);
    entry(out, 259, 3, 1, 1);
    let photo = if indexed {
        3
    } else if channels >= 3 {
        2
    } else {
        1
    };
    entry(out, 262, 3, 1, photo);
    entry(out, 273, 4, 1, data_start);
    if channels > 1 {
        entry(out, 277, 3, 1, channels);
    }
    entry(out, 278, 4, 1, image.height().max(1));
    // These tags are LONG even for values smaller than 65536: TiffTags
    // assigns their types before the save plugin's explicit promotion.
    entry(out, 279, 4, 1, length);
    entry(out, 284, 3, 1, 1);
    if indexed {
        entry(out, 320, 3, 768, directory_end);
    }
    if extra {
        entry(out, 338, 3, 1, 2);
    }
    out.le32(0);
    if channels > 2 {
        for _ in 0..channels {
            out.le16(8);
        }
    }
    if indexed {
        palette(image, out, true);
    }
    raw_tile(image)?;
    out.put(image.pixels());
    Ok(())
}

pub(super) fn eps(image: &Image<'_>, out: &mut Sink) -> Result<(), Refusal> {
    let operator = match image.mode() {
        Mode::L => "image",
        Mode::Rgb => "false 3 colorimage",
        _ => {
            return Err(Refusal::new(
                Exception::ValueError,
                "image mode is not supported",
            ));
        }
    };
    let width = image.width();
    let height = image.height();
    let channels = image.mode().channels();
    out.put(b"%!PS-Adobe-3.0 EPSF-3.0\n%%Creator: PIL 0.1 EpsEncode\n");
    out.text(format_args!("%%BoundingBox: 0 0 {width} {height}\n"));
    out.put(b"%%Pages: 1\n%%EndComments\n%%Page: 1 1\n");
    out.text(format_args!(
        "%ImageData: {width} {height} 8 {channels} 0 1 1 \"{operator}\"\n"
    ));
    out.text(format_args!(
        "gsave\n10 dict begin\n/buf {} string def\n{width} {height} scale\n{width} {height} 8\n[{width} 0 0 -{height} 0 {height}]\n{{ currentfile buf readhexstring pop }} bind\n{operator}\n",
        u64::from(width) * channels as u64
    ));
    nonempty(image)?;
    // EpsEncode.c emits a newline before the next byte after each 39
    // bytes, so a final full line receives only the Python footer's LF.
    for (index, value) in image.pixels().iter().enumerate() {
        if index != 0 && index % 39 == 0 {
            out.byte(b'\n');
        }
        for nibble in [value >> 4, value & 15] {
            out.byte(if nibble < 10 {
                b'0' + nibble
            } else {
                b'a' + nibble - 10
            });
        }
    }
    out.put(b"\n%%%%EndBinary\ngrestore end\n");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Exception, Image, Mode, Sink, eps, filename_parts, im, sgi, tiff};

    #[test]
    fn pillow_posix_splitext() {
        for (path, expected) in [
            ("/long/path/test.sgi", ("test", ".sgi")),
            ("/.profile", (".profile", "")),
            ("/..a", ("..a", "")),
            ("/.a.b", (".a", ".b")),
            ("a.", ("a", ".")),
            ("a\\b.sgi", ("a\\b", ".sgi")),
            ("/", ("", "")),
        ] {
            assert_eq!(filename_parts(path), expected);
        }
    }

    #[test]
    fn sgi_empty_images_and_name_ascii_ignore() {
        let image = Image::new(0, 1, Mode::L, &[]).unwrap();
        let mut bytes = [0; 512];
        let mut out = Sink::write(&mut bytes);
        sgi(&image, &mut out, "/directory/naïve.sgi").unwrap();
        assert_eq!(out.finish().unwrap(), 512);
        assert_eq!(&bytes[..12], b"\x01\xda\0\x01\0\x01\0\0\0\x01\0\x01");
        assert_eq!(&bytes[24..30], b"nave\0\0");
        let oversized = Image::new(65536, 0, Mode::L, &[]).unwrap();
        assert_eq!(
            sgi(&oversized, &mut Sink::count(), "").unwrap_err().class,
            Exception::StructError
        );
    }

    #[test]
    fn im_row_planes_and_tiff_contiguous_strip() {
        let image = Image::new(2, 2, Mode::Rgb, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]).unwrap();
        let mut bytes = [0; 524];
        let mut out = Sink::write(&mut bytes);
        im(&image, &mut out, "").unwrap();
        assert_eq!(out.finish().unwrap(), 524);
        assert_eq!(&bytes[512..], &[6, 9, 7, 10, 8, 11, 0, 3, 1, 4, 2, 5]);
        let mut bytes = [0; 152];
        let mut out = Sink::write(&mut bytes);
        tiff(&image, &mut out).unwrap();
        assert_eq!(out.finish().unwrap(), 152);
        assert_eq!(&bytes[..10], b"II\x2a\0\x08\0\0\0\x0a\0");
        assert_eq!(&bytes[134..140], b"\x08\0\x08\0\x08\0");
        assert_eq!(&bytes[140..], image.pixels());
    }

    #[test]
    fn eps_full_hex_line_has_one_trailing_newline() {
        let image = Image::new(39, 1, Mode::L, &[0; 39]).unwrap();
        let mut bytes = [0; 512];
        let mut out = Sink::write(&mut bytes);
        eps(&image, &mut out).unwrap();
        let len = out.finish().unwrap();
        let encoded = &bytes[..len];
        assert!(encoded.windows(11).any(|w| w == b"%ImageData:"));
        assert!(encoded.ends_with(b"000000\n%%%%EndBinary\ngrestore end\n"));
        assert!(!encoded.windows(15).any(|w| w == b"\n\n%%%%EndBinary"));
    }
}
