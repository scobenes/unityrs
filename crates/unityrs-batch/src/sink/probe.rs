//! The case and normalization probe (`docs/BATCH_SPEC.md` 7.2). Keys compare
//! byte-exactly, so a sink that folds two of them into one entry would let
//! the second write replace the first while the head lists both.
use super::{Class, Context, Error, path::Root};
use std::{ffi::CStr, fs::File, os::unix::fs::MetadataExt};

/// Each pair is one entry on a sink that folds it: ASCII case, then Unicode
/// normalization (U+00E9 against `e` followed by U+0301), with the fixed
/// `sink_root_unusable` detail its fold gives. The names are in the temp
/// files' reserved namespace (no key may name them, a leftover is a temp file
/// the caller may delete), and never a temp file's own name, whose suffix is
/// digits.
const PAIRS: [(&CStr, &CStr, &str); 2] = [
    (
        c".unityrs-tmp-probe-a",
        c".unityrs-tmp-probe-A",
        "case_insensitive",
    ),
    (
        c".unityrs-tmp-probe-\u{e9}",
        c".unityrs-tmp-probe-e\u{301}",
        "normalization_insensitive",
    ),
];

/// Creates each pair under the run's own directory `run`, held at `key`'s
/// parent, and removes it. A pair whose second create fails with `EEXIST`,
/// or resolves to the first's inode, refuses the sink. Skipped only for a
/// test adapter that answers `probe_sink_semantics` false.
pub(crate) fn sensitive(root: &Root, run: &File, key: &str) -> Result<(), Error> {
    if !root.fs().probe_sink_semantics() {
        return Ok(());
    }
    for (first, second, detail) in PAIRS {
        root.revalidate(run, key, Context::Preflight)?;
        clear(root, run, first)?;
        clear(root, run, second)?;
        let folded = pair(root, run, key, first, second);
        let cleared = clear(root, run, first).and_then(|()| clear(root, run, second));
        if folded? {
            return Err(Error::plain(Class::SinkRootUnusable, detail));
        }
        cleared?;
    }
    Ok(())
}
fn pair(root: &Root, run: &File, key: &str, first: &CStr, second: &CStr) -> Result<bool, Error> {
    let cx = Context::Preflight;
    let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW;
    root.revalidate(run, key, cx)?;
    let one = root.child(run, first, flags, cx)?;
    root.revalidate(run, key, cx)?;
    let two = match root.child(run, second, flags, cx) {
        Ok(two) => two,
        Err(e) if e.errno == Some(libc::EEXIST) => return Ok(true),
        Err(e) => return Err(e),
    };
    let one = one.metadata().map_err(|e| Error::io(cx, "fstat", &e))?;
    let two = two.metadata().map_err(|e| Error::io(cx, "fstat", &e))?;
    Ok((one.dev(), one.ino()) == (two.dev(), two.ino()))
}
fn clear(root: &Root, run: &File, name: &CStr) -> Result<(), Error> {
    match root.fs().unlink(run, name) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(Context::Preflight, "unlinkat", &e)),
    }
}
