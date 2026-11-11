//! The zlib the encoder links is the vendored zlib 1.2.13, pristine
//! (`docs/plans/extract.md` E19): the library reports 1.2.13,
//! `vendor/zlib-1.2.13/` holds exactly the files `vendor/zlib-1.2.13.sha256`
//! lists, each with its hash there, which are those of the release tarball's
//! members (`scripts/zlib_vendor.py verify TARBALL` checks a downloaded
//! tarball against the same file), and the archive build.rs compiles
//! defines no name an unprefixed zlib defines.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;
use std::process::Command;

use sha2::{Digest, Sha256};
use unityrs_testkit::child;

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

/// Every name the vendored archive defines is one `Z_PREFIX` makes, a
/// `z_*` name zconf.h renames a zlib name to, or build.rs's
/// `unityrs_z_errmsg`; none is one an unprefixed zlib 1.2.13 defines,
/// zconf.h's unrenamed names and zlib's own `z_errmsg`. So the crate
/// links beside any zlib but a second `Z_PREFIX` one. The symbols are
/// `nm`'s, the platform's leading underscore taken off.
#[cfg(unix)]
#[test]
fn the_archive_defines_no_name_an_unprefixed_zlib_does() {
    let archive = Path::new(env!("UNITYRS_IMAGEIO_ZLIB_ARCHIVE"));
    let out = match child::output(Command::new("nm").arg("-g").arg(archive)) {
        Ok(out) => out,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            eprintln!(
                "skipped: no `nm` on this machine to read {}",
                archive.display()
            );
            return;
        }
        Err(e) => panic!("nm: {e}"),
    };
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8(out.stdout).unwrap();
    // `address type name` for a defined symbol; an undefined one has no
    // address, a member's heading one field.
    let defined: BTreeSet<&str> = text
        .lines()
        .filter_map(
            |line| match line.split_whitespace().collect::<Vec<_>>()[..] {
                [_, kind, name] if kind != "U" => Some(name),
                _ => None,
            },
        )
        .map(|name| {
            let apple = cfg!(target_vendor = "apple");
            if apple {
                name.strip_prefix('_').unwrap_or(name)
            } else {
                name
            }
        })
        .collect();
    let zconf = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/zlib-1.2.13/zconf.h"),
    )
    .unwrap();
    // `#  define deflate  z_deflate`.
    let renames: Vec<(&str, &str)> = zconf
        .lines()
        .filter_map(|l| match l.split_whitespace().collect::<Vec<_>>()[..] {
            ["#", "define", from, to] if to.strip_prefix("z_") == Some(from) => Some((from, to)),
            _ => None,
        })
        .collect();
    assert!(renames.len() > 100, "{} renames", renames.len());
    let unprefixed: BTreeSet<&str> = renames.iter().map(|r| r.0).chain(["z_errmsg"]).collect();
    let prefixed: BTreeSet<&str> = renames
        .iter()
        .map(|r| r.1)
        .chain(["unityrs_z_errmsg"])
        .collect();
    let clashing: Vec<&&str> = defined.intersection(&unprefixed).collect();
    assert!(
        clashing.is_empty(),
        "an unprefixed zlib's names: {clashing:?}"
    );
    let other: Vec<&&str> = defined.difference(&prefixed).collect();
    assert!(other.is_empty(), "names Z_PREFIX does not make: {other:?}");
    // The names were read: what the crate calls, and the renamed strings.
    for name in [
        "z_deflateInit2_",
        "z_deflate",
        "z_crc32",
        "z_zlibVersion",
        "unityrs_z_errmsg",
    ] {
        assert!(defined.contains(name), "{name} is not in {defined:?}");
    }
    assert!(defined.len() >= 30, "{defined:?}");
}
