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

/// Creates each pair in the root itself, held at a one-segment key, and in
/// the run's own directory `run`, held at `key`'s parent, and removes it. A
/// pair whose second create fails with `EEXIST`, or resolves to the first's
/// inode, refuses the sink. Both directories, since a directory's folding is
/// its own (ext4's casefold attribute): a root that folds can hold an
/// `_unityrs` that does not. Skipped only for a test adapter that answers
/// `probe_sink_semantics` false.
pub(crate) fn sensitive(root: &Root, run: &File, key: &str) -> Result<(), Error> {
    if !root.fs().probe_sink_semantics() {
        return Ok(());
    }
    for (dir, key) in [(root.file(), "probe"), (run, key)] {
        for (first, second, detail) in PAIRS {
            root.revalidate(dir, key, Context::Preflight)?;
            clear(root, dir, first)?;
            clear(root, dir, second)?;
            let folded = pair(root, dir, key, first, second);
            // Both removals are tried, whatever the first gives; its error
            // is the one kept.
            let one = clear(root, dir, first);
            let two = clear(root, dir, second);
            if folded? {
                return Err(Error::plain(Class::SinkRootUnusable, detail));
            }
            one.and(two)?;
        }
    }
    Ok(())
}
fn pair(root: &Root, dir: &File, key: &str, first: &CStr, second: &CStr) -> Result<bool, Error> {
    let cx = Context::Preflight;
    let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW;
    root.revalidate(dir, key, cx)?;
    let one = root.child(dir, first, flags, cx)?;
    root.revalidate(dir, key, cx)?;
    let two = match root.child(dir, second, flags, cx) {
        Ok(two) => two,
        Err(e) if e.errno == Some(libc::EEXIST) => return Ok(true),
        Err(e) => return Err(e),
    };
    let one = one.metadata().map_err(|e| Error::io(cx, "fstat", &e))?;
    let two = two.metadata().map_err(|e| Error::io(cx, "fstat", &e))?;
    Ok((one.dev(), one.ino()) == (two.dev(), two.ino()))
}
fn clear(root: &Root, dir: &File, name: &CStr) -> Result<(), Error> {
    match root.fs().unlink(dir, name) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(Context::Preflight, "unlinkat", &e)),
    }
}
