//! The fixture tests' pixels: a PNG's own pixels, decoded apart from the
//! encoder, and E20's visible-pixel hash computed from them.

use std::fmt::Write as _;
use std::io::Read as _;

use sha2::{Digest, Sha256};

/// Bytes as lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        write!(s, "{b:02x}").unwrap();
        s
    })
}

/// A PNG's pixels: its IDAT stream inflated and unfiltered.
pub fn png_pixels(png: &[u8], width: usize, height: usize, channels: usize) -> Vec<u8> {
    let mut at = 8;
    let mut idat = Vec::new();
    while at + 8 <= png.len() {
        let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
        let kind = &png[at + 4..at + 8];
        if kind == b"IDAT" {
            idat.extend_from_slice(&png[at + 8..at + 8 + len]);
        }
        at += 12 + len;
    }
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(idat.as_slice())
        .read_to_end(&mut raw)
        .unwrap();
    let stride = width * channels;
    let mut out = vec![0u8; stride * height];
    for y in 0..height {
        let filter = raw[y * (stride + 1)];
        let line = &raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        for x in 0..stride {
            let a = if x >= channels {
                out[y * stride + x - channels]
            } else {
                0
            };
            let b = if y > 0 { out[(y - 1) * stride + x] } else { 0 };
            let c = if x >= channels && y > 0 {
                out[(y - 1) * stride + x - channels]
            } else {
                0
            };
            let pred = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => a.midpoint(b),
                4 => {
                    let p = i16::from(a) + i16::from(b) - i16::from(c);
                    let (pa, pb, pc) = (
                        (p - i16::from(a)).abs(),
                        (p - i16::from(b)).abs(),
                        (p - i16::from(c)).abs(),
                    );
                    if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    }
                }
                f => panic!("filter {f}"),
            };
            out[y * stride + x] = line[x].wrapping_add(pred);
        }
    }
    out
}

/// E20's visible-pixel hash, computed here from the PNG's own pixels: the
/// SHA-256 of `"{width}x{height}:"` and the RGBA bytes, an RGB pixel's alpha
/// 255 and a pixel of alpha 0 written as zeros.
pub fn visible_hash(width: u32, height: u32, channels: usize, pixels: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(format!("{width}x{height}:").as_bytes());
    for px in pixels.chunks(channels) {
        let rgba = [px[0], px[1], px[2], px.get(3).copied().unwrap_or(255)];
        h.update(if rgba[3] == 0 { [0; 4] } else { rgba });
    }
    h.finalize().into()
}

/// 32 hex digits as an MD5's 16 bytes.
pub fn unhex(s: &str) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
    }
    out
}
