//! Split files, refused before they are read, and `files()` as an iterator.

use unityrs_core::RawStr;
use unityrs_core::env::{FileEntries, LoadMode, Source};
use unityrs_core::error::{Error, Unsupported};
use unityrs_core::files::File;
use unityrs_testkit::env::{Obj, bundle, res_bytes, serialized};

use crate::{TempDir, env, named_bundle, src};

/// A split file's parts (`name.split0`, ...), which UnityPy joins, are
/// refused with `Unsupported::Split` before anything is read: by a
/// `.splitN` name, and by a missing name whose `.split0` exists
/// (`environment.py:111-127`), in both modes and as a dependency (review
/// round 1: under `Lazy` the first part loaded as half a bundle).
#[test]
fn split_files_are_refused_before_they_are_read() {
    let dir = TempDir::new("split");
    let data = bundle(&[
        ("CAB-S", serialized(&[], &[Obj::named(1, "s1", &[])])),
        ("CAB-S.resS", res_bytes(4000, 2)),
    ]);
    let (a, b) = data.split_at(data.len() / 2);
    std::fs::write(dir.0.join("data.unity3d.split0"), a).unwrap();
    std::fs::write(dir.0.join("data.unity3d.split1"), b).unwrap();
    let split = |e: Result<File, Error>| matches!(e, Err(Error::Unsupported(Unsupported::Split)));
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let env = env(mode);
        for name in ["data.unity3d.split0", "data.unity3d.split1", "data.unity3d"] {
            let p = dir.0.join(name);
            assert!(split(env.load(Source::Path(p.clone()))), "{mode:?} {name}");
            assert!(
                split(env.load_dependency(Source::Path(p))),
                "{mode:?} {name}"
            );
        }
        assert!(env.files().next().is_none());
        assert_eq!(env.registry().len(), 0);
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
