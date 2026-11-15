//! All legacy formats and each WebFile packer, plus signed hostile lengths.
use super::Seeds;
use unityrs_testkit::web;
pub(super) fn seeds(s: &mut Seeds) -> std::io::Result<()> {
    for f in 1..=6 {
        for sig in ["UnityWeb", "UnityRaw"] {
            let data = web::legacy(sig.as_bytes(), f, &[("raw.resS", b"payload")]);
            s.add("unityweb", &format!("{sig}-{f}"), &data)?;
            s.add("bundle_save", &format!("{sig}-{f}"), &data)?;
        }
    }
    s.add("webfile", "huffman", &web::huffman())?;
    let inner = web::plain("TuanjieWebData1.0", &[("raw", b"payload")]);
    let plain = web::plain(
        "UnityWebData1.0",
        &[
            ("nested", &inner),
            ("duplicate", b"first"),
            ("duplicate", b"last"),
        ],
    );
    s.add("webfile", "plain", &plain)?;
    s.add("webfile", "gzip", &web::gzip(&plain))?;
    s.add("webfile", "brotli", &web::brotli(&plain))?;
    for n in [-12i32, -1, 0, i32::MAX] {
        let mut data = plain.clone();
        data[28..32].copy_from_slice(&n.to_le_bytes());
        s.add("webfile", &format!("name-{n}"), &data)?;
    }
    Ok(())
}
