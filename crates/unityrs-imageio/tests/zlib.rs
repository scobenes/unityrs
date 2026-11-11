//! The zlib the encoder links is the vendored zlib 1.2.13, pristine
//! (`docs/plans/extract.md` E19): the library reports 1.2.13, and
//! `vendor/zlib-1.2.13/` holds exactly the files `vendor/zlib-1.2.13.sha256`
//! lists, each with its hash there, which are those of the release tarball's
//! members (`scripts/zlib_vendor.py verify TARBALL` checks a downloaded
//! tarball against the same file).

use std::collections::BTreeMap;
use std::path::Path;

use sha2::{Digest, Sha256};

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

#[test]
fn the_linked_zlib_is_1_2_13() {
    assert_eq!(unityrs_imageio::zlib_version(), "1.2.13");
}

#[test]
fn the_vendored_files_are_the_tarballs() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor");
    let manifest = std::fs::read_to_string(root.join("zlib-1.2.13.sha256")).unwrap();
    let mut listed = BTreeMap::new();
    let mut tarball = None;
    for line in manifest
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let (sha, name) = line.split_once("  ").unwrap();
        assert_eq!(sha.len(), 64, "{line}");
        if name == "zlib-1.2.13.tar.gz" {
            tarball = Some(sha.to_owned());
        } else {
            let file = name.strip_prefix("zlib-1.2.13/").unwrap();
            assert!(
                listed.insert(file.to_owned(), sha.to_owned()).is_none(),
                "{line}"
            );
        }
    }
    // zlib.net's and the GitHub release's zlib-1.2.13.tar.gz.
    assert_eq!(
        tarball.as_deref(),
        Some("b3a24de97a8fdbc835b9833169501030b8977031bcb54b3b3ac13740f846ab30")
    );
    let mut found = BTreeMap::new();
    for entry in std::fs::read_dir(root.join("zlib-1.2.13")).unwrap() {
        let entry = entry.unwrap();
        assert!(entry.file_type().unwrap().is_file(), "{:?}", entry.path());
        let bytes = std::fs::read(entry.path()).unwrap();
        found.insert(
            entry.file_name().into_string().unwrap(),
            hex(&Sha256::digest(&bytes)),
        );
    }
    assert_eq!(found, listed);
    // What build.rs compiles and includes, and the licence.
    for file in [
        "LICENSE",
        "adler32.c",
        "crc32.c",
        "deflate.c",
        "trees.c",
        "zutil.c",
        "zconf.h",
        "zlib.h",
    ] {
        assert!(listed.contains_key(file), "{file}");
    }
}

/// The crate's `LICENSE-ZLIB` is the vendored licence, so the package
/// carries zlib's notice.
#[test]
fn the_crate_carries_zlibs_licence() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ours = std::fs::read(crate_dir.join("LICENSE-ZLIB")).unwrap();
    let vendored = std::fs::read(crate_dir.join("vendor/zlib-1.2.13/LICENSE")).unwrap();
    assert_eq!(ours, vendored);
}
