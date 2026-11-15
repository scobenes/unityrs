//! Pristine manifest, packaged notices and static-link symbol isolation.
use sha2::{Digest, Sha256};
use std::fmt::Write;
use std::{collections::BTreeSet, path::Path, process::Command};
use unityrs_testkit::child;
#[path = "../build_webp_patch.rs"]
mod patch;

#[test]
fn webp_patches_require_exact_positions_context_and_lengths() {
    let source = "first\nold\nlast\n";
    let diff = "--- a/file.c\n+++ b/file.c\n@@ -2,1 +2,1 @@\n-old\n+new\n";
    assert_eq!(patch::apply(source, diff, "file.c"), "first\nnew\nlast\n");
    for changed in [
        diff.replace("-2,1", "-1,1"),
        diff.replace("+2,1", "+1,1"),
        diff.replace("-old", "-other"),
        diff.replace("-2,1", "-2,2"),
        diff.replace("+2,1", "+2,2"),
        diff.replace("b/file.c", "b/other.c"),
    ] {
        assert!(std::panic::catch_unwind(|| patch::apply(source, &changed, "file.c")).is_err());
    }
    let patched = patch::apply(source, diff, "file.c");
    assert!(std::panic::catch_unwind(|| patch::apply(&patched, diff, "file.c")).is_err());
}
fn files(root: &Path, names: &mut BTreeSet<String>, prefix: &str) {
    for entry in std::fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let name = format!("{prefix}{}", entry.file_name().to_str().unwrap());
        if entry.file_type().unwrap().is_dir() {
            files(&entry.path(), names, &format!("{name}/"));
        } else {
            assert!(entry.file_type().unwrap().is_file());
            names.insert(name);
        }
    }
}
#[test]
fn webp_sources_and_notices_are_pristine_and_complete() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(root.join("vendor/libwebp-1.3.0.sha256")).unwrap();
    let mut listed = BTreeSet::new();
    for line in manifest
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let (hash, name) = line.split_once("  ").unwrap();
        if name.ends_with(".tar.gz") {
            assert_eq!(
                hash,
                "64ac4614db292ae8c5aa26de0295bf1623dbb3985054cb656c55e67431def17c"
            );
            continue;
        }
        assert!(listed.insert(name.strip_prefix("libwebp-1.3.0/").unwrap().to_owned()));
        let bytes = std::fs::read(root.join("vendor").join(name)).unwrap();
        let actual = Sha256::digest(&bytes)
            .iter()
            .fold(String::new(), |mut s, b| {
                write!(s, "{b:02x}").unwrap();
                s
            });
        assert_eq!(hash, actual, "{name}");
    }
    let mut found = BTreeSet::new();
    files(&root.join("vendor/libwebp-1.3.0"), &mut found, "");
    assert_eq!(found, listed);
    for (package, vendor) in [
        ("LICENSE-WEBP", "COPYING"),
        ("NOTICE-WEBP-PATENTS", "PATENTS"),
    ] {
        assert_eq!(
            std::fs::read(root.join(package)).unwrap(),
            std::fs::read(root.join("vendor/libwebp-1.3.0").join(vendor)).unwrap()
        );
    }
}
#[test]
fn every_webp_global_is_prefixed_and_c_allocators_are_absent() {
    let output =
        child::output(Command::new("nm").args(["-g", env!("UNITYRS_IMAGEIO_WEBP_ARCHIVE")]))
            .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    let apple = cfg!(target_vendor = "apple");
    let mut definitions = 0;
    for line in text.lines().filter(|l| !l.trim_end().ends_with(':')) {
        let parts: Vec<_> = line.split_whitespace().collect();
        if let [_, kind, name] = parts.as_slice()
            && kind.len() == 1
            && *kind != "U"
        {
            let name = if apple {
                name.strip_prefix('_').unwrap_or(name)
            } else {
                name
            };
            assert!(
                name.starts_with("unityrs_w130_") || name == "unityrs_webp_encode",
                "unprefixed: {line}"
            );
            definitions += 1;
        }
        if let ["U", name] = parts.as_slice() {
            let name = if apple {
                name.strip_prefix('_').unwrap_or(name)
            } else {
                name
            };
            assert!(
                !["malloc", "calloc", "realloc", "free"].contains(&name),
                "C allocator: {line}"
            );
        }
    }
    assert!(definitions > 300);
}
