//! The typed spec from the members the parse accepted, defaults filled
//! in (`docs/BATCH_SPEC.md` section 1). The bundles and `prefer` are
//! already in the parse's tables, which the spec takes whole.
//!
//! The parse has checked every member, so nothing here can fail on a
//! document; each read still names its rule and pointer if it meets what
//! the parse should have refused, rather than panic.

use std::fmt::Write as _;
use std::marker::PhantomData;

use super::formats;
use unityrs_export::ngui::Formats;

use super::model::{
    Archive, Atlases, Deadline, Fencing, Http, Image, Member, MemberKind, ObjectStore, Outputs,
    Redirects, RetryOn, Sink, Spec, Spool, TryClass,
};
use super::node::{Member as NodeMember, Node};
use super::parse::Store;
use super::store::{Chunks, Seg};
use super::{BadSpec, Rule, push_token};

/// The typed spec of the top level's `members` and the parse's `store`,
/// its string table still empty (the caller moves the document's text in),
/// the bundles' `deps` lists, which the cross-field pass resolves into the
/// graph and drops (a validated spec keeps its ids once), and the sites of
/// the templates and literal keys in document order.
pub(super) fn spec(
    members: Vec<NodeMember<'_>>,
    store: Store,
    spec_version: u64,
) -> Result<(Spec, DepsLists, Sites), BadSpec> {
    let mut b = Builder {
        ptr: String::new(),
        sites: Sites::default(),
        text: PhantomData,
    };
    let Store {
        inputs,
        dependencies,
        inputs_deps,
        dependencies_deps,
        ids,
        headers,
        prefer,
        escaped,
        inputs_first,
    } = store;
    let mut spec = b.top(members)?;
    spec.spec_version = spec_version;
    spec.inputs = inputs;
    spec.dependencies = dependencies;
    spec.inputs_first = inputs_first;
    spec.headers = headers;
    spec.prefer = prefer;
    spec.escaped = escaped;
    let lists = DepsLists {
        ids,
        inputs: inputs_deps,
        dependencies: dependencies_deps,
    };
    Ok((spec, lists, b.sites))
}

/// Where the spec's templates and literal keys stand, each list in
/// document order, for the cross-field rules `template` and `key(<rule>)`
/// (`docs/BATCH_SPEC.md` section 10: of two breaks of one rule, the first
/// in document order is named). Dropped with the cross-field pass.
#[derive(Debug, Default)]
pub(super) struct Sites {
    pub(super) templates: Vec<TemplateSite>,
    pub(super) keys: Vec<KeySite>,
}

/// A template's place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TemplateSite {
    /// `outputs.textures.key`.
    Textures,
    /// `outputs.sprites.key`.
    Sprites,
    /// `outputs.atlases.sprite_key`.
    SpriteKey,
    /// `outputs.atlases.json_key`.
    JsonKey,
    /// `archives[archive].members[member].path`.
    Member { archive: usize, member: usize },
}

/// A literal key's place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KeySite {
    /// `prefer`'s keys, each in its order.
    Prefer,
    /// `archives[i].key`.
    Archive(usize),
    /// The object store sink's `prefix`.
    Prefix,
}

/// Each bundle's `deps` as written: a run of `ids`, each a string of the
/// spec's table, for each input and then each dependency.
#[derive(Debug, Default)]
pub(super) struct DepsLists {
    pub(super) ids: Chunks<Seg>,
    pub(super) inputs: Chunks<Seg>,
    pub(super) dependencies: Chunks<Seg>,
}

impl DepsLists {
    /// Each bundle's list, in graph order.
    pub(super) fn lists(&self) -> impl Iterator<Item = Seg> + '_ {
        self.inputs.iter().chain(self.dependencies.iter()).copied()
    }
}

struct Builder<'a> {
    ptr: String,
    sites: Sites,
    text: PhantomData<&'a str>,
}

/// A required member not yet seen.
type Req<T> = Option<T>;

impl<'a> Builder<'a> {
    fn err(&self, rule: Rule) -> BadSpec {
        BadSpec::new(rule, self.ptr.clone())
    }

    fn under<T>(&mut self, name: &str, f: impl FnOnce(&mut Self) -> T) -> T {
        let len = self.ptr.len();
        push_token(&mut self.ptr, name);
        let r = f(self);
        self.ptr.truncate(len);
        r
    }

    fn under_index<T>(&mut self, i: usize, f: impl FnOnce(&mut Self) -> T) -> T {
        let len = self.ptr.len();
        let _ = write!(self.ptr, "/{i}");
        let r = f(self);
        self.ptr.truncate(len);
        r
    }

    fn need<T>(&mut self, v: Req<T>, name: &str) -> Result<T, BadSpec> {
        match v {
            Some(v) => Ok(v),
            None => Err(self.under(name, |b| b.err(Rule::Missing))),
        }
    }

    fn members(&self, node: Node<'a>) -> Result<Vec<NodeMember<'a>>, BadSpec> {
        match node {
            Node::Obj(m) => Ok(m.into_vec()),
            _ => Err(self.err(Rule::Type)),
        }
    }

    fn items(&self, node: Node<'a>) -> Result<Vec<Node<'a>>, BadSpec> {
        match node {
            Node::Arr(a) => Ok(a.into_vec()),
            _ => Err(self.err(Rule::Type)),
        }
    }

    fn u64(&self, node: &Node) -> Result<u64, BadSpec> {
        match node {
            Node::UInt(v) => Ok(*v),
            _ => Err(self.err(Rule::Type)),
        }
    }

    fn u32(&self, node: &Node) -> Result<u32, BadSpec> {
        u32::try_from(self.u64(node)?).map_err(|_| self.err(Rule::Range))
    }

    fn bool(&self, node: &Node) -> Result<bool, BadSpec> {
        match node {
            Node::Bool(v) => Ok(*v),
            _ => Err(self.err(Rule::Type)),
        }
    }

    fn string(&self, node: Node<'a>) -> Result<String, BadSpec> {
        match node {
            Node::Str(s) => Ok(s.into_owned()),
            _ => Err(self.err(Rule::Type)),
        }
    }

    fn opt_string(&self, node: Node<'a>) -> Result<Option<String>, BadSpec> {
        match node {
            Node::Null => Ok(None),
            n => self.string(n).map(Some),
        }
    }

    fn strings(&mut self, node: Node<'a>) -> Result<Vec<String>, BadSpec> {
        let items = self.items(node)?;
        let mut out = Vec::with_capacity(items.len());
        for (i, item) in items.into_iter().enumerate() {
            out.push(self.under_index(i, |b| b.string(item))?);
        }
        Ok(out)
    }

    #[allow(clippy::too_many_lines)]
    fn top(&mut self, members: Vec<NodeMember<'a>>) -> Result<Spec, BadSpec> {
        let (mut run_id, mut attempt, mut deadline) = (None, None, None);
        let (mut grace, mut stdin_eof, mut parent_pid) = (20_000, false, None);
        let (mut threads, mut concurrency) = (None, None);
        let (mut inflight, mut decoded, mut dep_cache) = (None, None, None);
        let (mut max_view, mut evictions, mut export_work) = (1024, 1024, None);
        let (mut spool, mut http) = (None, Http::default());
        let mut inputs = None;
        let mut outputs = None;
        let (mut publish_incomplete, mut resume) = (true, false);
        let (mut sink, mut archives, mut fencing) = (None, Vec::new(), Fencing::Conditional);
        let mut unitycn_key = None;
        for (name, value) in members {
            self.under(&name, |b| -> Result<(), BadSpec> {
                match &*name {
                    "run_id" => run_id = Some(b.string(value)?),
                    "attempt" => attempt = Some(b.u32(&value)?),
                    "deadline" => deadline = b.deadline(value)?,
                    "shutdown_grace_ms" => grace = b.u32(&value)?,
                    "stop_on_stdin_eof" => stdin_eof = b.bool(&value)?,
                    "parent_pid" => {
                        parent_pid = match value {
                            Node::Null => None,
                            v => Some(b.u32(&v)?),
                        };
                    }
                    "threads" => threads = Some(b.u32(&value)?),
                    "download_concurrency" => concurrency = Some(b.u32(&value)?),
                    "max_inflight_uncompressed_bytes" => inflight = Some(b.u64(&value)?),
                    "max_decoded_bytes" => decoded = Some(b.u64(&value)?),
                    "dep_cache_bytes" => dep_cache = Some(b.u64(&value)?),
                    "max_view_bundles" => max_view = b.u32(&value)?,
                    "max_attempt_evictions" => evictions = b.u32(&value)?,
                    "max_export_work" => export_work = Some(b.u64(&value)?),
                    "spool" => spool = Some(b.spool(value)?),
                    "http" => http = b.http(value)?,
                    "inputs" => inputs = Some(()),
                    "dependencies" => {}
                    "unitycn_key" => unitycn_key = b.env(value)?,
                    "prefer" => b.sites.keys.push(KeySite::Prefer),
                    "outputs" => outputs = Some(b.outputs(value)?),
                    "publish_incomplete" => publish_incomplete = b.bool(&value)?,
                    "resume" => resume = b.bool(&value)?,
                    "sink" => sink = Some(b.sink(value)?),
                    "archives" => archives = b.archives(value)?,
                    "manifest_fencing" => {
                        fencing = match &*b.string(value)? {
                            "none" => Fencing::None,
                            _ => Fencing::Conditional,
                        };
                    }
                    _ => return Err(b.err(Rule::UnknownField)),
                }
                Ok(())
            })?;
        }
        let max_decoded_bytes = self.need(decoded, "max_decoded_bytes")?;
        Ok(Spec {
            // The caller sets the version the document was read by.
            spec_version: 1,
            run_id: self.need(run_id, "run_id")?,
            attempt: self.need(attempt, "attempt")?,
            deadline,
            shutdown_grace_ms: grace,
            stop_on_stdin_eof: stdin_eof,
            parent_pid,
            threads: self.need(threads, "threads")?,
            download_concurrency: self.need(concurrency, "download_concurrency")?,
            max_inflight_uncompressed_bytes: self
                .need(inflight, "max_inflight_uncompressed_bytes")?,
            max_decoded_bytes,
            dep_cache_bytes: self.need(dep_cache, "dep_cache_bytes")?,
            max_view_bundles: max_view,
            max_attempt_evictions: evictions,
            // 2 x M, M at most 2^40.
            max_export_work: export_work.unwrap_or(max_decoded_bytes.saturating_mul(2)),
            spool: self.need(spool, "spool")?,
            http,
            inputs: {
                self.need(inputs, "inputs")?;
                Chunks::new()
            },
            dependencies: Chunks::new(),
            inputs_first: true,
            headers: Chunks::new(),
            outputs: self.need(outputs, "outputs")?,
            prefer: Chunks::new(),
            publish_incomplete,
            resume,
            sink: self.need(sink, "sink")?,
            archives,
            manifest_fencing: fencing,
            unitycn_key,
            strings: String::new(),
            escaped: String::new(),
        })
    }

    fn deadline(&self, node: Node<'a>) -> Result<Option<Deadline>, BadSpec> {
        let Some(text) = self.opt_string(node)? else {
            return Ok(None);
        };
        match formats::date_time(&text) {
            Some(at) => Ok(Some(Deadline { text, at })),
            None => Err(self.err(Rule::Format)),
        }
    }

    fn spool(&mut self, node: Node<'a>) -> Result<Spool, BadSpec> {
        let (mut dir, mut size, mut reserve) = (None, None, None);
        for (name, value) in self.members(node)? {
            self.under(&name, |b| -> Result<(), BadSpec> {
                match &*name {
                    "dir" => dir = Some(b.string(value)?),
                    "size_bytes" => size = Some(b.u64(&value)?),
                    "reserve_bytes" => reserve = Some(b.u64(&value)?),
                    _ => return Err(b.err(Rule::UnknownField)),
                }
                Ok(())
            })?;
        }
        Ok(Spool {
            dir: self.need(dir, "dir")?,
            size_bytes: self.need(size, "size_bytes")?,
            reserve_bytes: self.need(reserve, "reserve_bytes")?,
        })
    }

    fn http(&mut self, node: Node<'a>) -> Result<Http, BadSpec> {
        let mut h = Http::default();
        for (name, value) in self.members(node)? {
            self.under(&name, |b| -> Result<(), BadSpec> {
                match &*name {
                    "idle_timeout_ms" => h.idle_timeout_ms = b.u32(&value)?,
                    "min_rate_bytes" => h.min_rate_bytes = b.u32(&value)?,
                    "connect_timeout_ms" => h.connect_timeout_ms = b.u32(&value)?,
                    "attempts" => h.attempts = b.u32(&value)?,
                    "backoff_initial_ms" => h.backoff_initial_ms = b.u32(&value)?,
                    "backoff_max_ms" => h.backoff_max_ms = b.u32(&value)?,
                    "retry_on" => {
                        let mut set = RetryOn::NONE;
                        for name in b.strings(value)? {
                            let c = TryClass::from_name(&name).ok_or_else(|| b.err(Rule::Range))?;
                            set = set.with(c);
                        }
                        h.retry_on = set;
                    }
                    "redirects" => {
                        h.redirects = match &*b.string(value)? {
                            "none" => Redirects::None,
                            _ => Redirects::SameOrigin,
                        };
                    }
                    _ => return Err(b.err(Rule::UnknownField)),
                }
                Ok(())
            })?;
        }
        Ok(h)
    }

    fn template_object(&mut self, node: Node<'a>) -> Result<String, BadSpec> {
        let mut key = None;
        for (name, value) in self.members(node)? {
            self.under(&name, |b| match &*name {
                "key" => b.string(value).map(|s| key = Some(s)),
                _ => Err(b.err(Rule::UnknownField)),
            })?;
        }
        self.need(key, "key")
    }

    /// A nullable `{"env": NAME}`: the variable's name.
    fn env(&mut self, node: Node<'a>) -> Result<Option<String>, BadSpec> {
        if matches!(node, Node::Null) {
            return Ok(None);
        }
        let mut name = None;
        for (field, value) in self.members(node)? {
            self.under(&field, |b| match &*field {
                "env" => b.string(value).map(|s| name = Some(s)),
                _ => Err(b.err(Rule::UnknownField)),
            })?;
        }
        self.need(name, "env").map(Some)
    }

    /// `outputs.formats`' names, each a format this binary decodes.
    fn formats(&mut self, node: Node<'a>) -> Result<Formats, BadSpec> {
        let mut set = Formats::NONE;
        for (i, name) in self.strings(node)?.into_iter().enumerate() {
            let format =
                Formats::decoded(&name).map_err(|_| self.under_index(i, |b| b.err(Rule::Range)))?;
            set = set.with(format);
        }
        Ok(set)
    }

    fn outputs(&mut self, node: Node<'a>) -> Result<Outputs, BadSpec> {
        let mut out = Outputs {
            textures: None,
            sprites: None,
            atlases: None,
            formats: Formats::V1,
            image: Image::Png,
        };
        for (name, value) in self.members(node)? {
            self.under(&name, |b| -> Result<(), BadSpec> {
                match &*name {
                    "textures" => {
                        b.sites.templates.push(TemplateSite::Textures);
                        out.textures = Some(b.template_object(value)?);
                    }
                    "sprites" => {
                        b.sites.templates.push(TemplateSite::Sprites);
                        out.sprites = Some(b.template_object(value)?);
                    }
                    "atlases" => out.atlases = Some(b.atlases(value)?),
                    "formats" => out.formats = b.formats(value)?,
                    "image" => {
                        out.image = match &*b.string(value)? {
                            "png" => Image::Png,
                            _ => return Err(b.err(Rule::Range)),
                        };
                    }
                    _ => return Err(b.err(Rule::UnknownField)),
                }
                Ok(())
            })?;
        }
        Ok(out)
    }

    fn atlases(&mut self, node: Node<'a>) -> Result<Atlases, BadSpec> {
        let (mut sprite_key, mut json_key) = (None, None);
        let mut script_classes = vec!["NGUIAtlas".to_owned()];
        let mut skip_name_suffixes = Vec::new();
        for (name, value) in self.members(node)? {
            self.under(&name, |b| -> Result<(), BadSpec> {
                match &*name {
                    "sprite_key" => {
                        b.sites.templates.push(TemplateSite::SpriteKey);
                        sprite_key = Some(b.string(value)?);
                    }
                    "json_key" => {
                        b.sites.templates.push(TemplateSite::JsonKey);
                        json_key = Some(b.string(value)?);
                    }
                    "script_classes" => script_classes = b.strings(value)?,
                    "skip_name_suffixes" => skip_name_suffixes = b.strings(value)?,
                    _ => return Err(b.err(Rule::UnknownField)),
                }
                Ok(())
            })?;
        }
        Ok(Atlases {
            sprite_key: self.need(sprite_key, "sprite_key")?,
            json_key: self.need(json_key, "json_key")?,
            script_classes,
            skip_name_suffixes,
        })
    }

    fn sink(&mut self, node: Node<'a>) -> Result<Sink, BadSpec> {
        let mut kind = None;
        let mut root = None;
        let (mut bucket, mut region) = (None, None);
        let mut store = ObjectStore {
            bucket: String::new(),
            region: String::new(),
            prefix: String::new(),
            endpoint: None,
            path_style: false,
            timeout_ms: 30_000,
            attempts: 3,
        };
        for (name, value) in self.members(node)? {
            self.under(&name, |b| -> Result<(), BadSpec> {
                match &*name {
                    "kind" => kind = Some(b.string(value)?),
                    "root" => root = Some(b.string(value)?),
                    "bucket" => bucket = Some(b.string(value)?),
                    "region" => region = Some(b.string(value)?),
                    "prefix" => {
                        b.sites.keys.push(KeySite::Prefix);
                        store.prefix = b.string(value)?;
                    }
                    "endpoint" => store.endpoint = b.opt_string(value)?,
                    "path_style" => store.path_style = b.bool(&value)?,
                    "timeout_ms" => store.timeout_ms = b.u32(&value)?,
                    "attempts" => store.attempts = b.u32(&value)?,
                    _ => return Err(b.err(Rule::UnknownField)),
                }
                Ok(())
            })?;
        }
        if self.need(kind, "kind")? == "directory" {
            Ok(Sink::Directory {
                root: self.need(root, "root")?,
            })
        } else {
            store.bucket = self.need(bucket, "bucket")?;
            store.region = self.need(region, "region")?;
            Ok(Sink::ObjectStore(store))
        }
    }

    fn archives(&mut self, node: Node<'a>) -> Result<Vec<Archive>, BadSpec> {
        let items = self.items(node)?;
        let mut out = Vec::with_capacity(items.len());
        for (i, item) in items.into_iter().enumerate() {
            out.push(self.under_index(i, |b| b.archive(i, item))?);
        }
        Ok(out)
    }

    fn archive(&mut self, archive: usize, node: Node<'a>) -> Result<Archive, BadSpec> {
        let (mut key, mut members) = (None, None);
        for (name, value) in self.members(node)? {
            self.under(&name, |b| -> Result<(), BadSpec> {
                match &*name {
                    "key" => {
                        b.sites.keys.push(KeySite::Archive(archive));
                        key = Some(b.string(value)?);
                    }
                    "members" => {
                        let items = b.items(value)?;
                        let mut ms = Vec::with_capacity(items.len());
                        for (member, item) in items.into_iter().enumerate() {
                            b.sites
                                .templates
                                .push(TemplateSite::Member { archive, member });
                            ms.push(b.under_index(member, |b| b.member(item))?);
                        }
                        members = Some(ms);
                    }
                    _ => return Err(b.err(Rule::UnknownField)),
                }
                Ok(())
            })?;
        }
        Ok(Archive {
            key: self.need(key, "key")?,
            members: self.need(members, "members")?,
        })
    }

    fn member(&mut self, node: Node<'a>) -> Result<Member, BadSpec> {
        let (mut kind, mut path) = (None, None);
        for (name, value) in self.members(node)? {
            self.under(&name, |b| -> Result<(), BadSpec> {
                match &*name {
                    "kind" => {
                        let s = b.string(value)?;
                        kind = Some(MemberKind::from_name(&s).ok_or_else(|| b.err(Rule::Range))?);
                    }
                    "path" => path = Some(b.string(value)?),
                    _ => return Err(b.err(Rule::UnknownField)),
                }
                Ok(())
            })?;
        }
        Ok(Member {
            kind: self.need(kind, "kind")?,
            path: self.need(path, "path")?,
        })
    }
}
