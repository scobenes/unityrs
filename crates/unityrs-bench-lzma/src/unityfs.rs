//! Just enough `UnityFS` to find a bundle's LZMA data blocks.
//!
//! Follows `UnityPy` 1.25.2 `BundleFile.read_fs`, with its two rules that hang
//! on the engine version string:
//! - the archive flags are `ArchiveFlagsOld` before 2020.3.34, 2021.3.2 and
//!   2022.1.1, and `ArchiveFlags` from then on. Bit 0x200 is encryption in the
//!   old set and padding-at-start in the new one, where encryption is 0x1400;
//! - the header is aligned to 16 for format 7 and later, and also for format 6
//!   from engine 2019.4.15 on, within 2019.
//!
//! Blocks-info may sit at the front or at the end, compressed with LZ4, LZ4HC
//! or nothing. Whatever this finder cannot follow is an error, never a guess:
//! a format other than 6-8; an engine version `UnityPy` would replace with its
//! fallback, or one whose packed form it might read differently from the
//! digits ([`Engine::parse`]); a header string `UnityPy` would cut short; an
//! encrypted bundle; and any other blocks-info compression. The real reader
//! belongs in `unityrs-core`.

/// Blocks-info and data-block compression type, as in `CompressionFlags`.
const COMPRESSION_MASK: u32 = 0x3F;
const LZMA: u32 = 1;
const LZ4: u32 = 2;
const LZ4HC: u32 = 3;
const BLOCKS_INFO_AT_END: u32 = 0x80;
/// `ArchiveFlags` (the new set): padding-at-start, and the encryption bits.
const PADDING_AT_START: u32 = 0x200;
const ENCRYPTION_NEW: u32 = 0x1400;
/// `ArchiveFlagsOld`: encryption, on the bit the new set uses for padding.
const ENCRYPTION_OLD: u32 = 0x200;

/// One LZMA data block: the byte range of its stream (5 property bytes, then
/// raw LZMA1) and the size the blocks-info promises it decompresses to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LzmaBlock {
    pub index: usize,
    pub start: usize,
    pub end: usize,
    pub uncompressed: usize,
}

/// Lists the LZMA data blocks of a `UnityFS` bundle. Blocks of any other
/// compression are skipped, since only LZMA is measured.
pub fn lzma_blocks(file: &[u8]) -> Result<Vec<LzmaBlock>, String> {
    let mut r = Reader { buf: file, pos: 0 };
    let signature = r.cstr()?;
    if signature != b"UnityFS" {
        return Err(format!(
            "not UnityFS: {}",
            String::from_utf8_lossy(signature)
        ));
    }
    let format = r.u32()?;
    if !(6..=8).contains(&format) {
        return Err(format!("UnityFS format {format} not supported here"));
    }
    r.cstr()?; // player version
    let engine = Engine::parse(r.cstr()?)?;
    r.take(8)?; // total size
    let info_compressed = r.usize()?;
    let info_uncompressed = r.usize()?;
    let flags = r.u32()?;
    let new_flags = engine.uses_new_flags();
    let encryption = if new_flags {
        ENCRYPTION_NEW
    } else {
        ENCRYPTION_OLD
    };
    if flags & encryption != 0 {
        return Err(format!(
            "encrypted bundle (flags {flags:#x}) not supported here"
        ));
    }
    if format >= 7 || engine.aligns_format_6() {
        r.align(16);
    }

    let info_bytes = if flags & BLOCKS_INFO_AT_END == 0 {
        r.take(info_compressed)?
    } else {
        let start = file
            .len()
            .checked_sub(info_compressed)
            .ok_or("blocks-info runs past the start")?;
        &file[start..]
    };
    let info = match flags & COMPRESSION_MASK {
        0 => info_bytes.to_vec(),
        LZ4 | LZ4HC => lz4_flex::block::decompress(info_bytes, info_uncompressed)
            .map_err(|e| format!("blocks-info LZ4: {e}"))?,
        other => {
            return Err(format!(
                "blocks-info compression {other} not supported here"
            ));
        }
    };
    if new_flags && flags & PADDING_AT_START != 0 {
        r.align(16);
    }

    let mut info = Reader { buf: &info, pos: 0 };
    info.take(16)?; // uncompressed data hash
    let count = info.usize()?;
    let mut offset = r.pos;
    let mut blocks = Vec::new();
    for index in 0..count {
        let uncompressed = info.usize()?;
        let compressed = info.usize()?;
        let block_flags = u32::from(info.u16()?);
        let end = offset + compressed;
        if end > file.len() {
            return Err(format!("block {index} runs past the end of the file"));
        }
        if block_flags & COMPRESSION_MASK == LZMA {
            blocks.push(LzmaBlock {
                index,
                start: offset,
                end,
                uncompressed,
            });
        }
        offset = end;
    }
    Ok(blocks)
}

/// The leading `major.minor.build` of an engine version, which is all that
/// `UnityPy`'s two rules compare (a `UnityVersion` against 3-tuples).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Engine(u32, u32, u32);

impl Engine {
    /// `UnityPy`'s `UnityVersion.from_str` matches
    /// `^(\d+)\.(\d+)\.(\d+)(.+?)?(\d+)?(.*)$`: the type is the one character
    /// after the build digits, and the type number is the digits after that.
    /// It packs the fields as `major << 48 | minor << 32 | build << 16 |
    /// type << 8 | type number` and reads each back masked, so a field too
    /// wide for its slot spills into the next one. Python's `\d` also matches
    /// every Unicode digit. This parse reads only what it can read exactly as
    /// `UnityPy` does, and returns an error for the rest:
    /// - anything but ASCII digits in major, minor or build, or any of them
    ///   above 0xFFFF;
    /// - a type number above 0xFF;
    /// - a non-ASCII character where `UnityPy` might take it for a digit:
    ///   right after the build digits or the type number;
    /// - major 0, or a string the pattern does not match, which send `UnityPy`
    ///   to its configured fallback version (none by default).
    fn parse(text: &[u8]) -> Result<Engine, String> {
        let fail = |why: &str| {
            let text = String::from_utf8_lossy(text);
            Err(format!("engine version {text:?}: {why}"))
        };
        let digits = |from: usize| {
            let rest = text.get(from..).unwrap_or_default();
            rest.iter().take_while(|b| b.is_ascii_digit()).count()
        };
        let mut pos = 0;
        let mut fields = [0u32; 3];
        for (i, field) in fields.iter_mut().enumerate() {
            let len = digits(pos);
            if len == 0 || (i < 2 && text.get(pos + len) != Some(&b'.')) {
                return fail("not major.minor.build in ASCII digits");
            }
            match bounded(&text[pos..pos + len], 0xFFFF) {
                Some(n) => *field = n,
                None => return fail("a field above 0xFFFF"),
            }
            pos += len + usize::from(i < 2);
        }
        if let Some(&kind) = text.get(pos) {
            if !kind.is_ascii() {
                return fail("a non-ASCII character after the build");
            }
            let start = pos + 1;
            let len = digits(start);
            if len > 0 && bounded(&text[start..start + len], 0xFF).is_none() {
                return fail("a type number above 0xFF");
            }
            if text.get(start + len).is_some_and(|b| !b.is_ascii()) {
                return fail("a non-ASCII character after the type");
            }
        }
        if fields[0] == 0 {
            return fail("major 0");
        }
        Ok(Engine(fields[0], fields[1], fields[2]))
    }

    /// `ArchiveFlags` rather than `ArchiveFlagsOld` (`BundleFile.read_fs`).
    fn uses_new_flags(self) -> bool {
        let old = self.0 < 2020
            || (self.0 == 2020 && self < Engine(2020, 3, 34))
            || (self.0 == 2021 && self < Engine(2021, 3, 2))
            || (self.0 == 2022 && self < Engine(2022, 1, 1));
        !old
    }

    /// `UnityPy`: "for 2019.4.15 and later, version should be 7 and aligned
    /// but some games in these versions somehow has version 6 while aligned".
    fn aligns_format_6(self) -> bool {
        self.0 == 2019 && self >= Engine(2019, 4, 15)
    }
}

/// ASCII digits as a number, or `None` above `max`.
fn bounded(digits: &[u8], max: u32) -> Option<u32> {
    digits.iter().try_fold(0u32, |n, &d| {
        n.checked_mul(10)?
            .checked_add(u32::from(d - b'0'))
            .filter(|&n| n <= max)
    })
}

/// `UnityPy` reads a header string whole only below this many bytes, and cuts
/// a longer one short (`read_string_to_null`, `max_length`).
const STRING_MAX: usize = 32767;

/// A big-endian cursor that reports truncation instead of panicking.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).filter(|&end| end <= self.buf.len());
        let end = end.ok_or_else(|| format!("truncated at byte {}", self.pos))?;
        let bytes = &self.buf[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn usize(&mut self) -> Result<usize, String> {
        usize::try_from(self.u32()?).map_err(|e| e.to_string())
    }

    fn cstr(&mut self) -> Result<&'a [u8], String> {
        let rest = &self.buf[self.pos..];
        let len = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or("unterminated string")?;
        if len >= STRING_MAX {
            return Err(format!(
                "a {len}-byte header string, which UnityPy cuts at {STRING_MAX}"
            ));
        }
        self.pos += len + 1;
        Ok(&rest[..len])
    }

    /// `UnityPy` aligns the absolute file position, and so does this.
    fn align(&mut self, to: usize) {
        self.pos = self.pos.next_multiple_of(to);
    }
}

#[cfg(test)]
mod tests {
    use super::{LzmaBlock, lzma_blocks};
    use crate::synth::{Header, bundle};

    fn header(format: u32, engine: &str, flags: u32, align: bool, pad: bool) -> Header<'_> {
        Header {
            format,
            engine,
            flags,
            align,
            pad,
        }
    }

    /// Three blocks, LZMA, LZ4HC and LZMA: the finder must report blocks 0
    /// and 2 where the bundle put them. `align` and `pad` are `UnityPy`'s
    /// reading of the header, written out by hand for each case.
    fn finds_both_lzma_blocks(format: u32, engine: &str, flags: u32, align: bool, pad: bool) {
        let (a, b, c) = (vec![0x5D; 37], vec![0xAB; 11], vec![0x5D; 23]);
        let blocks: [(&[u8], usize, u16); 3] = [(&a, 400, 0x41), (&b, 50, 0x43), (&c, 300, 0x41)];
        let (file, starts) = bundle(&header(format, engine, flags, align, pad), &blocks);
        let case = format!("format {format}, {engine}, flags {flags:#x}");
        let found = lzma_blocks(&file).unwrap_or_else(|e| panic!("{case}: {e}"));
        let want = vec![
            LzmaBlock {
                index: 0,
                start: starts[0],
                end: starts[0] + a.len(),
                uncompressed: 400,
            },
            LzmaBlock {
                index: 2,
                start: starts[2],
                end: starts[2] + c.len(),
                uncompressed: 300,
            },
        ];
        assert_eq!(found, want, "{case}");
    }

    fn rejection(format: u32, engine: &str, flags: u32, align: bool) -> String {
        let blocks: [(&[u8], usize, u16); 1] = [(&[0x5D; 9], 100, 0x41)];
        let (file, _) = bundle(&header(format, engine, flags, align, false), &blocks);
        match lzma_blocks(&file) {
            Ok(found) => panic!("format {format}, {engine}, flags {flags:#x}: Ok {found:?}"),
            Err(e) => e,
        }
    }

    #[test]
    fn blocks_info_at_the_front_raw_lz4_and_lz4hc() {
        finds_both_lzma_blocks(8, "6000.3.17f1", 0x240, true, true);
        finds_both_lzma_blocks(8, "6000.3.17f1", 0x242, true, true);
        finds_both_lzma_blocks(8, "6000.3.17f1", 0x243, true, true);
        finds_both_lzma_blocks(8, "6000.3.17f1", 0x43, true, false);
    }

    #[test]
    fn blocks_info_at_the_end_raw_lz4_and_lz4hc() {
        finds_both_lzma_blocks(8, "6000.3.17f1", 0x2C0, true, true);
        finds_both_lzma_blocks(8, "6000.3.17f1", 0xC2, true, false);
        finds_both_lzma_blocks(7, "2020.3.48f1", 0x2C3, true, true);
        finds_both_lzma_blocks(6, "2018.4.11f1", 0xC3, false, false);
    }

    #[test]
    fn format_6_is_aligned_from_2019_4_15() {
        for flags in [0x40, 0x43, 0x80, 0xC2] {
            finds_both_lzma_blocks(6, "2019.4.20f1", flags, true, false);
            finds_both_lzma_blocks(6, "2019.4.15f1", flags, true, false);
            finds_both_lzma_blocks(6, "2019.4.14f1", flags, false, false);
            finds_both_lzma_blocks(6, "2020.1.0f1", flags, false, false);
            finds_both_lzma_blocks(6, "2018.4.11f1", flags, false, false);
        }
    }

    /// 0x200 pads under the new flag set and encrypts under the old one.
    #[test]
    fn encryption_is_rejected_under_either_flag_set() {
        for flags in [0x1043, 0x443, 0x1643] {
            let e = rejection(8, "6000.3.17f1", flags, true);
            assert!(e.contains("encrypted"), "{flags:#x}: {e}");
        }
        for (format, engine) in [
            (6, "2019.4.20f1"),
            (6, "2018.4.11f1"),
            (7, "2020.3.33f1"),
            (7, "2021.3.1f1"),
            (7, "2022.1.0f1"),
        ] {
            let aligned = format >= 7 || engine.starts_with("2019");
            let e = rejection(format, engine, 0x243, aligned);
            assert!(e.contains("encrypted"), "{engine}: {e}");
        }
        for engine in ["2020.3.34f1", "2021.3.2f1", "2022.1.1f1", "2023.1.0f1"] {
            finds_both_lzma_blocks(7, engine, 0x243, true, true);
        }
    }

    #[test]
    fn other_formats_engine_versions_and_compressions_are_rejected() {
        for format in [5, 9] {
            let e = rejection(format, "6000.3.17f1", 0x43, true);
            assert!(e.contains("format"), "{format}: {e}");
        }
        for engine in ["0.0.0", "", "5.x.x", "2019.4", "a2019.4.20f1", "2019.4.f1"] {
            let e = rejection(8, engine, 0x43, true);
            assert!(e.contains("engine version"), "{engine:?}: {e}");
        }
        let (mut file, _) = bundle(&header(8, "6000.3.17f1", 0x40, true, false), &[]);
        // The flags' low byte, after a 46-byte header: LZMA blocks-info.
        assert_eq!(file[46..50], 0x40u32.to_be_bytes());
        file[49] = 0x41;
        let e = lzma_blocks(&file).unwrap_err();
        assert!(e.contains("compression 1"), "{e}");
    }

    #[test]
    fn engine_versions_read_as_unitypy_reads_them() {
        for (text, aligned, new_flags) in [
            ("2019.4.15f1", true, false),
            ("2019.4.15a1", true, false),
            ("2019.4.14f1", false, false),
            ("2019.10.0f1", true, false),
            ("2020.3.34f1", false, true),
            ("2020.3.33f1-custom", false, false),
            ("5.6.7", false, false),
            ("2017.4.40c1", false, false),
            ("6000.3.17f1", false, true),
            ("2019.4.15f255", true, false),
            ("2019.4.15.3", true, false),
            ("2019.4.15f1-é", true, false),
            ("65535.65535.65535x255", false, true),
        ] {
            let engine = super::Engine::parse(text.as_bytes()).unwrap();
            assert_eq!(engine.aligns_format_6(), aligned, "{text}");
            assert_eq!(engine.uses_new_flags(), new_flags, "{text}");
        }
    }

    /// Strings `UnityVersion` reads differently from their face value: a field
    /// that spills into the next once packed, a fullwidth digit that Python's
    /// `\d` matches. `UnityPy` reads the first two as 2019.4.15, falls back on
    /// the third (major 0 once masked), and reads the fourth as 2019.4.15. The
    /// finder must not guess at any of them.
    #[test]
    fn out_of_range_and_non_ascii_engine_versions_are_rejected() {
        for engine in [
            "2019.4.14f65536",
            "67555.4.15f1",
            "65536.0.0",
            "2019.4.1\u{FF15}f1",
            "2019.4.15f256",
            "2019.65536.0f1",
            "2019.4.65536f1",
            "2019.4.15\u{E9}1",
            "2019.4.15f1\u{FF15}",
        ] {
            assert!(
                super::Engine::parse(engine.as_bytes()).is_err(),
                "{engine:?}"
            );
            for flags in [0x40, 0x43] {
                let e = rejection(6, engine, flags, true);
                assert!(e.contains("engine version"), "{engine:?}: {e}");
            }
        }
    }

    /// `UnityPy` reads a header string whole only below 32,767 bytes.
    #[test]
    fn header_strings_of_32767_bytes_or_more_are_rejected() {
        let engine = |len: usize| format!("6000.3.17f1-{}", "x".repeat(len - 12));
        finds_both_lzma_blocks(8, &engine(32_766), 0x243, true, true);
        let e = rejection(8, &engine(32_767), 0x243, true);
        assert!(e.contains("32767-byte header string"), "{e}");
    }
}
