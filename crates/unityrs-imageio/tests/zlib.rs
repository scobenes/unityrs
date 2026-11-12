//! The zlib the encoder derives its compiled deflate from is pristine
//! zlib 1.2.13 (only `longest_match` is replaced in `OUT_DIR`).
//! (`docs/plans/extract.md` E19): the library reports 1.2.13,
//! `vendor/zlib-1.2.13/` holds exactly the files `vendor/zlib-1.2.13.sha256`
//! lists, each with its hash there, and the archive build.rs compiles
//! defines no name an unprefixed zlib defines. That the manifest's hashes
//! are the release tarball's is `scripts/zlib_vendor.py verify TARBALL`'s
//! to check, which CI's parity-py job runs on the downloaded tarball.

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
fn the_vendored_files_are_the_manifests() {
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

/// The names `nm -g` says `text`'s archive defines: each `address kind
/// name` line whose kind is one letter but `U` (an undefined symbol has no
/// address), less Apple's leading underscore when `apple`. A member's
/// heading ends in `:`; Apple's `nm` writes it with the archive's whole
/// path, so a path holding two spaces splits into three fields as well.
fn defined_names(text: &str, apple: bool) -> BTreeSet<&str> {
    let defined =
        |kind: &str| matches!(kind.as_bytes(), [k] if k.is_ascii_alphabetic() && *k != b'U');
    text.lines()
        .filter(|line| !line.trim_end().ends_with(':'))
        .filter_map(
            |line| match line.split_whitespace().collect::<Vec<_>>()[..] {
                [_, kind, name] if defined(kind) => Some(name),
                _ => None,
            },
        )
        .map(|name| {
            if apple {
                name.strip_prefix('_').unwrap_or(name)
            } else {
                name
            }
        })
        .collect()
}

#[test]
fn the_nm_parse_reads_only_defined_symbols() {
    // Apple's headings name the archive by its path, spaces and all.
    let apple = "\n/t a b/libunityrs_zlib.a(deflate.o):\n\
                 0000000000000010 T _z_deflate\n\
                 \x20                U _memcpy\n\
                 0000000000000000 D _unityrs_z_errmsg\n\
                 \n/t ab c/libunityrs_zlib.a(trees.o):\n\
                 0000000000000020 T _z__tr_init\n\
                 0000000000000030 -- _a_kind_of_two\n";
    let want = BTreeSet::from(["z_deflate", "unityrs_z_errmsg", "z__tr_init"]);
    assert_eq!(defined_names(apple, true), want);
    // GNU nm's heading is the member's name alone.
    let gnu = "\ndeflate.o:\n0000000000000010 T z_deflate\n                 U memcpy\n";
    assert_eq!(defined_names(gnu, false), BTreeSet::from(["z_deflate"]));
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
    let defined = defined_names(&text, cfg!(target_vendor = "apple"));
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
    let mut prefixed: BTreeSet<&str> = renames
        .iter()
        .map(|r| r.1)
        .chain(["unityrs_z_errmsg"])
        .collect();
    // The optional pristine reference is namespaced independently, and
    // must export only deflate names that zconf.h also knows about.
    let reference_names: Vec<String> = renames
        .iter()
        .filter(|(from, _)| from.starts_with("deflate"))
        .map(|(from, _)| format!("unityrs_zref_{from}"))
        .collect();
    if cfg!(feature = "test-reference") {
        prefixed.extend(reference_names.iter().map(String::as_str));
        for name in [
            "unityrs_zref_deflateInit2_",
            "unityrs_zref_deflate",
            "unityrs_zref_deflateEnd",
        ] {
            assert!(defined.contains(name), "missing reference symbol: {name}");
        }
    } else {
        assert!(!defined.iter().any(|name| name.starts_with("unityrs_zref_")));
    }
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
