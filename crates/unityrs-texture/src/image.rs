// Ported from UnityPy 1.25.2 (MIT): export/Texture2DConverter.py's pad_image and crop-before-flip ordering. See THIRD_PARTY_LICENSES.
//! Edge extension (`export/Texture2DConverter.py:53-80`) without temporary
//! strips; Switch crop ordering is `export/Texture2DConverter.py:254-320`.
use crate::{DecodeError, Image, MAX_PIXELS, Mode, PlatformError};
use std::sync::Arc;
use unityrs_core::limits::{Meter, MeteredVec};

fn size(mode: Mode, w: u32, h: u32) -> Result<usize, DecodeError> {
    let pixels = u64::from(w) * u64::from(h);
    if pixels > MAX_PIXELS {
        return Err(DecodeError::TooLarge {
            pixels,
            max: MAX_PIXELS,
        });
    }
    Ok(
        usize::try_from(pixels * mode.bytes_per_pixel())
            .map_err(|_| PlatformError::SizeOverflow)?,
    )
}

/// Copy already ordered pixels into a charged image, validating its length.
pub fn image_from_pixels(
    mode: Mode,
    width: u32,
    height: u32,
    data: &[u8],
    meter: &Arc<Meter>,
) -> Result<Image, DecodeError> {
    let n = size(mode, width, height)?;
    if data.len() != n {
        return Err(DecodeError::NotEnoughData {
            needed: n as u64,
            got: data.len() as u64,
        });
    }
    meter.count_decode(n as u64)?;
    let mut pixels = MeteredVec::with_capacity(meter, n)?;
    pixels.extend_from_slice(data)?;
    let (pixels, charge) = pixels.into_parts();
    Ok(Image::from_parts(mode, width, height, pixels, charge))
}

/// Extend the final column, row and corner with NEAREST sampling. Equal
/// dimensions return the original allocation and charge, without work.
pub fn pad_image(
    image: Image,
    width: u32,
    height: u32,
    meter: &Arc<Meter>,
) -> Result<Image, DecodeError> {
    if (width, height) == (image.width(), image.height()) {
        return Ok(image);
    }
    if width < image.width() || height < image.height() {
        return Err(PlatformError::PaddingSize.into());
    }
    let n = size(image.mode(), width, height)?;
    if image.width() == 0 || image.height() == 0 {
        // Pillow's out-of-bounds single-edge crop supplies black pixels.
        // Changing both dimensions reaches its undefined corner pixel.
        // UnityPy export/Texture2DConverter.py:53-80 also resizes each strip:
        // with a zero side Pillow accepts its unchanged size (a one-pixel
        // extension), but refuses a resize to another zero-extent size.
        if (width != image.width() && height != image.height())
            || (image.height() == 0 && width - image.width() > 1)
            || (image.width() == 0 && height - image.height() > 1)
        {
            return Err(PlatformError::PaddingSize.into());
        }
        meter.count_decode(n as u64)?;
        let mut out = MeteredVec::with_capacity(meter, n)?;
        for _ in 0..n {
            out.push(0)?;
        }
        let (pixels, charge) = out.into_parts();
        return Ok(Image::from_parts(
            image.mode(),
            width,
            height,
            pixels,
            charge,
        ));
    }
    meter.count_decode(n as u64)?;
    let mut out = MeteredVec::with_capacity(meter, n)?;
    let channels =
        usize::try_from(image.mode().bytes_per_pixel()).map_err(|_| PlatformError::SizeOverflow)?;
    for y in 0..height {
        for x in 0..width {
            let sx = x.min(image.width() - 1);
            let sy = y.min(image.height() - 1);
            let at = (u64::from(sy) * u64::from(image.width()) + u64::from(sx)) * channels as u64;
            let at = usize::try_from(at).map_err(|_| PlatformError::SizeOverflow)?;
            out.extend_from_slice(
                image
                    .pixels()
                    .get(at..at + channels)
                    .ok_or(PlatformError::SizeOverflow)?,
            )?;
        }
    }
    let (pixels, charge) = out.into_parts();
    Ok(Image::from_parts(
        image.mode(),
        width,
        height,
        pixels,
        charge,
    ))
}

pub(crate) fn crop_flipped(
    image: &Image,
    width: u32,
    height: u32,
    meter: &Arc<Meter>,
) -> Result<Image, DecodeError> {
    let n = size(image.mode(), width, height)?;
    meter.count_decode(n as u64)?;
    let mut out = MeteredVec::with_capacity(meter, n)?;
    let row = width as usize
        * usize::try_from(image.mode().bytes_per_pixel())
            .map_err(|_| PlatformError::SizeOverflow)?;
    let stride = image.width() as usize
        * usize::try_from(image.mode().bytes_per_pixel())
            .map_err(|_| PlatformError::SizeOverflow)?;
    let start = image
        .height()
        .checked_sub(height)
        .ok_or(PlatformError::SizeOverflow)? as usize;
    for y in start..start + height as usize {
        let at = y * stride;
        out.extend_from_slice(
            image
                .pixels()
                .get(at..at + row)
                .ok_or(PlatformError::SizeOverflow)?,
        )?;
    }
    let (pixels, charge) = out.into_parts();
    Ok(Image::from_parts(
        image.mode(),
        width,
        height,
        pixels,
        charge,
    ))
}
