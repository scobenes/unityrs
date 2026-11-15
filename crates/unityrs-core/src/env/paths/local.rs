// Ported from CPython 3.11 (PSF-2.0): Lib/posixpath.py. See THIRD_PARTY_LICENSES.
//! Physical paths for fsspec 2025.5.1's local filesystem: strip its local
//! protocols, expanduser, then strip trailing slashes (keeping `/`). Keys
//! continue to use the caller's spelling.
//! Only `~` and `~/...` expand, using HOME. Named users stay literal. With
//! HOME unset, paths also stay literal: `CPython`'s `pwd.getpwuid(os.getuid())`
//! fallback is omitted rather than adding a system-account lookup dependency.
//! fsspec's `_strip_protocol` and POSIX `make_path_posix`:
//! <https://github.com/fsspec/filesystem_spec/blob/2025.5.1/fsspec/implementations/local.py#L228-L294>

use crate::{Error, RawStr, env::split, limits::Meter};
use std::{path::Path, sync::Arc};

/// Resolve the physical spelling immediately before opening it. Compare
/// ancestor identities on Unix: canonicalize may preserve a caller's casing
/// on a case-insensitive filesystem mounted by a case-sensitive OS.
pub(in crate::env) fn confined_path(path: &Path, root: &Path) -> Result<std::path::PathBuf, Error> {
    let resolved = std::fs::canonicalize(path)?;
    let root = std::fs::canonicalize(root)?;
    if resolved.starts_with(&root) {
        return Ok(resolved);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let root_meta = std::fs::metadata(&root)?;
        for ancestor in resolved.ancestors() {
            let meta = std::fs::metadata(ancestor)?;
            if meta.dev() == root_meta.dev() && meta.ino() == root_meta.ino() {
                return Ok(resolved);
            }
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "file lies outside environment root",
    )
    .into())
}

pub(in crate::env) enum LocalPath<'a> {
    Borrowed(&'a Path),
    Expanded(RawStr),
}

impl LocalPath<'_> {
    pub(in crate::env) fn path(&self) -> Result<&Path, Error> {
        let raw = match self {
            Self::Borrowed(p) => p.as_os_str().as_encoded_bytes(),
            Self::Expanded(raw) => raw.as_bytes(),
        };
        // _strip_protocol applies rstrip only after make_path_posix, so
        // this must also trim the result of HOME expansion.
        split::path(strip_trailing_slashes(raw))
    }
}

fn strip_trailing_slashes(mut raw: &[u8]) -> &[u8] {
    while let Some((b'/', head)) = raw.split_last() {
        if head.is_empty() {
            break;
        }
        raw = head;
    }
    raw
}

fn strip_protocol(raw: &[u8]) -> &[u8] {
    [b"file://".as_slice(), b"local://", b"file:", b"local:"]
        .iter()
        .find_map(|prefix| raw.strip_prefix(*prefix))
        .unwrap_or(raw)
}

/// The root yielded by fsspec's walk: `make_path_posix`, then `rstrip('/')`.
/// Only a single leading './' is dropped; embedded '.', '..' and repeated
/// separators survive. Tilde paths return expanduser's spelling directly.
/// See local.py's POSIX branch (link above), and AbstractFileSystem.walk:
/// <https://github.com/fsspec/filesystem_spec/blob/2025.5.1/fsspec/spec.py#L384-L420>
pub(super) fn walk_root(path: &Path, meter: &Arc<Meter>) -> Result<RawStr, Error> {
    let raw = strip_protocol(path.as_os_str().as_encoded_bytes());
    meter.count_decode(raw.len() as u64)?;
    if raw.starts_with(b"/") || raw.starts_with(b"~") {
        let local = local_path(path, meter)?;
        return Ok(RawStr::charged(
            local.path()?.as_os_str().as_encoded_bytes(),
            meter,
        )?);
    }
    let raw = if let Some(tail) = raw.strip_prefix(b"./") {
        tail
    } else if raw == b"." {
        b""
    } else {
        raw
    };
    // rstrip can empty this relative tail; omit the joining slash then.
    let tail = raw
        .get(..raw.iter().rposition(|&b| b != b'/').map_or(0, |i| i + 1))
        .unwrap_or_default();
    let cwd = std::env::current_dir()?;
    let cwd = cwd.as_os_str().as_encoded_bytes();
    Ok(if tail.is_empty() {
        RawStr::charged(cwd, meter)?
    } else {
        RawStr::charged_concat(&[cwd, b"/", tail], meter)?
    })
}

pub(in crate::env) fn local_path<'a>(
    path: &'a Path,
    meter: &Arc<Meter>,
) -> Result<LocalPath<'a>, Error> {
    let raw = strip_protocol(path.as_os_str().as_encoded_bytes());
    // fsspec's make_path_posix resolves an empty (possibly stripped) path
    // to cwd. Borrowing "." has the same filesystem meaning without an
    // allocation, including before a folder/constructor existence check.
    if raw.is_empty() {
        return Ok(LocalPath::Borrowed(Path::new(".")));
    }
    if raw != b"~" && !raw.starts_with(b"~/") {
        return Ok(LocalPath::Borrowed(split::path(raw)?));
    }
    meter.count_decode(raw.len() as u64)?;
    // HOME is OS-owned, independent of input size. The expanded input
    // buffer is charged before copying. An empty HOME still expands.
    let Some(home) = std::env::var_os("HOME") else {
        return Ok(LocalPath::Borrowed(split::path(raw)?));
    };
    let home = home.as_encoded_bytes();
    // expanduser removes trailing slashes, including HOME="/" or HOME="".
    meter.count_decode(home.len() as u64)?;
    let home = home
        .get(..home.iter().rposition(|&b| b != b'/').map_or(0, |i| i + 1))
        .unwrap_or_default();
    let tail = raw.get(1..).unwrap_or_default();
    let parts = if home.is_empty() && tail.is_empty() {
        [b"/".as_slice(), b"".as_slice()]
    } else {
        [home, tail]
    };
    Ok(LocalPath::Expanded(RawStr::charged_concat(&parts, meter)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FileContext, budget::Reservation};

    #[test]
    fn physical_paths_strip_trailing_slashes_and_preserve_root() {
        let ctx = FileContext::unitypy();
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        for (path, expected) in [
            ("x.zip/", "x.zip"),
            ("file:x.zip///", "x.zip"),
            ("/", "/"),
            ("///", "/"),
            ("local:////", "/"),
        ] {
            let local = local_path(Path::new(path), &meter).unwrap();
            assert_eq!(
                local.path().unwrap().as_os_str().as_encoded_bytes(),
                expected.as_bytes()
            );
        }
    }

    #[test]
    fn named_users_are_left_literal() {
        let ctx = FileContext::unitypy();
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        for path in ["~root", "~root/x.zip", "~unityrs_no_such_user_9f4e/x.zip"] {
            let local = local_path(Path::new(path), &meter).unwrap();
            assert_eq!(local.path().unwrap(), Path::new(path));
        }
    }

    #[test]
    fn home_expansion_preserves_cpython_edge_cases() {
        use std::process::Command;
        for (home, tilde, child) in [
            ("", "/", "/x//y"),
            ("/", "/", "/x//y"),
            ("///", "/", "/x//y"),
            ("relative///", "relative", "relative/x//y"),
            ("spaces /", "spaces ", "spaces /x//y"),
        ] {
            let output = unityrs_testkit::child::output(
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "env::paths::local::tests::home_edge_probe",
                        "--ignored",
                    ])
                    .env("HOME", home)
                    .env("UNITYRS_EXPECT_TILDE", tilde)
                    .env("UNITYRS_EXPECT_CHILD", child),
            )
            .unwrap();
            assert!(output.status.success(), "HOME={home:?}: {output:?}");
        }
    }

    #[test]
    #[ignore = "runs in a child with a controlled HOME"]
    fn home_edge_probe() {
        let ctx = FileContext::unitypy();
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        for (path, expected) in [
            ("~", std::env::var_os("UNITYRS_EXPECT_TILDE").unwrap()),
            ("~/x//y", std::env::var_os("UNITYRS_EXPECT_CHILD").unwrap()),
        ] {
            let local = local_path(Path::new(path), &meter).unwrap();
            assert_eq!(local.path().unwrap().as_os_str(), expected);
        }
    }
}
