//! Split files and `files()` as an iterator.

use unityrs_core::RawStr;
use unityrs_core::env::{FileEntries, LoadMode, Source};
use unityrs_core::files::File;
use unityrs_testkit::env::{Obj, bundle, serialized};

use crate::{TempDir, env, named_bundle, src};

/// Split paths and a missing base load the joined bundle, in both modes
/// and through the dependency loader.
#[test]
fn split_files_load_as_one_source() {
    let dir = TempDir::new("split");
    let data = bundle(&[("CAB-S", serialized(&[], &[Obj::named(1, "s1", &[])]))]);
    let (a, b) = data.split_at(data.len() / 2);
    std::fs::write(dir.0.join("data.unity3d.split0"), a).unwrap();
    std::fs::write(dir.0.join("data.unity3d.split1"), b).unwrap();
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        for name in ["data.unity3d.split0", "data.unity3d.split1", "data.unity3d"] {
            let env = env(mode);
            let p = dir.0.join(name);
            assert!(matches!(
                env.load(Source::Path(p.clone())),
                Ok(File::Bundle(_))
            ));
            assert!(env.load_dependency(Source::Path(p)).is_ok());
            assert_eq!(env.files().count(), 1);
        }
    }
}

/// A name ending in `.split` and a numeric character that is no decimal
/// digit (`²` is `No`, `Ⅰ` is `Nl`) is one file, which UnityPy loads:
/// `reSplit`'s `\d` is Python's decimal digit (`tests/fixtures/d/pystr.json`).
/// Review round 2: the core refused it as the part of a split file.
#[test]
fn a_split_name_without_a_decimal_digit_is_one_file() {
    let dir = TempDir::new("split-numeric");
    let data = named_bundle("CAB-N", 16);
    for name in ["a.split\u{b2}", "a.split\u{2160}", "a.split\u{bd}0"] {
        std::fs::write(dir.0.join(name), &data).unwrap();
        for mode in [LoadMode::Eager, LoadMode::Lazy] {
            let env = env(mode);
            let p = dir.0.join(name);
            assert!(env.load(Source::Path(p.clone())).is_ok(), "{mode:?} {name}");
            assert!(
                env.load_dependency(Source::Path(p)).is_ok(),
                "{mode:?} {name}"
            );
            assert_eq!(env.files().count(), 1, "{mode:?} {name}");
        }
    }
}

/// `files()` is an iterator, as the other listings are: it allocates no
/// copy of the list (review round 1: it returned an uncharged `Vec`).
#[test]
fn files_is_an_iterator_over_the_list() {
    let env = env(LoadMode::Eager);
    env.load(src("a.bundle", &named_bundle("CAB-A", 16)))
        .unwrap();
    env.load(src("b.bundle", &named_bundle("CAB-B", 16)))
        .unwrap();
    let files: FileEntries<'_> = env.files();
    let names: Vec<_> = files.map(|f| f.name).collect();
    assert_eq!(names, [RawStr::from("a.bundle"), RawStr::from("b.bundle")]);
}
