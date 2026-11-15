// Ported from Crunch legacy decoder in texture2ddecoder 1.0.5 (LicenseRef-Crunch): legacy CRN unpacking. See THIRD_PARTY_LICENSES.
// Ported from Crunch Unity decoder in texture2ddecoder 1.0.5 (Zlib): shared packed header and Unity format tags. See THIRD_PARTY_LICENSES.
// Unity fork: src/Texture2DDecoder/unitycrunch/crn_defs.h:244-291; crnlib.h:48-81.
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
    pub format: u8,
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
        Self::parse_codec(data, false)
    }

    pub fn parse_unity(data: &[u8]) -> Result<Self> {
        Self::parse_codec(data, true)
    }

    fn parse_codec(data: &[u8], unity: bool) -> Result<Self> {
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
        if !(matches!(format, 0 | 2) || unity && matches!(format, 10..=12)) {
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
        }
        // decode_palettes reads each endpoint/selector pair only when its
        // endpoint count is nonzero, regardless of the selector count:
        // crunch/crn_decomp.h:3824-3839; unitycrunch/crn_decomp.h:3131-3147.
        for [endpoint, selector] in palettes.as_chunks::<2>().0 {
            if endpoint.count != 0 {
                validate_region(*endpoint, size, end)?;
                validate_region(*selector, size, end)?;
            }
        }
        let tables = Region {
            start: uint(data, 67, 3)?,
            len: uint(data, 65, 2)?,
            count: 0,
        };
        // init_tables always reads the table stream before either palette
        // gate (crunch/crn_decomp.h:3796-3822; unitycrunch/crn_decomp.h:3102-3129).
        validate_region(tables, size, end)?;
        // The unpack loops dereference these palettes unconditionally.
        if palettes
            .iter()
            .take(if matches!(format, 2 | 12) { 4 } else { 2 })
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
            format: u8::try_from(format).map_err(|_| Error::Header("format"))?,
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

#[cfg(test)]
mod tests {
    use super::{Error, Header};

    fn sample() -> Vec<u8> {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/crunch_native.json")).unwrap();
        let hex = fixture["cases"][0]["data"]["hex"].as_str().unwrap();
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn descriptor_ranges_follow_endpoint_gates_in_both_forks() {
        for parse in [Header::parse, Header::parse_unity] {
            let data = sample();
            // With no colour endpoints, neither colour range is read. This
            // decoder safely refuses the later required palette fetch.
            for at in [33, 41] {
                let mut bad = data.clone();
                bad[39..41].fill(0);
                bad[at..at + 3].fill(255);
                assert!(matches!(parse(&bad), Err(Error::Header("missing palette"))));
            }
            // With no alpha endpoints, both ranges are ignored even with
            // a nonzero selector count and an out-of-buffer offset.
            for at in [49, 57] {
                let mut ignored = data.clone();
                ignored[at..at + 3].fill(255);
                ignored[63..65].copy_from_slice(&1u16.to_be_bytes());
                assert!(parse(&ignored).is_ok());
            }
            // Each live pair reads both descriptors, including a selector
            // stream whose count is zero. Tables are always read.
            for at in [33, 41, 49, 57, 67] {
                for selectors in [0u16, 1] {
                    let mut bad = data.clone();
                    bad[55..57].copy_from_slice(&1u16.to_be_bytes());
                    bad[47..49].copy_from_slice(&selectors.to_be_bytes());
                    bad[63..65].copy_from_slice(&selectors.to_be_bytes());
                    // Supply valid alpha ranges before corrupting one.
                    let region: [u8; 6] = bad[33..39].try_into().unwrap();
                    bad[49..55].copy_from_slice(&region);
                    bad[57..63].copy_from_slice(&region);
                    bad[at..at + 3].fill(255);
                    assert!(
                        matches!(parse(&bad), Err(Error::Header("palette/table range"))),
                        "offset {at}, selector count {selectors}"
                    );
                }
            }
        }
    }
}
