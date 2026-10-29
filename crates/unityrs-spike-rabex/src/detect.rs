//! L0 detection: a port of UnityPy 1.25.2's `ImportHelper.check_file_type`.
//!
//! rabex has no detection of its own (a caller must already know whether it
//! holds a bundle or a SerializedFile), so this is layer we would add on top.
//! The rules, quirks included, are docs/CANONICAL_DUMP.md section 4.2.

use std::io::Read;

/// `FileType` names as UnityPy prints them, plus `error` when it raises.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileType {
    AssetsFile,
    BundleFile,
    WebFile,
    ResourceFile,
    Zip,
    Error,
}

impl FileType {
    pub fn name(self) -> &'static str {
        match self {
            FileType::AssetsFile => "AssetsFile",
            FileType::BundleFile => "BundleFile",
            FileType::WebFile => "WebFile",
            FileType::ResourceFile => "ResourceFile",
            FileType::Zip => "ZIP",
            FileType::Error => "error",
        }
    }
}

/// UnityPy's `read_string_to_null(20)` at offset 0, or `None` where it raises.
fn signature(data: &[u8]) -> Option<&[u8]> {
    let window = &data[..data.len().min(20)];
    match window.iter().position(|&b| b == 0) {
        Some(nul) => Some(&data[..nul]),
        None if data.len() <= 20 => None,
        None => Some(window),
    }
}

/// Eight U+00FA characters: UnityPy compares the decoded signature, so only
/// their UTF-8 bytes match, never the raw `FA` bytes (spec section 7).
const FA8: &[u8] = "\u{fa}\u{fa}\u{fa}\u{fa}\u{fa}\u{fa}\u{fa}\u{fa}".as_bytes();

fn is_webdata(sig: &[u8]) -> bool {
    sig.starts_with(b"UnityWebData") || sig.starts_with(b"TuanjieWebData")
}

pub fn check_file_type(data: &[u8]) -> FileType {
    if data.len() < 20 {
        return FileType::ResourceFile;
    }
    let Some(sig) = signature(data) else {
        return FileType::Error;
    };
    if matches!(sig, b"UnityWeb" | b"UnityRaw" | b"UnityFS") || sig == FA8 {
        return FileType::BundleFile;
    }
    if is_webdata(sig) {
        return FileType::WebFile;
    }
    if sig == b"PK\x03\x04" {
        return FileType::Zip;
    }
    if data.len() < 128 {
        return FileType::ResourceFile;
    }
    if data.starts_with(&[0x1f, 0x8b]) {
        let mut head = Vec::with_capacity(20);
        let mut gz = flate2::read::GzDecoder::new(data).take(20);
        if gz.read_to_end(&mut head).is_err() {
            return FileType::Error;
        }
        let end = head.iter().position(|&b| b == 0).unwrap_or(head.len());
        if is_webdata(&head[..end]) {
            return FileType::WebFile;
        }
    }
    if &data[0x20..0x26] == b"brotli" {
        return FileType::WebFile;
    }
    if assets_header_plausible(data) {
        FileType::AssetsFile
    } else {
        FileType::ResourceFile
    }
}

/// The SerializedFile header check, read big-endian whatever the file says.
fn assets_header_plausible(data: &[u8]) -> bool {
    let u32_at = |o: usize| i64::from(u32::from_be_bytes(data[o..o + 4].try_into().unwrap()));
    let i64_at = |o: usize| i64::from_be_bytes(data[o..o + 8].try_into().unwrap());
    let (mut metadata_size, mut file_size, version, mut data_offset) =
        (u32_at(0), u32_at(4), u32_at(8), u32_at(12));
    if version >= 22 {
        metadata_size = u32_at(20);
        file_size = i64_at(24);
        data_offset = i64_at(32);
    }
    let len = i64::try_from(data.len()).unwrap_or(i64::MAX);
    let bad = |x: i64| x < 0 || x > len;
    !(version > 100
        || [file_size, metadata_size, version, data_offset]
            .into_iter()
            .any(bad)
        || file_size < metadata_size
        || file_size < data_offset)
}

/// What UnityPy builds from a detected type (`ImportHelper.parse_file`).
pub fn parsed_as(ty: FileType, name: &str) -> &'static str {
    const RAW_SUFFIXES: [&str; 5] = [".resS", ".resource", ".config", ".xml", ".dat"];
    match ty {
        FileType::AssetsFile if !RAW_SUFFIXES.iter().any(|s| name.ends_with(s)) => "SerializedFile",
        FileType::BundleFile => "BundleFile",
        FileType::WebFile => "WebFile",
        FileType::Zip => "zip",
        FileType::Error => "error",
        _ => "raw",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quirks_match_the_spec() {
        assert_eq!(check_file_type(b"short"), FileType::ResourceFile);
        assert_eq!(check_file_type(&[b'x'; 20]), FileType::Error);
        let mut fa = vec![0xfa; 8];
        fa.extend_from_slice(&[0; 200]);
        assert_eq!(check_file_type(&fa), FileType::ResourceFile);
        let mut fa_utf8 = FA8.to_vec();
        fa_utf8.extend_from_slice(&[0; 200]);
        assert_eq!(check_file_type(&fa_utf8), FileType::BundleFile);
        let mut zip = b"PK\x03\x04\x14\x00".to_vec();
        zip.extend_from_slice(&[0; 200]);
        assert_eq!(check_file_type(&zip), FileType::ResourceFile);
    }
}
