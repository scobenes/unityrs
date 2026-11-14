//! Synthetic single-disk ZIPs. Deflate uses a final uncompressed block,
//! so fixture creation needs no compressor dependency.

#![allow(
    clippy::cast_possible_truncation,
    reason = "test-only builder for small synthetic inputs"
)]

/// CRC-32/ISO-HDLC of a member.
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

/// Entries `(name, bytes, deflate)` in central-directory order. Test inputs
/// are below 64 KiB per member. Version-needed zero matches UnityPy's ZIP
/// byte detector; callers can set byte four to 20 to test constructor dispatch.
#[must_use]
pub fn archive(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
    let mut data = Vec::new();
    let mut central = Vec::new();
    for &(name, bytes, deflate) in entries {
        let offset = data.len() as u32;
        let crc = crc32(bytes);
        let compressed = if deflate {
            let n = bytes.len() as u16;
            let mut out = vec![1];
            out.extend(n.to_le_bytes());
            out.extend((!n).to_le_bytes());
            out.extend(bytes);
            out
        } else {
            bytes.to_vec()
        };
        let method: u16 = if deflate { 8 } else { 0 };
        data.extend(b"PK\x03\x04");
        data.extend(0_u16.to_le_bytes());
        data.extend(0x800_u16.to_le_bytes());
        data.extend(method.to_le_bytes());
        data.extend([0; 4]);
        data.extend(crc.to_le_bytes());
        data.extend((compressed.len() as u32).to_le_bytes());
        data.extend((bytes.len() as u32).to_le_bytes());
        data.extend((name.len() as u16).to_le_bytes());
        data.extend([0; 2]);
        data.extend(name.as_bytes());
        data.extend(&compressed);
        central.extend(b"PK\x01\x02");
        central.extend([0; 4]);
        central.extend(0x800_u16.to_le_bytes());
        central.extend(method.to_le_bytes());
        central.extend([0; 4]);
        central.extend(crc.to_le_bytes());
        central.extend((compressed.len() as u32).to_le_bytes());
        central.extend((bytes.len() as u32).to_le_bytes());
        central.extend((name.len() as u16).to_le_bytes());
        central.extend([0; 12]);
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let start = data.len() as u32;
    let size = central.len() as u32;
    data.extend(central);
    data.extend(b"PK\x05\x06");
    data.extend([0; 4]);
    data.extend((entries.len() as u16).to_le_bytes());
    data.extend((entries.len() as u16).to_le_bytes());
    data.extend(size.to_le_bytes());
    data.extend(start.to_le_bytes());
    data.extend([0; 2]);
    data
}
