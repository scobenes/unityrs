// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/BundleFile.py, UnityPy/files/WebFile.py. See THIRD_PARTY_LICENSES.
//! Synthetic legacy bundles and WebFile containers, including stored compression streams.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "synthetic builders deliberately encode fixed-width fields"
)]
use crate::bundle::{BundleSpec, EntrySpec, TestCodec, build_with_signature, compress};
use crate::writer::ByteWriter;

/// UnityWeb/UnityRaw, formats 1–6 (`files/BundleFile.py:55-94`).
#[must_use]
pub fn legacy(signature: &[u8], format: u32, entries: &[(&str, &[u8])]) -> Vec<u8> {
    legacy_levels(signature, format, entries, 1)
}

/// The same legacy payload with multiple levels; only the last is used.
#[must_use]
pub fn legacy_levels(
    signature: &[u8],
    format: u32,
    entries: &[(&str, &[u8])],
    levels: usize,
) -> Vec<u8> {
    let levels = levels.clamp(1, 1024);
    if format == 6 {
        return build_with_signature(
            &BundleSpec {
                format,
                entries: entries
                    .iter()
                    .map(|(n, b)| EntrySpec {
                        name: n.as_bytes().to_vec(),
                        data: b.to_vec(),
                        flags: 0,
                    })
                    .collect(),
                ..BundleSpec::default()
            },
            signature,
        );
    }
    let table = 4 + entries.iter().map(|(n, _)| n.len() + 9).sum::<usize>();
    let mut body = ByteWriter::be();
    body.i32(entries.len() as i32);
    let mut offset = table;
    for (name, data) in entries {
        body.cstr(name.as_bytes())
            .u32(offset as u32)
            .u32(data.len() as u32);
        offset += data.len();
    }
    for (_, data) in entries {
        body.bytes(data);
    }
    let body = body.into_inner();
    let packed = if signature == b"UnityWeb" {
        let raw = compress(TestCodec::Lzma, &body);
        let mut packed = raw[..5].to_vec();
        packed.extend_from_slice(&(body.len() as u64).to_le_bytes());
        packed.extend_from_slice(&raw[5..]);
        packed
    } else {
        body.clone()
    };
    let mut w = ByteWriter::be();
    w.cstr(signature)
        .u32(format)
        .cstr(b"5.x.x")
        .cstr(b"2022.3.0f1");
    if format >= 4 {
        w.bytes(&[42; 16]).u32(1234);
    }
    let header = w.len()
        + 24
        + 8 * (levels - 1)
        + usize::from(format >= 2) * 4
        + usize::from(format >= 3) * 4;
    w.u32(0).u32(header as u32).u32(1).i32(levels as i32);
    for _ in 1..levels {
        w.u32(u32::MAX).u32(u32::MAX);
    }
    w.u32(packed.len() as u32).u32(body.len() as u32);
    if format >= 2 {
        w.u32((header + packed.len()) as u32);
    }
    if format >= 3 {
        w.u32(table as u32);
    }
    w.bytes(&packed);
    w.into_inner()
}

/// Plain WebFile (`files/WebFile.py:73-105`).
#[must_use]
pub fn plain(signature: &str, entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = ByteWriter::le();
    let mut offset = signature.len() + 5 + entries.iter().map(|(n, _)| 12 + n.len()).sum::<usize>();
    w.cstr(signature.as_bytes()).i32(offset as i32);
    for (name, data) in entries {
        w.i32(offset as i32)
            .i32(data.len() as i32)
            .i32(name.len() as i32)
            .bytes(name.as_bytes());
        offset += data.len();
    }
    for (_, data) in entries {
        w.bytes(data);
    }
    w.into_inner()
}

/// A plain WebFile whose first member's name length is -2 (Tier U).
/// Nonempty `entries` retain their payloads, but opening the directory is unsupported.
#[must_use]
pub fn unsupported_name_length(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut data = crate::env::web_file(entries);
    let name_length = b"UnityWebData1.0\0".len() + 12;
    if let Some(length) = data.get_mut(name_length..name_length + 4) {
        length.copy_from_slice(&(-2_i32).to_le_bytes());
    }
    data
}

/// Gzip with a stored DEFLATE stream, deterministic header and CRC.
#[must_use]
pub fn gzip(data: &[u8]) -> Vec<u8> {
    let mut out = vec![31, 139, 8, 0, 0, 0, 0, 0, 0, 3];
    for (i, chunk) in data.chunks(65535).enumerate() {
        out.push(u8::from((i + 1) * 65535 >= data.len()));
        let n = chunk.len() as u16;
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&(!n).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    if data.is_empty() {
        out.extend_from_slice(&[1, 0, 0, 255, 255]);
    }
    let mut crc = !0u32;
    for b in data {
        crc ^= u32::from(*b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    out.extend_from_slice(&(!crc).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

/// Brotli with a metadata block placing Unity's detection marker at 0x20,
/// then uncompressed meta-blocks (RFC 7932 sections 9.1 and 9.2).
#[must_use]
pub fn brotli(data: &[u8]) -> Vec<u8> {
    // WBITS=16 (0); ISLAST=0; MNIBBLES=3; reserved=0; MSKIPBYTES=1;
    // MSKIPLEN=35 (36 bytes). Header occupies two bytes.
    let mut out = vec![0xac, 0x11];
    out.resize(38, 0);
    out[32..38].copy_from_slice(b"brotli");
    for chunk in data.chunks(65536) {
        let bits = (((chunk.len() - 1) as u32) << 3) | (1 << 19);
        out.extend_from_slice(&bits.to_le_bytes()[..3]);
        out.extend_from_slice(chunk);
    }
    out.push(3);
    out
}

/// A Huffman-coded WebFile containing `raw` = `ab12` repeated 3000 times.
/// Synthetic stream encoded by brotli 1.2.0, quality 11, lgwin 16, with a
/// metadata block for detection. Exercises tables and the static dictionary.
#[must_use]
pub fn huffman() -> Vec<u8> {
    let mut out = vec![0xac, 0x27];
    out.resize(82, 0);
    out[32..38].copy_from_slice(b"brotli");
    out.extend_from_slice(&[
        33, 240, 130, 223, 226, 192, 110, 242, 60, 50, 201, 102, 109, 84, 95, 156, 158, 53, 192,
        128, 28, 248, 116, 25, 145, 41, 152, 178, 20, 139, 185, 235, 152, 171, 176, 60, 176, 1, 7,
        26, 121, 0, 22, 225, 38, 114, 1, 224, 80, 45, 109, 107, 199, 33, 56, 242, 150, 38, 0, 235,
        48,
    ]);
    out
}
