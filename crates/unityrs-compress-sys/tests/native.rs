use std::collections::BTreeMap;
use std::path::Path;

use sha2::{Digest, Sha256};
use unityrs_compress_sys::{Error, lz4_bound, lz4_compress, lz4_state_words, lz4_version_number};

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut text, byte| {
        write!(text, "{byte:02x}").unwrap();
        text
    })
}

#[test]
fn pristine_vendor_hashes_and_licence_match_the_manifest() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(root.join("vendor/lz4-1.9.4.sha256")).unwrap();
    let mut expected = BTreeMap::new();
    for line in manifest
        .lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
    {
        let (sha, file) = line.split_once("  ").unwrap();
        if file == "lz4-4.4.4.tar.gz" {
            assert_eq!(
                sha,
                "070fd0627ec4393011251a094e08ed9fdcc78cb4e7ab28f507638eee4e39abda"
            );
            continue;
        }
        assert!(
            expected
                .insert(
                    file.strip_prefix("lz4-1.9.4/").unwrap().to_owned(),
                    sha.to_owned()
                )
                .is_none()
        );
    }
    let mut actual = BTreeMap::new();
    for entry in std::fs::read_dir(root.join("vendor/lz4-1.9.4")).unwrap() {
        let entry = entry.unwrap();
        let bytes = std::fs::read(entry.path()).unwrap();
        let hash = Sha256::digest(bytes);
        actual.insert(entry.file_name().into_string().unwrap(), hex(&hash));
    }
    assert_eq!(actual, expected);
    assert_eq!(actual.len(), 5);
    assert_eq!(
        std::fs::read(root.join("LICENSE-LZ4")).unwrap(),
        std::fs::read(root.join("vendor/lz4-1.9.4/LICENSE")).unwrap()
    );
}

#[test]
fn safe_lz4_bridge_checks_every_caller_slice() {
    assert_eq!(lz4_version_number(), 10904);
    let words = lz4_state_words().unwrap();
    let mut state = vec![0; words];
    let mut output = vec![0; lz4_bound(0).unwrap()];
    assert_eq!(lz4_compress(&[], &mut output, &mut []), Err(Error::Range));
    assert_eq!(lz4_compress(&[], &mut [], &mut state), Err(Error::Range));
    assert_eq!(lz4_compress(&[], &mut output, &mut state), Ok(1));
    assert_eq!(output[0], 0);
    assert_eq!(lz4_bound(usize::MAX), Err(Error::Range));
}

#[cfg(unix)]
#[test]
fn every_defined_c_symbol_has_the_private_prefix() {
    use std::process::Command;
    use unityrs_testkit::child;

    let result = child::output(
        Command::new("nm")
            .arg("-g")
            .arg(env!("UNITYRS_CORE_LZ4_ARCHIVE")),
    )
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let text = String::from_utf8(result.stdout).unwrap();
    let mut count = 0;
    for line in text.lines().filter(|line| !line.trim_end().ends_with(':')) {
        let fields: Vec<_> = line.split_whitespace().collect();
        if let [_, kind, symbol] = fields.as_slice()
            && kind.len() == 1
            && kind.as_bytes()[0].is_ascii_alphabetic()
            && *kind != "U"
        {
            let symbol = if cfg!(target_vendor = "apple") {
                symbol.strip_prefix('_').unwrap_or(symbol)
            } else {
                symbol
            };
            assert!(
                symbol.starts_with("unityrs_"),
                "unprefixed C symbol: {symbol}"
            );
            count += 1;
        }
    }
    assert!(count > 50, "the archive had only {count} defined symbols");
}
