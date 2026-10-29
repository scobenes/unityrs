//! The source scan behind plan section 3.16's rule: every public struct,
//! enum and trait of a crate is asserted `Send + Sync`.
//!
//! A type is named by its full module path (`files::bundle::BundleFile`),
//! taken from its file's place under `src/` and any `pub mod name { … }` it
//! sits in; items in a private inline module (`mod tests { … }`) are not
//! public. An assertion is any `send_sync::<Path>()` in any file of the
//! crate: a `crate::` path, or a path relative to the file's module (so
//! `lib.rs` names `files::bundle::BundleFile`, and `files/bundle.rs` may say
//! `crate::send_sync::<BundleFile>()` next to the type). A re-export does not
//! count, so a new type that shares a name with an asserted one is still
//! caught.
//!
//! Owner: the scaffold (implemented).

use std::collections::HashSet;
use std::path::Path;

/// The public types under `src` that no assertion names, as full paths.
///
/// # Panics
/// When `src` cannot be read (a test helper).
#[must_use]
pub fn unasserted(src: &Path) -> Vec<String> {
    let mut files = Vec::new();
    collect(src, src, &mut files);
    files.sort();
    check(&files)
}

/// The declared public types no assertion names, over `(module, text)`
/// pairs.
fn check(files: &[(String, String)]) -> Vec<String> {
    let mut declared = Vec::new();
    let mut asserted = HashSet::new();
    for (module, text) in files {
        scan(module, text, &mut declared, &mut asserted);
    }
    declared.retain(|p| !asserted.contains(p));
    declared
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|e| {
            e.unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
                .path()
        })
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            out.push((module_of(root, &path), text));
        }
    }
}

/// `lib.rs` and `main.rs` are the root, `a/mod.rs` and `a.rs` are `a`,
/// `a/b.rs` is `a::b`.
fn module_of(root: &Path, file: &Path) -> String {
    let rel = file.strip_prefix(root).unwrap_or(file).with_extension("");
    let mut parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if matches!(
        parts.last().map(String::as_str),
        Some("lib" | "main" | "mod")
    ) {
        parts.pop();
    }
    parts.join("::")
}

fn join(module: &str, name: &str) -> String {
    if module.is_empty() {
        name.to_owned()
    } else {
        format!("{module}::{name}")
    }
}

fn ident(s: &str) -> String {
    s.chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

/// An inline `mod name {`: its indent, name and whether it is `pub`.
struct Inline {
    indent: usize,
    name: String,
    public: bool,
}

fn scan(file_module: &str, text: &str, declared: &mut Vec<String>, asserted: &mut HashSet<String>) {
    let mut stack: Vec<Inline> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if trimmed.starts_with("//") {
            continue;
        }
        if trimmed == "}" && stack.last().is_some_and(|m| m.indent == indent) {
            stack.pop();
            continue;
        }
        let module = stack
            .iter()
            .fold(file_module.to_owned(), |m, i| join(&m, &i.name));
        for (kw, public) in [
            ("pub mod ", true),
            ("pub(crate) mod ", false),
            ("mod ", false),
        ] {
            if let Some(rest) = trimmed.strip_prefix(kw)
                && trimmed.ends_with('{')
            {
                stack.push(Inline {
                    indent,
                    name: ident(rest),
                    public,
                });
            }
        }
        if stack.iter().all(|m| m.public) {
            for kw in ["pub struct ", "pub enum ", "pub trait "] {
                if let Some(rest) = trimmed.strip_prefix(kw) {
                    declared.push(join(&module, &ident(rest)));
                }
            }
        }
        let mut rest = line;
        while let Some(at) = rest.find("send_sync::<") {
            rest = &rest[at + "send_sync::<".len()..];
            let Some(end) = rest.rfind(">()") else { break };
            let path = rest[..end].trim();
            let path = path.strip_prefix("dyn ").unwrap_or(path);
            let path = path.split('<').next().unwrap_or(path).trim();
            let full = match path.strip_prefix("crate::") {
                Some(abs) => abs.to_owned(),
                None => join(&module, path),
            };
            asserted.insert(full);
            rest = &rest[end..];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HashSet, check, scan};

    fn run(module: &str, text: &str) -> (Vec<String>, HashSet<String>) {
        let mut d = Vec::new();
        let mut a = HashSet::new();
        scan(module, text, &mut d, &mut a);
        (d, a)
    }

    #[test]
    fn types_are_named_by_their_full_path() {
        let (d, a) = run(
            "env::resource",
            "pub struct Permit(pub std::rc::Rc<u8>);\n\
             pub mod inner {\n    pub enum E {}\n}\n\
             mod tests {\n    pub struct Hidden;\n}\n\
             const _: () = crate::send_sync::<Ok>();\n",
        );
        assert_eq!(d, ["env::resource::Permit", "env::resource::inner::E"]);
        assert!(a.contains("env::resource::Ok"));
    }

    #[test]
    fn a_type_that_shares_an_asserted_name_is_still_unasserted() {
        // Round 2's probe: a second, non-Send `Permit` beside the asserted
        // `budget::Permit` passed a scan that matched bare names.
        let files = [
            (
                String::new(),
                "const _: () = send_sync::<budget::Permit>();\n".to_owned(),
            ),
            ("budget".to_owned(), "pub struct Permit;\n".to_owned()),
            (
                "env::resource".to_owned(),
                "pub struct Permit(pub std::rc::Rc<u8>);\n".to_owned(),
            ),
        ];
        assert_eq!(check(&files), ["env::resource::Permit"]);
    }

    #[test]
    fn assertions_resolve_crate_and_relative_paths() {
        let (_, a) = run(
            "",
            "    send_sync::<budget::DecodeBuf>();\n\
             \x20   send_sync::<dyn env::Fetch>();\n\
             \x20   send_sync::<walk::ReadCtx<'static>>();\n\
             \x20   send_sync::<crate::x::Y>();\n",
        );
        for p in ["budget::DecodeBuf", "env::Fetch", "walk::ReadCtx", "x::Y"] {
            assert!(a.contains(p), "{p}: {a:?}");
        }
    }
}
