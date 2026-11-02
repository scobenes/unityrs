//! The scene `Environment::find_file` is recorded on: every kind of file it
//! can return, found through the CAB registry and through the resolver.

use super::{Obj, Scene, bundle, named, res_bytes, serialized};

/// One `find_file(name, is_dependency)` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindAsk {
    /// The name asked for.
    pub name: Vec<u8>,
    /// UnityPy's `is_dependency` (its default is `True`).
    pub is_dependency: bool,
}

/// A scene and the `find_file` calls made on it, in order, once its files
/// are loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FindScene {
    /// The files and the resolver's.
    pub scene: Scene,
    /// The calls.
    pub asks: Vec<FindAsk>,
}

/// A plain WebFile (`UnityWebData1.0`, no compression) of `entries`, as
/// `files/WebFile.py:40-56` reads one: the signature to its NUL, the header
/// length, an `(offset, size, name length, name)` row per entry, the data.
#[must_use]
pub fn web_file(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let sig = b"UnityWebData1.0\0";
    let int = |n: usize| i32::try_from(n).unwrap_or(i32::MAX).to_le_bytes();
    let head = sig.len() + 4 + entries.iter().map(|(n, _)| 12 + n.len()).sum::<usize>();
    let mut out = sig.to_vec();
    out.extend_from_slice(&int(head));
    let mut offset = head;
    for (name, data) in entries {
        out.extend_from_slice(&int(offset));
        out.extend_from_slice(&int(data.len()));
        out.extend_from_slice(&int(name.len()));
        out.extend_from_slice(name.as_bytes());
        offset += data.len();
    }
    for (_, data) in entries {
        out.extend_from_slice(data);
    }
    out
}

/// Loaded: a bundle of a CAB and its `.resS`, a raw file and a
/// `SerializedFile` at top level. The resolver (UnityPy's `env.path`): a
/// bundle of a CAB and its `.resS`, a `SerializedFile`, a raw file, a second
/// bundle and a WebFile of one CAB. `find_file` is asked for each loaded
/// kind by its CAB name (the registry), for each resolver file by its name
/// (UnityPy's `path/name` step), for a CAB a resolver load registered, for
/// a resolver file again (now in the registry), for a name nothing answers
/// (the walk's `FileNotFoundError`), for the second bundle with
/// `is_dependency=False`, and last for the WebFile, which P1 refuses (plan
/// section 1).
#[must_use]
pub fn find_file_kinds() -> FindScene {
    let cab = |path_id: i64, name: &str| serialized(&[], &[Obj::named(path_id, name, &[])]);
    let ask = |name: &str, is_dependency: bool| FindAsk {
        name: name.as_bytes().to_vec(),
        is_dependency,
    };
    FindScene {
        scene: Scene {
            files: vec![
                named(
                    "a.bundle",
                    bundle(&[("CAB-A", cab(1, "a1")), ("CAB-A.resS", res_bytes(16, 1))]),
                ),
                named("top.resS", res_bytes(24, 0)),
                named("CAB-T", cab(2, "t2")),
            ],
            resolver: vec![
                named(
                    "r.bundle",
                    bundle(&[("CAB-R", cab(3, "r3")), ("CAB-R.resS", res_bytes(8, 5))]),
                ),
                named("cab-s", cab(4, "s4")),
                named("r.resS", res_bytes(12, 7)),
                named("q.bundle", bundle(&[("CAB-Q", cab(6, "q6"))])),
                named("w.web", web_file(&[("CAB-W", cab(5, "w5"))])),
            ],
        },
        asks: vec![
            ask("CAB-A", true),
            ask("CAB-A.resS", true),
            ask("top.resS", true),
            ask("CAB-T", true),
            ask("r.bundle", true),
            ask("CAB-R.resS", true),
            ask("cab-s", true),
            ask("cab-s", true),
            ask("r.resS", true),
            ask("nothing", true),
            ask("q.bundle", false),
            ask("w.web", true),
        ],
    }
}

/// Every `find_file` scene, by name: what `tools/fixtures/env.py` records.
#[must_use]
pub fn find_scenes() -> Vec<(&'static str, FindScene)> {
    vec![("find_file_kinds", find_file_kinds())]
}
