//! The typed spec, version 1, with its defaults filled in
//! (`docs/BATCH_SPEC.md` section 1). Built only from a document the
//! per-value pass accepted; the cross-field rules are checked on it.
//!
//! Templates and literal keys are raw strings until their rules (B2).
//!
//! The parts whose count no field caps (`inputs` and `dependencies`, their
//! `headers`, and `prefer`) are kept compact (`store`): each string is a
//! [`Seg`] of the spec's string table, read through [`BundleRef`] and
//! [`Spec::prefer`].

use std::num::NonZeroU64;

use super::store::{Chunks, Seg};

/// Where a parse-time [`Seg`] of a string starts when it is in the escape
/// table (a string the document escaped), not the document's text.
pub(crate) const ESCAPED: usize = 1 << 31;

/// A spec.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // The spec's own booleans.
pub struct Spec {
    /// Always [`super::SPEC_VERSION`] here.
    pub spec_version: u64,
    /// `run_id`.
    pub run_id: String,
    /// `attempt`.
    pub attempt: u32,
    /// `deadline`, its form checked; `None` for `null` or absent.
    pub deadline: Option<Deadline>,
    /// `shutdown_grace_ms`.
    pub shutdown_grace_ms: u32,
    /// `stop_on_stdin_eof`.
    pub stop_on_stdin_eof: bool,
    /// `parent_pid`.
    pub parent_pid: Option<u32>,
    /// `threads`.
    pub threads: u32,
    /// `download_concurrency`.
    pub download_concurrency: u32,
    /// `I`.
    pub max_inflight_uncompressed_bytes: u64,
    /// `M`.
    pub max_decoded_bytes: u64,
    /// `D`.
    pub dep_cache_bytes: u64,
    /// `max_view_bundles`.
    pub max_view_bundles: u32,
    /// `max_attempt_evictions`.
    pub max_attempt_evictions: u32,
    /// `max_export_work`, by default `2 x M` (final, 1.1).
    pub max_export_work: u64,
    /// `spool`.
    pub spool: Spool,
    /// `http`.
    pub http: Http,
    /// `inputs`, read through [`Self::bundle`].
    pub(crate) inputs: Chunks<Bundle>,
    /// `dependencies`.
    pub(crate) dependencies: Chunks<Bundle>,
    /// Whether `inputs` comes before `dependencies` in the document.
    pub(crate) inputs_first: bool,
    /// Every bundle's headers, each bundle's a run of them.
    pub(crate) headers: Chunks<Header>,
    /// `outputs`.
    pub outputs: Outputs,
    /// `prefer`'s entries, in document order.
    pub(crate) prefer: Chunks<PreferEntry>,
    /// `publish_incomplete`.
    pub publish_incomplete: bool,
    /// `resume`.
    pub resume: bool,
    /// `sink`.
    pub sink: Sink,
    /// `archives`.
    pub archives: Vec<Archive>,
    /// `manifest_fencing`.
    pub manifest_fencing: Fencing,
    /// The string table: the document's text while phase 1 runs, then only
    /// the strings the tables name (`compact`).
    pub(crate) strings: String,
    /// The strings the document escaped, while phase 1 runs.
    pub(crate) escaped: String,
}

impl Spec {
    /// The string `s` names.
    pub(crate) fn str(&self, s: Seg) -> &str {
        let r = s.range();
        let (table, r) = match r.start.checked_sub(ESCAPED) {
            Some(at) => (&self.escaped, at..at + s.len()),
            None => (&self.strings, r),
        };
        table.get(r).unwrap_or_default()
    }

    /// How many inputs.
    #[must_use]
    pub fn input_count(&self) -> usize {
        self.inputs.len()
    }

    /// How many dependencies.
    #[must_use]
    pub fn dependency_count(&self) -> usize {
        self.dependencies.len()
    }

    /// How many bundles: inputs and dependencies.
    #[must_use]
    pub fn bundle_count(&self) -> usize {
        self.inputs.len() + self.dependencies.len()
    }

    /// Whether `inputs` comes before `dependencies` in the document (each
    /// array is in document order; `duplicate_id` names the later of two in
    /// document order).
    #[must_use]
    pub fn inputs_first(&self) -> bool {
        self.inputs_first
    }

    /// Every bundle, `inputs` first: a bundle's index in the graph
    /// ([`crate::view::Graph`]) is its place here.
    pub fn bundles(&self) -> impl Iterator<Item = BundleRef<'_>> {
        self.inputs
            .iter()
            .chain(self.dependencies.iter())
            .map(|b| BundleRef { spec: self, b })
    }

    /// The bundle at graph index `i`.
    #[must_use]
    pub fn bundle(&self, i: usize) -> Option<BundleRef<'_>> {
        let b = match i.checked_sub(self.inputs.len()) {
            None => self.inputs.get(i),
            Some(d) => self.dependencies.get(d),
        }?;
        Some(BundleRef { spec: self, b })
    }

    /// `prefer`'s entries in document order: each output key and its
    /// Prefer.
    pub fn prefer(&self) -> impl Iterator<Item = (&str, Prefer<'_>)> {
        self.prefer.iter().map(|e| {
            let p = Prefer {
                input: e.input.map(|s| self.str(s)),
                md5: e.md5.map(|s| self.str(s)),
            };
            (self.str(e.key), p)
        })
    }

    /// Indexed preference accessor for phase 1's compact winner index.
    #[must_use]
    pub fn prefer_at(&self, index: u32) -> Option<(&str, Prefer<'_>)> {
        let e = self.prefer.get(usize::try_from(index).ok()?)?;
        Some((
            self.str(e.key),
            Prefer {
                input: e.input.map(|s| self.str(s)),
                md5: e.md5.map(|s| self.str(s)),
            },
        ))
    }

    /// How many `prefer` entries.
    #[must_use]
    pub fn prefer_count(&self) -> usize {
        self.prefer.len()
    }

    /// Moves every string the tables name into a string table of its own,
    /// dropping the document's text and the escape table, so a spec keeps
    /// only what it uses (`docs/plans/batch.md` 2.11 step 2: the bytes are
    /// dropped when the parse ends).
    pub(crate) fn compact(&mut self) {
        let mut total = 0;
        let mut add = |s: Seg| total += s.len();
        for b in self.inputs.iter().chain(self.dependencies.iter()) {
            b.each_string(&mut add);
        }
        for h in self.headers.iter() {
            add(h.name);
            add(h.value);
        }
        for e in self.prefer.iter() {
            e.each_string(&mut add);
        }
        let old = Old {
            strings: std::mem::take(&mut self.strings),
            escaped: std::mem::take(&mut self.escaped),
        };
        let mut table = String::with_capacity(total);
        let mut move_one = |s: &mut Seg| *s = old.copy(*s, &mut table);
        for b in self.inputs.iter_mut() {
            b.each_string_mut(&mut move_one);
        }
        for b in self.dependencies.iter_mut() {
            b.each_string_mut(&mut move_one);
        }
        for h in self.headers.iter_mut() {
            move_one(&mut h.name);
            move_one(&mut h.value);
        }
        for e in self.prefer.iter_mut() {
            e.each_string_mut(&mut move_one);
        }
        self.strings = table;
    }

    /// The JSON pointer of the bundle at graph index `i`.
    #[must_use]
    pub fn bundle_pointer(&self, i: usize) -> String {
        match i.checked_sub(self.inputs.len()) {
            None => format!("/inputs/{i}"),
            Some(d) => format!("/dependencies/{d}"),
        }
    }
}

/// An RFC 3339 date-time's fields, as written (its form only; the
/// conversion is preflight's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateTime {
    /// Four digits.
    pub year: u32,
    /// 1 to 12.
    pub month: u32,
    /// 1 to the month's last day.
    pub day: u32,
    /// 0 to 23.
    pub hour: u32,
    /// 0 to 59.
    pub minute: u32,
    /// 0 to 60 (a leap second).
    pub second: u32,
    /// The fraction's first nine digits, in nanoseconds.
    pub nanos: u32,
    /// The offset east of UTC, in minutes (`Z`: 0).
    pub offset_minutes: i32,
}

/// `deadline`: the text and its fields.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deadline {
    /// As the spec wrote it.
    pub text: String,
    /// Its fields.
    pub at: DateTime,
}

/// `spool` (1.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spool {
    /// `dir`.
    pub dir: String,
    /// `size_bytes`.
    pub size_bytes: u64,
    /// `reserve_bytes`.
    pub reserve_bytes: u64,
}

/// A try class of 1.4, as `retry_on` names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TryClass {
    /// `connect`.
    Connect,
    /// `no_response`.
    NoResponse,
    /// `timeout`.
    Timeout,
    /// `truncated`.
    Truncated,
    /// `status_5xx`.
    Status5xx,
    /// `status_429`.
    Status429,
}

impl TryClass {
    /// Every class, in 1.4's order.
    pub const ALL: [Self; 6] = [
        Self::Connect,
        Self::NoResponse,
        Self::Timeout,
        Self::Truncated,
        Self::Status5xx,
        Self::Status429,
    ];

    /// Its name in `retry_on`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::NoResponse => "no_response",
            Self::Timeout => "timeout",
            Self::Truncated => "truncated",
            Self::Status5xx => "status_5xx",
            Self::Status429 => "status_429",
        }
    }

    /// The class `name` names.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.name() == name)
    }

    fn bit(self) -> u8 {
        1 << (self as u8)
    }
}

/// `retry_on`: a set of try classes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RetryOn(u8);

impl RetryOn {
    /// All six, the default.
    pub const ALL: Self = Self(0b11_1111);
    /// None.
    pub const NONE: Self = Self(0);

    /// The set with `class` added.
    #[must_use]
    pub fn with(self, class: TryClass) -> Self {
        Self(self.0 | class.bit())
    }

    /// Whether `class` retries.
    #[must_use]
    pub fn contains(self, class: TryClass) -> bool {
        self.0 & class.bit() != 0
    }
}

/// `http.redirects`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Redirects {
    /// `"same_origin"`.
    SameOrigin,
    /// `"none"`.
    None,
}

/// `http` (1.4), defaults filled in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Http {
    /// `idle_timeout_ms`.
    pub idle_timeout_ms: u32,
    /// `min_rate_bytes`.
    pub min_rate_bytes: u32,
    /// `connect_timeout_ms`.
    pub connect_timeout_ms: u32,
    /// `attempts`.
    pub attempts: u32,
    /// `backoff_initial_ms`.
    pub backoff_initial_ms: u32,
    /// `backoff_max_ms`.
    pub backoff_max_ms: u32,
    /// `retry_on`.
    pub retry_on: RetryOn,
    /// `redirects`.
    pub redirects: Redirects,
}

impl Default for Http {
    fn default() -> Self {
        Self {
            idle_timeout_ms: 30_000,
            min_rate_bytes: 1024,
            connect_timeout_ms: 10_000,
            attempts: 3,
            backoff_initial_ms: 500,
            backoff_max_ms: 30_000,
            retry_on: RetryOn::ALL,
            redirects: Redirects::SameOrigin,
        }
    }
}

/// The text and escape table a [`Spec::compact`] moves strings out of.
struct Old {
    strings: String,
    escaped: String,
}

impl Old {
    /// Copies the string `s` names to the end of `table`: its new `Seg`.
    fn copy(&self, s: Seg, table: &mut String) -> Seg {
        let r = s.range();
        let text = match r.start.checked_sub(ESCAPED) {
            Some(at) => self.escaped.get(at..at + s.len()),
            None => self.strings.get(r),
        }
        .unwrap_or_default();
        let at = table.len();
        table.push_str(text);
        Seg::new(at, text.len()).unwrap_or(s)
    }
}

/// A header's value (1.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderValue<'a> {
    /// A literal string.
    Literal(&'a str),
    /// `{"env": NAME}`: read from the environment at preflight.
    Env(&'a str),
}

/// A header of a bundle: its name and its literal value or variable name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Header {
    pub(crate) name: Seg,
    pub(crate) value: Seg,
    /// Whether `value` names an environment variable.
    pub(crate) env: bool,
}

/// An item of `inputs` or `dependencies` (1.2), its strings in the spec's
/// string table: read it through [`BundleRef`]. Its `deps` are the
/// graph's ([`crate::view::Graph::deps`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bundle {
    pub(crate) id: Seg,
    pub(crate) path: Option<Seg>,
    pub(crate) url: Option<Seg>,
    pub(crate) sha256: Option<Seg>,
    /// `size + 1`.
    pub(crate) size1: Option<NonZeroU64>,
    /// Its run of the header table; `Some` whenever the member is present,
    /// `{}` too (`headers_without_url`).
    pub(crate) headers: Option<Seg>,
}

impl Bundle {
    fn each_string(&self, f: &mut impl FnMut(Seg)) {
        f(self.id);
        for s in [self.path, self.url, self.sha256].into_iter().flatten() {
            f(s);
        }
    }

    fn each_string_mut(&mut self, f: &mut impl FnMut(&mut Seg)) {
        f(&mut self.id);
        for s in [&mut self.path, &mut self.url, &mut self.sha256]
            .into_iter()
            .flatten()
        {
            f(s);
        }
    }
}

/// A bundle of a spec. Once validated, exactly one of `path` and `url` is
/// set (`source`), and `headers` is absent unless `url` is set
/// (`headers_without_url`).
#[derive(Clone, Copy, Debug)]
pub struct BundleRef<'a> {
    spec: &'a Spec,
    b: &'a Bundle,
}

impl<'a> BundleRef<'a> {
    /// `id`.
    #[must_use]
    pub fn id(&self) -> &'a str {
        self.spec.str(self.b.id)
    }

    /// `path`.
    #[must_use]
    pub fn path(&self) -> Option<&'a str> {
        self.b.path.map(|s| self.spec.str(s))
    }

    /// `url`, as written.
    #[must_use]
    pub fn url(&self) -> Option<&'a str> {
        self.b.url.map(|s| self.spec.str(s))
    }

    /// `sha256`, 64 lowercase hex digits.
    #[must_use]
    pub fn sha256(&self) -> Option<&'a str> {
        self.b.sha256.map(|s| self.spec.str(s))
    }

    /// `size`.
    #[must_use]
    pub fn size(&self) -> Option<u64> {
        self.b.size1.map(|s| s.get() - 1)
    }

    /// Whether the document gave `headers`, `{}` included.
    #[must_use]
    pub fn has_headers(&self) -> bool {
        self.b.headers.is_some()
    }

    /// `headers`, in document order.
    pub fn headers(&self) -> impl Iterator<Item = (&'a str, HeaderValue<'a>)> + use<'a> {
        let spec = self.spec;
        let run = self.b.headers.and_then(|s| Seg::new(s.start(), s.len()));
        run.into_iter().flat_map(move |r| {
            spec.headers.run(r).map(move |h| {
                let v = spec.str(h.value);
                let v = if h.env {
                    HeaderValue::Env(v)
                } else {
                    HeaderValue::Literal(v)
                };
                (spec.str(h.name), v)
            })
        })
    }
}

/// `outputs.atlases` (1.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Atlases {
    /// `sprite_key`, a raw template.
    pub sprite_key: String,
    /// `json_key`, a raw template.
    pub json_key: String,
    /// `script_classes`.
    pub script_classes: Vec<String>,
    /// `skip_name_suffixes`.
    pub skip_name_suffixes: Vec<String>,
}

/// `outputs` (1.5). Once validated, at least one is set
/// (`outputs_empty`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outputs {
    /// `textures.key`, a raw template.
    pub textures: Option<String>,
    /// `sprites.key`, a raw template.
    pub sprites: Option<String>,
    /// `atlases`.
    pub atlases: Option<Atlases>,
}

/// A Prefer (1.5). Once validated, not both `None` (`prefer_empty`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prefer<'a> {
    /// `input`: an input's id.
    pub input: Option<&'a str>,
    /// `md5`: 32 lowercase hex digits.
    pub md5: Option<&'a str>,
}

/// A `prefer` entry in the tables: its key and its Prefer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PreferEntry {
    pub(crate) key: Seg,
    pub(crate) input: Option<Seg>,
    pub(crate) md5: Option<Seg>,
}

impl PreferEntry {
    fn each_string(&self, f: &mut impl FnMut(Seg)) {
        f(self.key);
        for s in [self.input, self.md5].into_iter().flatten() {
            f(s);
        }
    }

    fn each_string_mut(&mut self, f: &mut impl FnMut(&mut Seg)) {
        f(&mut self.key);
        for s in [&mut self.input, &mut self.md5].into_iter().flatten() {
            f(s);
        }
    }
}

/// The sink (1.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Sink {
    /// `"directory"`.
    Directory {
        /// `root`.
        root: String,
    },
    /// `"object_store"`.
    ObjectStore(ObjectStore),
}

/// An object store sink's fields (1.6), defaults filled in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectStore {
    /// `bucket`.
    pub bucket: String,
    /// `region`.
    pub region: String,
    /// `prefix`, `""` or ending in `/`.
    pub prefix: String,
    /// `endpoint`, as written; `None`: AWS.
    pub endpoint: Option<String>,
    /// `path_style`.
    pub path_style: bool,
    /// `timeout_ms`.
    pub timeout_ms: u32,
    /// `attempts`.
    pub attempts: u32,
}

/// An archive member's kind (1.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MemberKind {
    /// `"texture"`.
    Texture,
    /// `"sprite"`.
    Sprite,
    /// `"atlas_sprite"`.
    AtlasSprite,
    /// `"atlas_json"`.
    AtlasJson,
}

impl MemberKind {
    /// The kind `name` names.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "texture" => Self::Texture,
            "sprite" => Self::Sprite,
            "atlas_sprite" => Self::AtlasSprite,
            "atlas_json" => Self::AtlasJson,
            _ => return None,
        })
    }
}

/// An archive member (1.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    /// `kind`.
    pub kind: MemberKind,
    /// `path`, a raw template.
    pub path: String,
}

/// An archive (1.7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Archive {
    /// `key`, a raw literal key.
    pub key: String,
    /// `members`.
    pub members: Vec<Member>,
}

/// `manifest_fencing`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fencing {
    /// `"conditional"`.
    Conditional,
    /// `"none"`.
    None,
}
