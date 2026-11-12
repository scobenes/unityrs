//! The cross-field rules (`docs/BATCH_SPEC.md` 1.1), each with its
//! pointer, in section 10's order: `budgets`, `duplicate_id`,
//! `duplicate_header`, `backoff`, `deps`, `view_size`, `spool_in_sink`,
//! `path_style`, `deadline_required`, `unknown_input`, [B2: `template`,
//! `key(<rule>)`], `archive_key`, `archive_kind`, `source`,
//! `headers_without_url`, `outputs_empty`, `prefer_empty`.
//!
//! Where a rule can break in several places, the first in document order
//! is named.

use std::path::Path;

use super::super::build::DepsLists;
use super::super::formats;
use super::super::model::{BundleRef, Sink, Spec};
use super::super::{BadSpec, Rule, pointer, push_token};
use crate::view::{self, Graph, Walker};

/// Checks the cross-field rules, building the graph on the way.
/// `lists` are the bundles' `deps` as written; they are dropped once
/// resolved. The views' check reads at most `view_work` ids.
pub(in crate::spec) fn check(
    spec: &Spec,
    lists: DepsLists,
    view_work: u64,
) -> Result<Graph, BadSpec> {
    budgets(spec)?;
    let order = view::id_order(spec);
    duplicate_id(spec, &order)?;
    duplicate_header(spec)?;
    backoff(spec)?;
    let graph = deps(spec, order, lists)?;
    view_size(spec, &graph, view_work)?;
    spool_in_sink(spec)?;
    path_style(spec)?;
    deadline_required(spec)?;
    unknown_input(spec)?;
    // B2: `template`, then `key(<rule>)`, go here, through
    // `unityrs_export::keys` (`BATCH_SPEC` 1.1's template and literal-key
    // rules; section 10's order).
    archive_key(spec)?;
    archive_kind(spec)?;
    source(spec)?;
    headers_without_url(spec)?;
    outputs_empty(spec)?;
    prefer_empty(spec)?;
    Ok(graph)
}

/// Graph indices in document order: `inputs` and `dependencies` each in
/// their own order, the one the document gives first first.
fn document_order(spec: &Spec) -> impl Iterator<Item = usize> {
    let n_in = spec.input_count();
    let n_dep = spec.dependency_count();
    let (first, second) = if spec.inputs_first() {
        (0..n_in, n_in..n_in + n_dep)
    } else {
        (n_in..n_in + n_dep, 0..n_in)
    };
    first.chain(second)
}

/// A bundle's place in document order.
fn document_position(spec: &Spec, i: usize) -> usize {
    let n_in = spec.input_count();
    match (spec.inputs_first(), i.checked_sub(n_in)) {
        (true, _) => i,
        (false, None) => spec.dependency_count() + i,
        (false, Some(d)) => d,
    }
}

fn bundle_field(spec: &Spec, i: usize, field: &str) -> String {
    let mut p = spec.bundle_pointer(i);
    push_token(&mut p, field);
    p
}

/// `M <= D` and `M + D <= I`.
fn budgets(spec: &Spec) -> Result<(), BadSpec> {
    let m = spec.max_decoded_bytes;
    let d = spec.dep_cache_bytes;
    let ok = m <= d
        && m.checked_add(d)
            .is_some_and(|s| s <= spec.max_inflight_uncompressed_bytes);
    if ok {
        Ok(())
    } else {
        Err(BadSpec::new(Rule::Budgets, "/max_decoded_bytes"))
    }
}

/// Every `id` unique across `inputs` and `dependencies`: the later of the
/// first repeated pair in document order is named.
fn duplicate_id(spec: &Spec, order: &[u32]) -> Result<(), BadSpec> {
    let id = |i: u32| spec.bundle(i as usize).map(|b| b.id());
    let pos = |i: usize| document_position(spec, i);
    // Each run of one id in `order`: its second occurrence in document
    // order is its first repeat. A run's two earliest are kept, not the
    // run, so a document of one id repeated holds nothing more.
    let mut worst: Option<usize> = None;
    let mut two: [Option<usize>; 2] = [None, None];
    let mut flush = |two: &mut [Option<usize>; 2]| {
        if let Some(second) = two[1]
            && worst.is_none_or(|w| pos(second) < pos(w))
        {
            worst = Some(second);
        }
        *two = [None, None];
    };
    let mut prev: Option<&str> = None;
    for &i in order {
        let cur = id(i);
        if cur != prev {
            flush(&mut two);
        }
        let i = i as usize;
        match two {
            [None, _] => two[0] = Some(i),
            [Some(a), _] if pos(i) < pos(a) => two = [Some(i), Some(a)],
            [Some(_), None] => two[1] = Some(i),
            [Some(_), Some(b)] if pos(i) < pos(b) => two[1] = Some(i),
            _ => {}
        }
        prev = cur;
    }
    flush(&mut two);
    match worst {
        Some(i) => Err(BadSpec::new(Rule::DuplicateId, bundle_field(spec, i, "id"))),
        None => Ok(()),
    }
}

/// No two header names of one bundle equal, ASCII case-insensitively: the
/// second is named.
fn duplicate_header(spec: &Spec) -> Result<(), BadSpec> {
    for i in document_order(spec) {
        let Some(b) = spec.bundle(i) else { continue };
        for (k, (name, _)) in b.headers().enumerate() {
            if b.headers()
                .take(k)
                .any(|(n, _)| n.eq_ignore_ascii_case(name))
            {
                let mut p = bundle_field(spec, i, "headers");
                push_token(&mut p, name);
                return Err(BadSpec::new(Rule::DuplicateHeader, p));
            }
        }
    }
    Ok(())
}

/// `http.backoff_max_ms >= http.backoff_initial_ms`.
fn backoff(spec: &Spec) -> Result<(), BadSpec> {
    if spec.http.backoff_max_ms >= spec.http.backoff_initial_ms {
        Ok(())
    } else {
        Err(BadSpec::new(Rule::Backoff, "/http/backoff_max_ms"))
    }
}

/// Every `deps` id names a bundle, and no list repeats an id or holds
/// more than `max_view_bundles`: the first such list in document order is
/// named. Builds the graph.
fn deps(spec: &Spec, order: Vec<u32>, written: DepsLists) -> Result<Graph, BadSpec> {
    let n = order.len();
    let max = spec.max_view_bundles as usize;
    let mut walker = Walker::new(n);
    let mut offsets: Vec<u32> = Vec::with_capacity(n + 1);
    // Room for every kept id, so the list never moves while it grows.
    let mut targets: Vec<u32> = Vec::with_capacity(written.ids.len());
    offsets.push(0);
    let mut bad: Option<usize> = None;
    // Built only when some list names an id.
    let index = (!written.ids.is_empty()).then(|| view::IdIndex::new(spec));
    for (i, list) in written.lists().enumerate() {
        let start = targets.len();
        let mut ok = list.len() <= max;
        if ok {
            for &id in written.ids.run(list) {
                let found = index.as_ref().and_then(|x| x.find(spec, spec.str(id)));
                let Some(t) = found else {
                    ok = false;
                    break;
                };
                targets.push(t);
            }
        }
        ok = ok
            && targets
                .get(start..)
                .is_some_and(|l| walker.first_repeat(l).is_none());
        if !ok {
            targets.truncate(start);
            if bad.is_none_or(|w| document_position(spec, i) < document_position(spec, w)) {
                bad = Some(i);
            }
        }
        offsets.push(u32::try_from(targets.len()).unwrap_or(u32::MAX));
    }
    drop((written, index));
    if let Some(i) = bad {
        return Err(BadSpec::new(Rule::Deps, bundle_field(spec, i, "deps")));
    }
    targets.shrink_to_fit();
    let inputs = u32::try_from(spec.input_count()).unwrap_or(u32::MAX);
    Ok(Graph::new(order, offsets, targets, inputs))
}

/// No input's view holds more than `max_view_bundles` bundles, and the
/// check reads at most `view_work` ids: the running sum, over the inputs
/// in document order, of the `deps` lengths of each member of the input's
/// view (`docs/BATCH_SPEC.md` 1.1). The first input whose view passes
/// either bound is named, and the walk stops there.
fn view_size(spec: &Spec, graph: &Graph, view_work: u64) -> Result<(), BadSpec> {
    let mut walker = Walker::new(graph.len());
    let mut work = view_work;
    for x in 0..graph.inputs() {
        if walker
            .view_within(graph, x, spec.max_view_bundles, &mut work)
            .is_err()
        {
            return Err(BadSpec::new(
                Rule::ViewSize,
                spec.bundle_pointer(x as usize),
            ));
        }
    }
    Ok(())
}

/// With a directory sink, `spool.dir` and `sink.root`, compared as paths
/// (by component, so a trailing `/` counts for nothing), neither equal
/// nor contain one another.
fn spool_in_sink(spec: &Spec) -> Result<(), BadSpec> {
    let Sink::Directory { root } = &spec.sink else {
        return Ok(());
    };
    let spool = Path::new(&spec.spool.dir);
    let root = Path::new(root);
    if spool.starts_with(root) || root.starts_with(spool) {
        Err(BadSpec::new(Rule::SpoolInSink, "/spool/dir"))
    } else {
        Ok(())
    }
}

/// An object store's IP-address endpoint, and a bucket with an uppercase
/// letter or `_`, need `path_style`.
fn path_style(spec: &Spec) -> Result<(), BadSpec> {
    let Sink::ObjectStore(s) = &spec.sink else {
        return Ok(());
    };
    if s.path_style {
        return Ok(());
    }
    if s.endpoint
        .as_deref()
        .is_some_and(formats::endpoint_host_is_ip)
    {
        return Err(BadSpec::new(Rule::PathStyle, "/sink/endpoint"));
    }
    if s.bucket
        .bytes()
        .any(|b| b.is_ascii_uppercase() || b == b'_')
    {
        return Err(BadSpec::new(Rule::PathStyle, "/sink/bucket"));
    }
    Ok(())
}

/// An object store sink needs a `deadline`.
fn deadline_required(spec: &Spec) -> Result<(), BadSpec> {
    if matches!(spec.sink, Sink::ObjectStore(_)) && spec.deadline.is_none() {
        Err(BadSpec::new(Rule::DeadlineRequired, "/deadline"))
    } else {
        Ok(())
    }
}

/// Every `prefer` entry's non-null `input` names an input.
fn unknown_input(spec: &Spec) -> Result<(), BadSpec> {
    // Built only when some entry names an input, over the inputs alone.
    let mut index: Option<view::IdIndex> = None;
    for (key, p) in spec.prefer() {
        let Some(id) = p.input else { continue };
        let inputs = index.get_or_insert_with(|| view::IdIndex::over(spec, spec.input_count()));
        if inputs.find(spec, id).is_none() {
            return Err(BadSpec::new(
                Rule::UnknownInput,
                pointer(&["prefer", key, "input"]),
            ));
        }
    }
    Ok(())
}

/// Whether `a` equals `b` or is its ancestor (`a` of `a/b.zip`).
fn covers(a: &str, b: &str) -> bool {
    b.strip_prefix(a)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// `archives`' keys unique, none an ancestor of another: the later of the
/// first such pair is named.
fn archive_key(spec: &Spec) -> Result<(), BadSpec> {
    for (j, a) in spec.archives.iter().enumerate() {
        let earlier = spec.archives.get(..j).unwrap_or_default();
        if earlier
            .iter()
            .any(|e| covers(&e.key, &a.key) || covers(&a.key, &e.key))
        {
            return Err(BadSpec::new(Rule::ArchiveKey, format!("/archives/{j}/key")));
        }
    }
    Ok(())
}

/// In one archive, each member `kind` once: the later member's is named.
fn archive_kind(spec: &Spec) -> Result<(), BadSpec> {
    for (j, a) in spec.archives.iter().enumerate() {
        for (k, m) in a.members.iter().enumerate() {
            let earlier = a.members.get(..k).unwrap_or_default();
            if earlier.iter().any(|e| e.kind == m.kind) {
                return Err(BadSpec::new(
                    Rule::ArchiveKind,
                    format!("/archives/{j}/members/{k}/kind"),
                ));
            }
        }
    }
    Ok(())
}

fn first_bundle(spec: &Spec, bad: impl Fn(&BundleRef) -> bool) -> Option<usize> {
    document_order(spec).find(|&i| spec.bundle(i).is_some_and(|b| bad(&b)))
}

/// Each bundle has exactly one of `path` and `url`.
fn source(spec: &Spec) -> Result<(), BadSpec> {
    match first_bundle(spec, |b| b.path().is_some() == b.url().is_some()) {
        Some(i) => Err(BadSpec::new(Rule::Source, spec.bundle_pointer(i))),
        None => Ok(()),
    }
}

/// `headers` only with `url`: `{}` too (C2: the member's presence).
fn headers_without_url(spec: &Spec) -> Result<(), BadSpec> {
    match first_bundle(spec, |b| b.has_headers() && b.url().is_none()) {
        Some(i) => Err(BadSpec::new(
            Rule::HeadersWithoutUrl,
            bundle_field(spec, i, "headers"),
        )),
        None => Ok(()),
    }
}

/// At least one of `outputs`' three kinds.
fn outputs_empty(spec: &Spec) -> Result<(), BadSpec> {
    let o = &spec.outputs;
    if o.textures.is_none() && o.sprites.is_none() && o.atlases.is_none() {
        Err(BadSpec::new(Rule::OutputsEmpty, "/outputs"))
    } else {
        Ok(())
    }
}

/// No Prefer with both fields null.
fn prefer_empty(spec: &Spec) -> Result<(), BadSpec> {
    match spec
        .prefer()
        .find(|(_, p)| p.input.is_none() && p.md5.is_none())
    {
        Some((key, _)) => Err(BadSpec::new(Rule::PreferEmpty, pointer(&["prefer", key]))),
        None => Ok(()),
    }
}
