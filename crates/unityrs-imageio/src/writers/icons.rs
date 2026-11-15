// Ported from Pillow 9.5.0 (MIT-CMU): src/PIL/IcoImagePlugin.py's _save
// and src/PIL/IcnsImagePlugin.py's _save. See THIRD_PARTY_LICENSES.
// Copyright (c) 2004 by Bob Ippolito.
// Copyright (c) 2004 by Secret Labs.
// Copyright (c) 2004 by Fredrik Lundh.
// Copyright (c) 2014 by Alastair Houghton.
// Copyright (c) 2020 by Pan Jing.
//! PNG-backed default icon containers.
use super::resample::{self, Filter};
use super::{Exception, Format, Refusal, SaveError, Saved, Sink};
use crate::{Buffer, Charger, EncodeError, Image, Png};

pub(crate) fn encode<C: Charger>(
    image: &Image<'_>,
    format: Format,
    charger: &mut C,
) -> Result<Saved<C::Charge>, SaveError<C::Refusal>> {
    if format == Format::Ico {
        ico(image, charger)
    } else {
        icns(image, charger)
    }
}

struct Frame<C> {
    width: u32,
    height: u32,
    png: Png<C>,
}

fn ico<C: Charger>(
    image: &Image<'_>,
    charger: &mut C,
) -> Result<Saved<C::Charge>, SaveError<C::Refusal>> {
    let mut frames: [Option<Frame<C::Charge>>; 7] = std::array::from_fn(|_| None);
    let sizes = [16, 24, 32, 48, 64, 128, 256];
    for (&size, frame) in sizes.iter().zip(&mut frames) {
        if size <= image.width() && size <= image.height() {
            let (width, height) = thumbnail(image.width(), image.height(), size);
            *frame = Some(Frame {
                width,
                height,
                png: png(image, width, height, Filter::Lanczos, charger)?,
            });
        }
    }
    let count = frames.iter().flatten().count();
    let write = |out: &mut Sink| {
        out.put(b"\0\0\x01\0");
        out.le16(u16::try_from(count).map_err(|_| overflow())?);
        let mut offset = 6 + count * 16;
        for frame in frames.iter().flatten() {
            out.byte(u8::try_from(frame.width).unwrap_or_default());
            out.byte(u8::try_from(frame.height).unwrap_or_default());
            out.le16(0); // color count and reserved
            out.le16(0); // planes
            out.le16(32); // Pillow uses 32 even for an L/LA/RGB/P PNG.
            out.le32(length32(frame.png.bytes().len())?);
            out.le32(length32(offset)?);
            offset = offset
                .checked_add(frame.png.bytes().len())
                .ok_or_else(overflow)?;
        }
        for frame in frames.iter().flatten() {
            out.put(frame.png.bytes());
        }
        Ok(())
    };
    output(charger, write)
}

fn icns<C: Charger>(
    image: &Image<'_>,
    charger: &mut C,
) -> Result<Saved<C::Charge>, SaveError<C::Refusal>> {
    let mut frames: [Option<Png<C::Charge>>; 6] = std::array::from_fn(|_| None);
    for (frame, size) in frames.iter_mut().zip([128, 256, 512, 1024, 32, 64]) {
        *frame = Some(png(image, size, size, Filter::Bicubic, charger)?);
    }
    let entries = [
        (b"ic07", 0),
        (b"ic08", 1),
        (b"ic09", 2),
        (b"ic10", 3),
        (b"ic11", 4),
        (b"ic12", 5),
        (b"ic13", 1),
        (b"ic14", 2),
    ];
    let mut length = 8_usize + 8 + 8 * entries.len();
    for (_, index) in entries {
        let png = frames
            .get(index)
            .and_then(Option::as_ref)
            .ok_or_else(overflow)?;
        length = length
            .checked_add(8)
            .and_then(|n| n.checked_add(png.bytes().len()))
            .ok_or_else(overflow)?;
    }
    let write = |out: &mut Sink| {
        out.put(b"icns");
        out.be32(length32(length)?);
        out.put(b"TOC ");
        out.be32(72);
        for (kind, index) in entries {
            let png = frames
                .get(index)
                .and_then(Option::as_ref)
                .ok_or_else(overflow)?;
            out.put(kind);
            out.be32(length32(
                png.bytes().len().checked_add(8).ok_or_else(overflow)?,
            )?);
        }
        for (kind, index) in entries {
            let png = frames
                .get(index)
                .and_then(Option::as_ref)
                .ok_or_else(overflow)?;
            out.put(kind);
            out.be32(length32(
                png.bytes().len().checked_add(8).ok_or_else(overflow)?,
            )?);
            out.put(png.bytes());
        }
        Ok(())
    };
    output(charger, write)
}

fn png<C: Charger>(
    image: &Image<'_>,
    width: u32,
    height: u32,
    filter: Filter,
    charger: &mut C,
) -> Result<Png<C::Charge>, SaveError<C::Refusal>> {
    if image.width() == width && image.height() == height {
        return Ok(crate::encode(image, charger)?);
    }
    let resized = resample::resize(image, width, height, filter, charger)?;
    let view = match image.palette() {
        Some(palette) => Image::with_palette(width, height, &resized.bytes, palette),
        None => Image::new(width, height, image.mode(), &resized.bytes),
    }
    .map_err(EncodeError::from)?;
    Ok(crate::encode(&view, charger)?)
}

// floor/ceil intentionally become integers, within the supplied icon size.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn thumbnail(width: u32, height: u32, size: u32) -> (u32, u32) {
    // Image.thumbnail chooses floor or ceil by the resulting aspect error;
    // ties use floor. Its second branch gives zero an error of zero.
    let aspect = f64::from(width) / f64::from(height);
    if aspect <= 1.0 {
        let ideal = f64::from(size) * aspect;
        let floor = ideal.floor();
        let ceil = ideal.ceil();
        let best = if (aspect - floor / f64::from(size)).abs()
            <= (aspect - ceil / f64::from(size)).abs()
        {
            floor
        } else {
            ceil
        };
        ((best as u32).max(1), size)
    } else {
        let ideal = f64::from(size) / aspect;
        let floor = ideal.floor();
        let ceil = ideal.ceil();
        let error = |n: f64| {
            if n == 0.0 {
                0.0
            } else {
                (aspect - f64::from(size) / n).abs()
            }
        };
        let best = if error(floor) <= error(ceil) {
            floor
        } else {
            ceil
        };
        (size, (best as u32).max(1))
    }
}

fn length32(length: usize) -> Result<u32, Refusal> {
    u32::try_from(length).map_err(|_| overflow())
}

fn overflow() -> Refusal {
    Refusal::new(Exception::MemoryError, "icon exceeds address space")
}

fn output<C: Charger>(
    charger: &mut C,
    write: impl Fn(&mut Sink) -> Result<(), Refusal>,
) -> Result<Saved<C::Charge>, SaveError<C::Refusal>> {
    let mut counter = Sink::count();
    write(&mut counter)?;
    let length = counter.finish()?;
    let charge = charger
        .charge(Buffer::Output, &[length])
        .map_err(|refusal| EncodeError::Refused {
            buffer: Buffer::Output,
            refusal,
        })?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length)
        .map_err(|_| EncodeError::Alloc {
            buffer: Buffer::Output,
            request: length,
        })?;
    bytes.resize(length, 0);
    let mut writer = Sink::write(&mut bytes);
    write(&mut writer)?;
    writer.finish()?;
    Ok(Saved::new(bytes, charge))
}

#[cfg(test)]
mod tests {
    use super::encode;
    use crate::writers::Format;
    use crate::{Image, Mode, Unmetered};
    #[test]
    fn small_and_empty_images_write_an_empty_ico() {
        for (width, height) in [(0, 0), (3, 0), (0, 20), (15, 30), (30, 15)] {
            let pixels = vec![0; width as usize * height as usize];
            let image = Image::new(width, height, Mode::L, &pixels).unwrap();
            assert_eq!(
                encode(&image, Format::Ico, &mut Unmetered).unwrap().bytes(),
                b"\0\0\x01\0\0\0"
            );
        }
    }
    #[test]
    fn exact_size_ico_preserves_original_png() {
        for mode in Mode::ALL {
            let pixels: Vec<u8> = (0..16 * 16 * mode.channels())
                .map(|n| u8::try_from((n * 73 + n / 7) % 256).unwrap())
                .collect();
            let image = Image::new(16, 16, mode, &pixels).unwrap();
            let png = crate::encode(&image, &mut Unmetered).unwrap();
            let ico = encode(&image, Format::Ico, &mut Unmetered).unwrap();
            assert_eq!(
                ico.bytes().get(..14),
                Some(b"\0\0\x01\0\x01\0\x10\x10\0\0\0\0\x20\0".as_slice())
            );
            assert_eq!(ico.bytes().get(22..), Some(png.bytes()));
        }
    }
}
