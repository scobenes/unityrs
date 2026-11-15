// Ported from Crunch legacy decoder in texture2ddecoder 1.0.5 (LicenseRef-Crunch): legacy CRN unpacking. See THIRD_PARTY_LICENSES.
// src/Texture2DDecoder/crunch/crn_decomp.h:272-323, crn_decomp.h:2702-2771,
// crn_decomp.h:3690-3740. Altered source: checked, borrowed Rust header.
// Crunch Library Copyright (c) 2010-2016 Richard Geldreich, Jr.
// See LICENSE-CRUNCH and THIRD_PARTY_LICENSES.

use super::{CrunchError as Error, Result};

#[derive(Clone, Copy, Default, Debug)]
pub(super) struct Region {
    pub start: usize,
    pub len: usize,
    pub count: usize,
}

impl Region {
    pub fn bytes(self, data: &[u8]) -> Result<&[u8]> {
        data.get(self.start..self.start + self.len)
            .ok_or(Error::Header("range outside data"))
    }
}

#[derive(Debug)]
pub(super) struct Header {
    pub width: usize,
    pub height: usize,
    pub dxt5: bool,
    pub palettes: [Region; 4],
    pub tables: Region,
    pub levels: usize,
    offsets: [usize; 16],
}

fn uint(data: &[u8], at: usize, n: usize) -> Result<usize> {
    let bytes = data.get(at..at + n).ok_or(Error::Header("short header"))?;
    Ok(bytes.iter().fold(0, |v, &b| (v << 8) | usize::from(b)))
}

impl Header {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.get(..2) != Some(b"Hx") {
            return Err(Error::Header("signature"));
        }
        let size = uint(data, 2, 2)?;
        let end = uint(data, 6, 4)?;
        let width = uint(data, 12, 2)?;
        let height = uint(data, 14, 2)?;
        let levels = uint(data, 16, 1)?;
        let faces = u8::try_from(uint(data, 17, 1)?).map_err(|_| Error::Header("faces"))?;
        let format = uint(data, 18, 1)?;
        if size < 74 || size > end || end > data.len() {
            return Err(Error::Header("header/data size"));
        }
        if faces != 1 {
            return Err(Error::Faces(faces));
        }
        if levels == 0 || levels > 16 || size < 70 + 4 * levels {
            return Err(Error::Header("level count"));
        }
        if format != 0 && format != 2 {
            return Err(Error::Header("expected DXT1 or DXT5"));
        }
        if uint(data, 19, 2)? & 1 != 0 {
            return Err(Error::Segmented);
        }
        let mut palettes = [Region::default(); 4];
        for (i, region) in palettes.iter_mut().enumerate() {
            let at = 33 + i * 8;
            *region = Region {
                start: uint(data, at, 3)?,
                len: uint(data, at + 3, 3)?,
                count: uint(data, at + 6, 2)?,
            };
            if region.count != 0 {
                validate_region(*region, size, end)?;
            }
        }
        let tables = Region {
            start: uint(data, 67, 3)?,
            len: uint(data, 65, 2)?,
            count: 0,
        };
        validate_region(tables, size, end)?;
        // The unpack loops dereference these palettes unconditionally.
        if palettes
            .iter()
            .take(if format == 2 { 4 } else { 2 })
            .any(|p| p.count == 0)
        {
            return Err(Error::Header("missing palette"));
        }
        let mut offsets = [0; 16];
        let mut previous = size - 1;
        for (i, offset) in offsets.iter_mut().take(levels).enumerate() {
            *offset = uint(data, 70 + i * 4, 4)?;
            if *offset <= previous || *offset >= end {
                return Err(Error::Header("level offset/order"));
            }
            previous = *offset;
        }
        Ok(Self {
            width,
            height,
            dxt5: format == 2,
            palettes,
            tables,
            levels,
            offsets,
        })
    }

    pub fn level<'a>(&self, data: &'a [u8], level: usize) -> Result<(&'a [u8], usize, usize)> {
        if level >= self.levels {
            return Err(Error::Header("level index"));
        }
        let start = *self
            .offsets
            .get(level)
            .ok_or(Error::Header("level index"))?;
        let end = if level + 1 < self.levels {
            *self
                .offsets
                .get(level + 1)
                .ok_or(Error::Header("level index"))?
        } else {
            // init stores the supplied buffer size, not header.data_size;
            // unpack_level uses it for the last mip (texture2ddecoder 1.0.5
            // sdist crn_decomp.h:3672-3687, crn_decomp.h:3690-3703).
            data.len()
        };
        let bytes = data.get(start..end).ok_or(Error::Header("level range"))?;
        Ok((
            bytes,
            (self.width >> level).max(1),
            (self.height >> level).max(1),
        ))
    }
}

fn validate_region(region: Region, header: usize, end: usize) -> Result<()> {
    if region.start < header || region.len == 0 || region.start + region.len > end {
        return Err(Error::Header("palette/table range"));
    }
    Ok(())
}
