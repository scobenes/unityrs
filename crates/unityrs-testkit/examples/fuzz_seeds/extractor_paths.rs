//! Serialized builder files plus hostile byte paths for both interpretations.
use super::Seeds;
pub(super) fn file(s: &mut Seeds, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    s.add("extractor_paths", name, bytes)
}
pub(super) fn write(s: &mut Seeds) -> std::io::Result<()> {
    for (name, bytes) in [
        ("parent", b"../escape".as_slice()),
        ("nested-parent", b"a/../../escape"),
        ("absolute", b"/escape"),
        ("nul", b"a\0b"),
        ("non-utf8", b"Asset\xff/image.sgi"),
        ("suffix", b"a/.hidden.ext"),
        ("colon", b"C:/escape"),
        ("backslash", b"a\\b"),
    ] {
        s.add("extractor_paths", name, bytes)?;
    }
    Ok(())
}
