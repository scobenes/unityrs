//! Input-order traversal and stable pagination, retaining at most one page.
use super::{Args, Document, Error, Result, document, meter, name, render};
use std::fmt::Write as _;
use std::{ffi::OsStr, os::unix::ffi::OsStrExt, path::Path, sync::Arc};
use unityrs_core::{
    ObjectRef, Parsed, Value,
    env::{EnvOptions, Environment, Source},
    files::File,
    limits::MeteredVec,
    provider::ProviderChain,
};
use unityrs_dump::{
    json,
    text::{Sink, Text},
};

fn environment(args: &Args<'_>) -> Result<Environment> {
    let mut opts = EnvOptions::unitypy();
    let mut ctx = (*opts.context).clone();
    ctx.limits = args.limits;
    if let Some(path) = args.get("--tpk") {
        ctx.providers = Arc::new(ProviderChain::new(vec![Arc::new(
            unityrs_tpk::TpkProvider::open_with(Path::new(OsStr::from_bytes(path)), &args.limits)
                .map_err(|e| {
                Error::new(
                    if matches!(
                        e,
                        unityrs_tpk::TpkError::TooLarge { .. }
                            | unityrs_tpk::TpkError::Allocator(_)
                    ) {
                        "budget"
                    } else {
                        "input"
                    },
                    super::bounded_message(&e),
                )
            })?,
        )]));
    }
    opts.context = Arc::new(ctx);
    Ok(Environment::new(opts))
}
fn load(env: &Environment, path: &[u8]) -> Result<File> {
    Ok(env.load(Source::Path(Path::new(OsStr::from_bytes(path)).to_owned()))?)
}
fn walk(root: File, mut visit: impl FnMut(&File) -> Result<()>) -> Result<()> {
    let mut stack = MeteredVec::new(root.meter());
    stack.push((root, 0usize))?;
    while let Some((file, next)) = stack.as_mut_slice().last_mut() {
        if *next == 0 {
            visit(file)?;
        }
        if let Some(slot) = file.container_files().get(*next) {
            let child = file.container_entry(slot.dir_index)?;
            *next += 1;
            stack.push((child, 0))?;
        } else {
            stack.pop();
        }
    }
    Ok(())
}
fn file_name(file: &File) -> &[u8] {
    match file {
        File::Serialized(f) => f.name().as_bytes(),
        File::Bundle(f) => f.name().as_bytes(),
        File::Web(f) => f.name().as_bytes(),
        File::Raw(f) => f.name.as_bytes(),
    }
}
fn metadata(out: &mut Text, file: &File) {
    out.push_str("{\"file\":");
    name(out, file_name(file));
    let _ = write!(
        out,
        ",\"kind\":\"{:?}\",\"unity_version\":",
        file.file_type()
    );
    match file {
        File::Serialized(f) => {
            if let Some(v) = f.unity_version() {
                name(out, v.as_bytes());
            } else {
                out.push_str("null");
            }
        }
        File::Bundle(f) => name(out, f.header().version_engine.as_bytes()),
        _ => out.push_str("null"),
    }
    out.push_str(",\"target_platform\":");
    match file {
        File::Serialized(f) => {
            if let Some(p) = f.target_platform() {
                let _ = write!(out, "{p}");
            } else {
                out.push_str("null");
            }
        }
        _ => out.push_str("null"),
    }
    out.push('}');
}
fn parsed_name(p: &Parsed) -> Option<&[u8]> {
    p.get(&[b"m_Name"]).and_then(Value::as_bytes)
}
fn read_name(obj: &ObjectRef) -> Result<Option<Parsed>> {
    match obj.parse_as_dict(false) {
        Ok(p) => Ok(Some(p)),
        Err(e) => {
            let e = Error::from(e);
            if e.code == "budget" { Err(e) } else { Ok(None) }
        }
    }
}
fn header(
    out: &mut Text,
    index: usize,
    obj: &ObjectRef,
    env: &Environment,
    parsed: Option<&Parsed>,
) -> Result<()> {
    let _ = write!(out, "{{\"input\":{index},\"file\":");
    name(out, obj.file().name().as_bytes());
    let _ = write!(
        out,
        ",\"path_id\":{},\"class_id\":{},\"class_name\":",
        obj.path_id(),
        obj.class_id()
    );
    name(
        out,
        unityrs_core::classid::class_name(obj.class_id())
            .unwrap_or("Unknown")
            .as_bytes(),
    );
    out.push_str(",\"name\":");
    if let Some(n) = parsed.and_then(parsed_name) {
        name(out, n);
    } else {
        out.push_str("null");
    }
    out.push_str(",\"container_path\":");
    if let Some(unityrs_core::container::ContainerPath::Str(path)) = env.container_path(obj)? {
        name(out, path.as_bytes());
    } else {
        out.push_str("null");
    }
    let _ = write!(out, ",\"byte_size\":{}}}", obj.info().byte_size);
    Ok(())
}
fn classes(args: &Args<'_>) -> Result<Vec<i32>> {
    args.options
        .iter()
        .filter(|(k, _)| *k == "--class")
        .map(|(_, b)| {
            let s = std::str::from_utf8(b).map_err(|_| Error::usage("invalid CLASS"))?;
            s.parse()
                .ok()
                .or_else(|| {
                    unityrs_core::classid::CLASS_NAMES
                        .iter()
                        .find(|(_, n)| *n == s)
                        .map(|(id, _)| *id)
                })
                .ok_or_else(|| Error::usage("unknown CLASS"))
        })
        .collect()
}
fn fingerprint(args: &Args<'_>) -> String {
    // Length framing prevents ambiguous concatenations, preserves raw argv bytes.
    let mut framed = Vec::new();
    for b in args.inputs.iter().copied().chain(
        args.options
            .iter()
            .filter(|(k, _)| !matches!(*k, "--cursor" | "--limit"))
            .flat_map(|(k, v)| [k.as_bytes(), *v]),
    ) {
        framed.extend_from_slice(&(b.len() as u64).to_le_bytes());
        framed.extend_from_slice(b);
    }
    json::sha256_hex(&framed)
}
fn token(key: &str, offset: usize) -> String {
    let payload = format!("{key}.{offset}");
    format!("{payload}.{}", json::sha256_hex(payload.as_bytes()))
}
fn offset(args: &Args<'_>, key: &str) -> Result<usize> {
    let Some(cursor) = args.get("--cursor") else {
        return Ok(0);
    };
    let bad = || {
        Error::new(
            "bad_cursor",
            "cursor does not belong to these inputs and filters",
        )
    };
    let cursor = std::str::from_utf8(cursor).map_err(|_| bad())?;
    let mut pieces = cursor.split('.');
    if pieces.next() != Some(key) {
        return Err(bad());
    }
    let n: usize = pieces.next().and_then(|s| s.parse().ok()).ok_or_else(bad)?;
    if n == 0 || token(key, n) != cursor {
        return Err(bad());
    }
    Ok(n)
}
#[allow(
    clippy::too_many_lines,
    reason = "one staged, transactional inventory page"
)]
pub fn inspect(args: &Args<'_>) -> Result<Document> {
    let classes = classes(args)?;
    let globs = args
        .options
        .iter()
        .filter(|(k, _)| *k == "--name")
        .map(|(_, b)| unityrs_dump::select::Glob::new(b).map_err(Error::usage))
        .collect::<Result<Vec<_>>>()?;
    let key = fingerprint(args);
    let offset = offset(args, &key)?;
    let limit = args.number("inspect", "--limit")?;
    let meter = meter(args);
    let mut inputs = Text::on(&meter);
    let mut objects = Text::on(&meter);
    let mut total = 0usize;
    let mut emitted = 0usize;
    for (index, path) in args.inputs.iter().enumerate() {
        if index > 0 {
            inputs.push(',');
        }
        inputs.push_str("{\"path\":");
        name(&mut inputs, path);
        let mut files = Text::on(&meter);
        let mut page = Text::on(&meter);
        let mut count = 0usize;
        let mut kept = 0usize;
        let result = (|| {
            let env = environment(args)?;
            let root = load(&env, path)?;
            inputs.push_str(",\"metadata\":");
            metadata(&mut inputs, &root);
            let mut first_file = true;
            walk(root, |file| {
                if !first_file {
                    files.push(',');
                }
                first_file = false;
                metadata(&mut files, file);
                if let File::Serialized(f) = file {
                    for obj in f.objects_by_path() {
                        if !classes.is_empty() && !classes.contains(&obj.class_id()) {
                            continue;
                        }
                        let p = read_name(&obj)?;
                        if !globs.is_empty()
                            && !p
                                .as_ref()
                                .and_then(parsed_name)
                                .is_some_and(|n| globs.iter().any(|g| g.matches(n)))
                        {
                            continue;
                        }
                        let position = total.saturating_add(count);
                        count += 1;
                        if position >= offset && emitted + kept < limit {
                            if kept > 0 {
                                page.push(',');
                            }
                            header(&mut page, index, &obj, &env, p.as_ref())?;
                            kept += 1;
                        }
                    }
                }
                Ok(())
            })?;
            if let Some(e) = files.refused().or_else(|| page.refused()) {
                return Err(Error::formatted("budget", e));
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                inputs.push_str(",\"files\":[");
                inputs.push_str(files.as_str());
                inputs.push(']');
                if kept > 0 {
                    if emitted > 0 {
                        objects.push(',');
                    }
                    objects.push_str(page.as_str());
                    emitted += kept;
                }
                total += count;
            }
            Err(e) => {
                inputs.push_str(",\"error\":");
                super::write_error(&mut inputs, &e);
            }
        }
        inputs.push('}');
    }
    if offset > total || (offset == total && offset != 0) {
        return Err(Error::new("bad_cursor", "cursor is past the inventory"));
    }
    let mut out = Text::on(&meter);
    out.push_str("{\"format\":\"unityrs.inspect\",\"version\":1,\"inputs\":[");
    out.push_str(inputs.as_str());
    out.push_str("],\"objects\":[");
    out.push_str(objects.as_str());
    let _ = write!(out, "],\"total_objects\":{total},\"next_cursor\":");
    if offset + emitted < total {
        name(&mut out, token(&key, offset + emitted).as_bytes());
    } else {
        out.push_str("null");
    }
    out.push_str("}\n");
    // A refused staging text is never published as a partial document.
    drop(inputs.finish()?);
    drop(objects.finish()?);
    document(out)
}
pub fn show(args: &Args<'_>) -> Result<Document> {
    let path_id = args.path_id()?;
    let meter = meter(args);
    let mut candidates = Text::on(&meter);
    candidates.push('[');
    let mut selected = None;
    let mut count = 0usize;
    for (index, path) in args.inputs.iter().enumerate() {
        let env = environment(args)?;
        let root = load(&env, path)?;
        if matches!(root, File::Raw(_)) {
            return Err(Error::new("input", "input is not a Unity object container"));
        }
        walk(root, |file| {
            if let File::Serialized(f) = file {
                if args
                    .get("--file")
                    .is_some_and(|name| name != f.name().as_bytes())
                {
                    return Ok(());
                }
                if let Some(obj) = f.object(path_id) {
                    if count > 0 {
                        candidates.push(',');
                    }
                    let _ = write!(candidates, "{{\"input\":{index},\"file\":");
                    name(&mut candidates, f.name().as_bytes());
                    candidates.push('}');
                    count += 1;
                    if selected.is_none() {
                        selected = Some((index, obj, env.clone()));
                    }
                }
            }
            Ok(())
        })?;
    }
    if count > 1 {
        candidates.push(']');
        let mut e = Error::new(
            "ambiguous",
            "path ID has multiple candidates; select --file",
        );
        e.detail = Some(Box::new(document(candidates)?));
        return Err(e);
    }
    let (index, object, env) =
        selected.ok_or_else(|| Error::new("not_found", "no object with that path ID and file"))?;
    let parsed = object.parse_as_dict(false)?;
    let mut head = Text::on(&meter);
    header(&mut head, index, &object, &env, Some(&parsed))?;
    let head = document(head)?;
    render::show(args, &parsed, &head.json, &meter)
}
