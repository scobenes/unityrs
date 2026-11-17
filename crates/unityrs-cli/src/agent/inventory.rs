//! Input-order traversal and stable pagination, retaining at most one page.
use super::{Args, Document, Error, Result, cursor, document, meter, name, render};
use std::fmt::Write as _;
use std::{ffi::OsStr, os::unix::ffi::OsStrExt, path::Path, sync::Arc};
use unityrs_core::{
    ObjectRef, Parsed, Value,
    env::{EnvOptions, Environment, Source},
    files::File,
    limits::{Meter, MeteredVec},
    provider::ProviderChain,
};
use unityrs_dump::{
    json,
    text::{Sink, Text},
};

struct Provider {
    tree: Arc<unityrs_tpk::TpkProvider>,
    digest: [u8; 32],
}
fn provider(args: &Args<'_>, meter: &Arc<Meter>) -> Result<Option<Provider>> {
    let Some(path) = args.get("--tpk") else {
        return Ok(None);
    };
    let context = |mut error: Error| {
        error.message = super::bounded_message(&format_args!(
            "--tpk {}: {}",
            super::options::spelling(path),
            error.message
        ));
        error.at_input(0, args.inputs.first().copied().unwrap_or(path))
    };
    let data =
        unityrs_core::env::read_path(Path::new(OsStr::from_bytes(path)), &args.limits, meter)
            .map_err(|e| context(Error::from(e)))?;
    meter
        .count_decode(data.len() as u64)
        .map_err(|e| context(Error::from(e)))?;
    let digest = json::sha256(&data);
    // Parse exactly the bytes whose digest was computed, without reopening.
    let tree = unityrs_tpk::TpkProvider::from_bytes(&data, &args.limits).map_err(|e| {
        context(Error::new(
            if matches!(
                e,
                unityrs_tpk::TpkError::TooLarge { .. } | unityrs_tpk::TpkError::Allocator(_)
            ) {
                "budget"
            } else {
                "input"
            },
            super::bounded_message(&e),
        ))
    })?;
    Ok(Some(Provider {
        tree: Arc::new(tree),
        digest,
    }))
}
fn environment(args: &Args<'_>, provider: Option<&Provider>) -> Environment {
    let mut opts = EnvOptions::unitypy();
    let mut ctx = (*opts.context).clone();
    ctx.limits = args.limits;
    if let Some(provider) = provider {
        ctx.providers = Arc::new(ProviderChain::new(vec![provider.tree.clone()]));
    }
    opts.context = Arc::new(ctx);
    Environment::new(opts)
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
fn metadata(out: &mut Text, file: &File, file_index: usize) {
    if !out.count_work(256) {
        return;
    }
    let _ = write!(out, "{{\"file_index\":{file_index},\"file\":");
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
    out: &mut impl Sink,
    index: usize,
    file_index: usize,
    obj: &ObjectRef,
    env: &Environment,
    parsed: Option<&Parsed>,
) -> Result<()> {
    if !out.count_work(256) {
        return Ok(());
    }
    let _ = write!(
        out,
        "{{\"input\":{index},\"file_index\":{file_index},\"file\":"
    );
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
            let bad = || {
                super::options::bad_value("--class", b, "a class name or signed 32-bit integer ID")
            };
            let s = std::str::from_utf8(b).map_err(|_| bad())?;
            s.parse()
                .ok()
                .or_else(|| {
                    unityrs_core::classid::CLASS_NAMES
                        .iter()
                        .find(|(_, n)| *n == s)
                        .map(|(id, _)| *id)
                })
                .ok_or_else(bad)
        })
        .collect()
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
        .map(|(_, b)| {
            unityrs_dump::select::Glob::new(b).map_err(|e| {
                super::options::bad_value(
                    "--name",
                    b,
                    &format!("a valid case-sensitive glob ({e})"),
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let limit = args.number("inspect", "--limit")?;
    let meter = meter(args);
    let provider = provider(args, &meter).map_err(|e| {
        if args.get("--cursor").is_some() {
            Error::new(
                "bad_cursor",
                "cursor refused: inputs changed or TPK cannot be read",
            )
        } else {
            e
        }
    })?;
    let mut fingerprint =
        cursor::Fingerprint::new(args, &meter, provider.as_ref().map(|p| &p.digest))?;
    let resume = cursor::Resume::parse(args, &meter)?;
    let mut resumed = resume.is_none();
    let mut last = None;
    let mut more = false;
    let mut unbound = None;
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
        let mut page_last = None;
        let before_resumed = resumed;
        let before_more = more;
        let mut source_digest = None;
        let result = (|| {
            let env = environment(args, provider.as_ref());
            let root = env.load_observed(
                Source::Path(Path::new(OsStr::from_bytes(path)).to_owned()),
                |data, input_meter| {
                    input_meter.count_decode(data.len() as u64)?;
                    source_digest = Some(json::sha256(data));
                    Ok(())
                },
            )?;
            let mut first_file = true;
            let mut file_index = 0usize;
            walk(root, |file| {
                let occurrence = file_index;
                file_index += 1;
                if !first_file {
                    files.push(',');
                }
                first_file = false;
                metadata(&mut files, file, occurrence);
                if let File::Serialized(f) = file {
                    for obj in f.objects_by_path() {
                        if !classes.is_empty() && !classes.contains(&obj.class_id()) {
                            continue;
                        }
                        let p = read_name(&obj)?;
                        if !globs.is_empty() {
                            let mut matches = false;
                            if let Some(n) = p.as_ref().and_then(parsed_name) {
                                for glob in &globs {
                                    if glob.matches_metered(n, obj.file().meter())? {
                                        matches = true;
                                        break;
                                    }
                                }
                            }
                            if !matches {
                                continue;
                            }
                        }
                        count += 1;
                        if !resumed {
                            if resume.as_ref().is_some_and(|r| {
                                r.key
                                    .matches(index, occurrence, f.name().as_bytes(), obj.path_id())
                            }) {
                                resumed = true;
                            }
                            continue;
                        }
                        if emitted + kept < limit {
                            if kept > 0 {
                                page.push(',');
                            }
                            header(&mut page, index, occurrence, &obj, &env, p.as_ref())?;
                            kept += 1;
                            page_last = Some(cursor::Key::new(
                                index,
                                occurrence,
                                f.name().as_bytes(),
                                obj.path_id(),
                                &meter,
                            )?);
                        } else {
                            more = true;
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
        if let Some(digest) = &source_digest {
            fingerprint.field(b"input-digest", digest, &meter)?;
        }
        match result {
            Ok(()) => {
                if page_last.is_some() {
                    last = page_last;
                }
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
                resumed = before_resumed;
                more = before_more;
                if source_digest.is_none() {
                    unbound = Some((index, *path, e.code));
                }
                fingerprint.field(b"input-error", e.code.as_bytes(), &meter)?;
                let e = e.at_input(index, path);
                inputs.push_str(",\"error\":");
                super::write_error(&mut inputs, &e);
            }
        }
        inputs.push('}');
    }
    if let Some((index, path, code)) = unbound {
        if resume.is_some() {
            return Err(Error::new(
                "bad_cursor",
                "cursor refused: inputs changed or cannot be read within budgets",
            ));
        }
        if more {
            return Err(Error::new(
                code,
                "cannot issue a cursor: input bytes unavailable for fingerprinting",
            )
            .at_input(index, path));
        }
    }
    let key = fingerprint.finish();
    if let Some(resume) = &resume {
        resume.verify(&key, resumed)?;
    }
    let mut out = Text::on(&meter);
    out.push_str("{\"format\":\"unityrs.inspect\",\"version\":1,\"inputs\":[");
    out.push_str(inputs.as_str());
    out.push_str("],\"objects\":[");
    out.push_str(objects.as_str());
    let _ = write!(out, "],\"total_objects\":{total},\"next_cursor\":");
    if more {
        if let Some(last) = last {
            let token = last.token(&key, &meter)?;
            name(&mut out, token.json.as_bytes());
        } else {
            out.push_str("null");
        }
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
    let address = args.file_index()?;
    let meter = meter(args);
    let provider = provider(args, &meter)?;
    let mut candidates = Text::on(&meter);
    candidates.push('[');
    let mut selected = None;
    let mut count = 0usize;
    for (index, path) in args.inputs.iter().enumerate() {
        let env = environment(args, provider.as_ref());
        let root = load(&env, path).map_err(|e| e.at_input(index, path))?;
        if matches!(root, File::Raw(_)) {
            return Err(
                Error::new("input", "input is not a Unity object container").at_input(index, path)
            );
        }
        let mut file_index = 0usize;
        walk(root, |file| {
            let occurrence = file_index;
            file_index += 1;
            if address.is_some_and(|selected| selected != (index, occurrence)) {
                return Ok(());
            }
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
                    let _ = write!(
                        candidates,
                        "{{\"input\":{index},\"file_index\":{occurrence},\"file\":"
                    );
                    name(&mut candidates, f.name().as_bytes());
                    candidates.push('}');
                    count += 1;
                    if selected.is_none() {
                        selected = Some((index, occurrence, obj, env.clone(), *path));
                    }
                }
            }
            Ok(())
        })
        .map_err(|e| e.at_input(index, path))?;
    }
    if count > 1 {
        candidates.push(']');
        let mut e = Error::new(
            "ambiguous",
            "path ID has multiple candidates; select --file-index INPUT:FILE from detail (or --file NAME when unique)",
        );
        e.detail = Some(Box::new(document(candidates)?));
        return Err(e);
    }
    let (index, file_index, object, env, path) =
        selected.ok_or_else(|| Error::new("not_found", "no object with that path ID and file"))?;
    let parsed = object
        .parse_as_dict(false)
        .map_err(|e| Error::from(e).at_input(index, path))?;
    let mut length = render::Length::on(&meter);
    header(&mut length, index, file_index, &object, &env, Some(&parsed))
        .map_err(|e| e.at_input(index, path))?;
    let header_len = length.finish()?;
    render::show(
        args,
        &parsed,
        header_len,
        |out| {
            header(out, index, file_index, &object, &env, Some(&parsed))
                .map_err(|e| e.at_input(index, path))
        },
        &meter,
    )
    .map_err(|e| {
        if e.code == "input" && e.detail.is_none() {
            e.at_input(index, path)
        } else {
            e
        }
    })
}
