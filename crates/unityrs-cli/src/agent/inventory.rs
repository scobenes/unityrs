//! Input-order traversal and stable pagination, retaining at most one page.
use super::{Args, Document, Error, Result, cursor, document, listing, meter, name, render};
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
    let context = |error: Error| error.at_tpk(path);
    let file = if let Some(roots) = &args.roots {
        roots.open(Path::new(OsStr::from_bytes(path)))
    } else {
        std::fs::File::open(Path::new(OsStr::from_bytes(path)))
    }
    .map_err(|e| context(Error::from(unityrs_core::Error::from(e))))?;
    let data = crate::provider_input::read(file, &args.limits, meter).map_err(|e| {
        context(if let Some(e) = e.downcast_ref::<unityrs_tpk::TpkError>() {
            Error::tpk(e)
        } else if let Some(e) = e.downcast_ref::<unityrs_core::Error>() {
            Error::from(e.clone())
        } else {
            Error::input(e)
        })
    })?;
    meter
        .count_decode(data.len() as u64)
        .map_err(|e| context(Error::from(e)))?;
    let digest = json::sha256(&data);
    // Parse exactly the bytes whose digest was computed, without reopening.
    let tree = unityrs_tpk::TpkProvider::from_bytes_metered(&data, &args.limits, meter)
        .map_err(|e| context(Error::tpk(&e)))?;
    Ok(Some(Provider {
        tree: Arc::new(tree),
        digest,
    }))
}
fn environment(args: &Args<'_>, provider: Option<&Provider>) -> Environment {
    let mut opts = EnvOptions::unitypy();
    let mut ctx = (*opts.context).clone();
    ctx.limits = args.limits;
    if args.roots.is_some() {
        ctx.budget = Arc::new(unityrs_core::budget::Counting::new(
            args.limits.max_input_memory,
        ));
    }
    if let Some(provider) = provider {
        ctx.providers = Arc::new(ProviderChain::new(vec![provider.tree.clone()]));
    }
    opts.context = Arc::new(ctx);
    Environment::new(opts)
}
fn source(env: &Environment, args: &Args<'_>, path: &[u8]) -> Source {
    let path = Path::new(OsStr::from_bytes(path));
    args.roots.as_ref().map_or_else(
        || Source::Path(path.to_owned()),
        |roots| roots.source(path, args.limits, env.options().context.budget.clone()),
    )
}
fn load(env: &Environment, path: &[u8], args: &Args<'_>) -> Result<File> {
    Ok(env.load(source(env, args, path))?)
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
    listing::string(out, file_name(file), true);
    let _ = write!(
        out,
        ",\"kind\":\"{:?}\",\"unity_version\":",
        file.file_type()
    );
    match file {
        File::Serialized(f) => {
            if let Some(v) = f.unity_version() {
                listing::string(out, v.as_bytes(), true);
            } else {
                out.push_str("null");
            }
        }
        File::Bundle(f) => listing::string(out, f.header().version_engine.as_bytes(), true),
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
    preview: bool,
) -> Result<()> {
    if !out.count_work(256) {
        return Ok(());
    }
    let _ = write!(
        out,
        "{{\"input\":{index},\"file_index\":{file_index},\"file\":"
    );
    listing::string(out, obj.file().name().as_bytes(), preview);
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
        listing::string(out, n, preview);
    } else {
        out.push_str("null");
    }
    out.push_str(",\"container_path\":");
    if let Some(unityrs_core::container::ContainerPath::Str(path)) = env.container_path(obj)? {
        listing::string(out, path.as_bytes(), preview);
    } else {
        out.push_str("null");
    }
    let _ = write!(out, ",\"byte_size\":{}}}", obj.info().byte_size);
    Ok(())
}
fn classes(args: &Args<'_>) -> Result<Vec<i32>> {
    let mut classes = args
        .options
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
        .collect::<Result<Vec<_>>>()?;
    classes.sort_unstable();
    classes.dedup();
    Ok(classes)
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
    let provider = provider(args, &meter).map_err(|mut e| {
        if args.get("--cursor").is_some() && e.code != "budget" {
            e.code = "bad_cursor";
            e.message = super::bounded_message(&format_args!(
                "cursor refused: inputs changed or TPK cannot be read: {}",
                e.message
            ));
        }
        e
    })?;
    let mut fingerprint =
        cursor::Fingerprint::new(args, &meter, provider.as_ref().map(|p| &p.digest))?;
    let resume = cursor::Resume::parse(args, &meter)?;
    let mut resumed = resume.is_none();
    let mut more = false;
    let mut unbound = None;
    let mut inputs = Text::on(&meter);
    let mut objects = listing::Page::new(&meter, args.number("inspect", "--max-bytes")?);
    let mut total = 0usize;
    let mut response_object = None;
    for (index, path) in args.inputs.iter().enumerate() {
        if index > 0 {
            inputs.push(',');
        }
        inputs.push_str("{\"path\":");
        listing::string(&mut inputs, path, true);
        let mut files = Text::on(&meter);
        let checkpoint = objects.checkpoint();
        let mut count = 0usize;
        let before_resumed = resumed;
        let before_more = more;
        let mut source_digest = None;
        let result = (|| {
            let env = environment(args, provider.as_ref());
            let root = env.load_observed(source(&env, args, path), |data, input_meter| {
                input_meter.count_decode(data.len() as u64)?;
                source_digest = Some(json::sha256(data));
                Ok(())
            });
            // Bind the loaded snapshot before object work. A fingerprint
            // refusal then cannot replace an object's contextual failure.
            if let Some(digest) = &source_digest {
                fingerprint.field(b"input-digest", digest, &meter)?;
            }
            let root = root?;
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
                if let Some(e) = files.refused() {
                    return Err(Error::from(e.clone()));
                }
                if let File::Serialized(f) = file {
                    for obj in f.objects_by_path() {
                        let result = (|| {
                            if !classes.is_empty() {
                                // Reserve the binary search's worst-case comparisons
                                // before comparing, even for non-matching objects.
                                obj.file()
                                    .meter()
                                    .count_decode(u64::from(classes.len().ilog2()) + 2)?;
                                if classes.binary_search(&obj.class_id()).is_err() {
                                    return Ok(());
                                }
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
                                    return Ok(());
                                }
                            }
                            count += 1;
                            if !resumed {
                                if let Some(r) = &resume
                                    && r.key.matches(
                                        index,
                                        occurrence,
                                        f.name().as_bytes(),
                                        obj.path_id(),
                                        &meter,
                                    )?
                                {
                                    resumed = true;
                                }
                                return Ok(());
                            }
                            if !more && objects.len() < limit {
                                response_object.get_or_insert((
                                    index,
                                    *path,
                                    occurrence,
                                    obj.path_id(),
                                ));
                                let mut text = Text::on(&meter);
                                header(&mut text, index, occurrence, &obj, &env, p.as_ref(), true)?;
                                let key = cursor::Key::new(
                                    index,
                                    occurrence,
                                    f.name().as_bytes(),
                                    obj.path_id(),
                                    &meter,
                                )?;
                                if !objects.push(document(text)?, key)? {
                                    more = true;
                                }
                            } else {
                                more = true;
                            }
                            Ok(())
                        })();
                        result.map_err(|e: Error| {
                            e.at_object(index, path, occurrence, obj.path_id())
                        })?;
                    }
                }
                Ok(())
            })?;
            if let Some(e) = files.refused() {
                return Err(Error::from(e.clone()));
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                inputs.push_str(",\"files\":[");
                inputs.push_str(files.as_str());
                inputs.push(']');
                total += count;
            }
            Err(e) => {
                objects.rollback(checkpoint);
                resumed = before_resumed;
                more = before_more;
                if source_digest.is_none() {
                    unbound = Some(Error {
                        code: e.code,
                        message: super::bounded_message(&format_args!("cannot issue a cursor: input bytes unavailable for fingerprinting: {}", e.message)),
                        detail: None,
                    }.at_input(index, path));
                }
                let e = e.at_input(index, path);
                if let Err(error) = fingerprint.field(b"input-error", e.code.as_bytes(), &meter) {
                    return Err(if e.code == "budget" {
                        e
                    } else {
                        error.at_input(index, path)
                    });
                }
                inputs.push_str(",\"error\":");
                super::write_error(&mut inputs, &e);
                if let Some(error) = inputs.refused() {
                    return Err(if e.code == "budget" {
                        e
                    } else {
                        Error::from(error.clone()).at_input(index, path)
                    });
                }
            }
        }
        inputs.push('}');
    }
    if resume.is_some()
        && let Some(mut error) = unbound.take()
    {
        if error.code != "budget" {
            error.code = "bad_cursor";
            error.message = super::bounded_message(&format_args!(
                "cursor refused: inputs changed: {}",
                error.message
            ));
        }
        return Err(error);
    }
    let key = fingerprint.finish();
    if let Some(resume) = &resume {
        resume.verify(&key, resumed)?;
    }
    let cursor_error = unbound;
    let result = (|| objects.finish(&document(inputs)?, total, more, &key, cursor_error, &meter))();
    result.map_err(|error| {
        if error.code == "budget"
            && error.detail.is_none()
            && let Some((index, path, file_index, path_id)) = response_object
        {
            error.at_object(index, path, file_index, path_id)
        } else {
            error
        }
    })
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
        if address.is_some_and(|(selected_input, _)| selected_input != index) {
            continue;
        }
        let env = environment(args, provider.as_ref());
        let root = load(&env, path, args).map_err(|e| e.at_input(index, path))?;
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
                    if let Some(e) = candidates.refused() {
                        return Err(
                            Error::from(e.clone()).at_object(index, path, occurrence, path_id)
                        );
                    }
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
    let result = (|| {
        let parsed = object.parse_as_dict(false).map_err(|error| {
            let missing_tree = matches!(error, unityrs_core::walk::ParseError::NoTypeTree { .. });
            let error = Error::from(error);
            if missing_tree {
                error.missing_tree(args)
            } else {
                error
            }
        })?;
        let mut length = render::Length::on(&meter);
        header(
            &mut length,
            index,
            file_index,
            &object,
            &env,
            Some(&parsed),
            false,
        )?;
        let header_len = length.finish()?;
        render::show(
            args,
            &parsed,
            header_len,
            |out| header(out, index, file_index, &object, &env, Some(&parsed), false),
            &meter,
        )
    })();
    result.map_err(|e: Error| e.at_object(index, path, file_index, path_id))
}
