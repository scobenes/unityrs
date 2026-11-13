//! The job spec, version 1 (`docs/BATCH_SPEC.md` section 1), and phase 1's
//! validation (`docs/plans/batch.md` 2.1, 2.11 step 2).
//!
//! - [`read::read_job`]: `JOB.json`'s bytes, by the document's own rules.
//! - [`parse`]: the bytes to a validated [`Validated`]: the typed
//!   [`model::Spec`] with its defaults and the [`crate::view::Graph`] of its
//!   `deps`, or the first [`BadSpec`] by section 10's order.
//! - [`load`]: both, the bytes dropped when the parse ends.
//! - [`fields`]: the parser's field table, which
//!   `docs/batch/spec-v1.schema.json` must equal (`tests/schema.rs`).
//!
//! Templates and literal keys stay raw strings in the spec; their rules
//! (`template`, `key(<rule>)`) are checked in section 10's order through
//! `unityrs_export::keys` (`validate::cross`).

pub mod fields;
pub mod model;
pub mod read;
pub mod store;

mod build;
mod formats;
pub(crate) mod node;
mod parse;
pub(crate) mod validate;

use std::fmt;
use std::path::Path;

use crate::reason::KeyRule;
use crate::view::Graph;

/// The highest `spec_version` this crate reads.
pub const SPEC_VERSION: u64 = 1;

/// `docs/batch/spec-v1.schema.json`, byte for byte: what `unityrs batch
/// --schema` prints. `tests/schema.rs` holds it to the parser's tables.
pub const SCHEMA: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../docs/batch/spec-v1.schema.json"
));

/// The largest `JOB.json`, in bytes (`docs/BATCH_SPEC.md` section 1).
pub const MAX_SPEC_BYTES: u64 = 64 << 20;

/// The most bytes of an `exit` line's `detail`, as written, JSON escaping
/// included (`docs/BATCH_SPEC.md` section 10).
pub const MAX_DETAIL_BYTES: usize = 1024;

/// A rule a spec breaks: `bad_spec`'s first word (`docs/BATCH_SPEC.md`
/// section 10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rule {
    /// `JOB.json` missing, or it cannot be opened or read.
    Unreadable,
    /// `JOB.json` is not a regular file.
    NotRegular,
    /// `JOB.json` is over 64 MiB.
    TooLarge,
    /// The bytes are not UTF-8.
    NotUtf8,
    /// The text is not one JSON value, or nests 128 levels deep or more.
    Syntax,
    /// A member repeated in one object.
    DuplicateMember,
    /// A member the object's table lacks.
    UnknownField,
    /// A required member absent.
    Missing,
    /// A value of the wrong JSON type.
    Type,
    /// An unknown `spec_version`.
    Version,
    /// A number, a length, a count, or a string outside its field's values.
    Range,
    /// A pattern, a URL's form, a date, a digest.
    Format,
    /// A header name 1.2 refuses.
    RefusedHeader,
    /// `M <= D` or `M + D <= I` broken.
    Budgets,
    /// An `id` repeated across `inputs` and `dependencies`.
    DuplicateId,
    /// Two names of one bundle's `headers` equal, ASCII case-insensitively.
    DuplicateHeader,
    /// `http.backoff_max_ms < http.backoff_initial_ms`.
    Backoff,
    /// A `deps` list naming no bundle, repeating an id, or too long.
    Deps,
    /// A view over `max_view_bundles`.
    ViewSize,
    /// The spool and a directory sink's root equal or nested.
    SpoolInSink,
    /// An object store's address endpoint or uppercase bucket without
    /// `path_style`.
    PathStyle,
    /// An object store sink with no `deadline`.
    DeadlineRequired,
    /// A `prefer` entry naming no input.
    UnknownInput,
    /// A template that does not parse for its kind, or whose literals break
    /// a key rule in every expansion (`docs/BATCH_SPEC.md` 2.1).
    Template,
    /// A literal key (`prefer`'s, an archive's, the sink's prefix) that
    /// breaks this rule of 2.3: `key(<rule>)`.
    Key(KeyRule),
    /// Two archive keys equal, or one an ancestor of the other.
    ArchiveKey,
    /// A member `kind` repeated in one archive.
    ArchiveKind,
    /// A bundle with both or neither of `path` and `url`.
    Source,
    /// `headers` on a bundle with no `url`.
    HeadersWithoutUrl,
    /// `outputs` with none of its three kinds.
    OutputsEmpty,
    /// A Prefer whose fields are both null.
    PreferEmpty,
    /// A bad command line: an unknown command or option, an argument
    /// missing or extra (section 10). The CLI's, never the parser's; its
    /// pointer is empty.
    Usage,
}

impl Rule {
    /// The rule's name in `bad_spec`'s detail.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Unreadable => "unreadable",
            Self::NotRegular => "not_regular",
            Self::TooLarge => "too_large",
            Self::NotUtf8 => "not_utf8",
            Self::Syntax => "syntax",
            Self::DuplicateMember => "duplicate_member",
            Self::UnknownField => "unknown_field",
            Self::Missing => "missing",
            Self::Type => "type",
            Self::Version => "version",
            Self::Range => "range",
            Self::Format => "format",
            Self::RefusedHeader => "refused_header",
            Self::Budgets => "budgets",
            Self::DuplicateId => "duplicate_id",
            Self::DuplicateHeader => "duplicate_header",
            Self::Backoff => "backoff",
            Self::Deps => "deps",
            Self::ViewSize => "view_size",
            Self::SpoolInSink => "spool_in_sink",
            Self::PathStyle => "path_style",
            Self::DeadlineRequired => "deadline_required",
            Self::UnknownInput => "unknown_input",
            Self::Template => "template",
            Self::Key(rule) => key_rule_name(rule),
            Self::ArchiveKey => "archive_key",
            Self::ArchiveKind => "archive_kind",
            Self::Source => "source",
            Self::HeadersWithoutUrl => "headers_without_url",
            Self::OutputsEmpty => "outputs_empty",
            Self::PreferEmpty => "prefer_empty",
            Self::Usage => "usage",
        }
    }
}

/// `key(<rule>)`'s name, for each rule a literal key can break (2.3's but
/// `prefix_conflict` and `archive_conflict`, which phase 6 decides).
fn key_rule_name(rule: KeyRule) -> &'static str {
    match rule {
        KeyRule::Empty => "key(empty)",
        KeyRule::NotUtf8 => "key(not_utf8)",
        KeyRule::ControlChar => "key(control_char)",
        KeyRule::LeadingSlash => "key(leading_slash)",
        KeyRule::TrailingSlash => "key(trailing_slash)",
        KeyRule::EmptySegment => "key(empty_segment)",
        KeyRule::DotSegment => "key(dot_segment)",
        KeyRule::SegmentTooLong => "key(segment_too_long)",
        KeyRule::TooLong => "key(too_long)",
        KeyRule::ReservedPrefix => "key(reserved_prefix)",
        KeyRule::ReservedName => "key(reserved_name)",
        KeyRule::PrefixConflict => "key(prefix_conflict)",
        KeyRule::ArchiveConflict => "key(archive_conflict)",
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// A refused spec: the rule it broke first and the RFC 6901 pointer of the
/// value refused (empty for the document itself; for `missing`, where the
/// absent member would be). Exit 78, reason `bad_spec`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadSpec {
    /// The rule.
    pub rule: Rule,
    /// The whole pointer, uncut.
    pub pointer: String,
}

impl BadSpec {
    /// A bad command line: `usage`, the rule alone.
    #[must_use]
    pub fn usage() -> Self {
        Self::new(Rule::Usage, "")
    }

    pub(crate) fn new(rule: Rule, pointer: impl Into<String>) -> Self {
        Self {
            rule,
            pointer: pointer.into(),
        }
    }

    /// The `exit` line's `detail`: `<rule> <pointer>`, or the rule alone
    /// for an empty pointer, at most [`MAX_DETAIL_BYTES`] once JSON-escaped
    /// as `serde_json` writes a string. Only the pointer is cut, at a code
    /// point boundary, and a cut pointer ends in `~`, which no whole one
    /// does (`docs/BATCH_SPEC.md` section 10).
    #[must_use]
    pub fn detail(&self) -> String {
        let rule = self.rule.name();
        if self.pointer.is_empty() {
            return rule.to_owned();
        }
        let head = rule.len() + 1;
        let whole: usize = self.pointer.chars().map(escaped_len).sum();
        if head + whole <= MAX_DETAIL_BYTES {
            return format!("{rule} {}", self.pointer);
        }
        // Room for the kept part and the closing `~`.
        let room = MAX_DETAIL_BYTES - head - 1;
        let mut used = 0;
        let mut end = 0;
        for (i, c) in self.pointer.char_indices() {
            let n = escaped_len(c);
            if used + n > room {
                break;
            }
            used += n;
            end = i + c.len_utf8();
        }
        let kept = self.pointer.get(..end).unwrap_or_default();
        format!("{rule} {kept}~")
    }
}

impl fmt::Display for BadSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.detail())
    }
}

impl std::error::Error for BadSpec {}

/// The bytes `c` takes in a JSON string as `serde_json` writes it: `"`
/// and `\` and the five short escapes take 2, any other control character
/// 6 (`\u00XX`), and every other character its UTF-8 bytes.
#[must_use]
pub fn escaped_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
        '\0'..='\u{1f}' => 6,
        _ => c.len_utf8(),
    }
}

/// Appends `/` and `name`, escaped as RFC 6901 says (`~` as `~0`, `/` as
/// `~1`), to `pointer`.
pub fn push_token(pointer: &mut String, name: &str) {
    pointer.push('/');
    for c in name.chars() {
        match c {
            '~' => pointer.push_str("~0"),
            '/' => pointer.push_str("~1"),
            _ => pointer.push(c),
        }
    }
}

/// A pointer built from tokens, each escaped.
#[must_use]
pub fn pointer(tokens: &[&str]) -> String {
    let mut p = String::new();
    for t in tokens {
        push_token(&mut p, t);
    }
    p
}

/// A validated spec: the typed spec and its dependency graph.
#[derive(Debug)]
pub struct Validated {
    /// SHA-256 of the exact buffer passed to parsing, before compaction.
    pub spec_sha256: [u8; 32],
    /// The spec, defaults filled in.
    pub spec: model::Spec,
    /// The `deps` graph, its id table in bytewise order.
    pub graph: Graph,
}

/// Validates `JOB.json`'s bytes: `not_utf8`, `syntax`, then every rule of
/// `docs/BATCH_SPEC.md` section 10's order but the document's first three
/// (`read::read_job`'s). The bytes
/// are the spec's string table while the rules run, then dropped: the
/// spec keeps a copy of only the strings it uses.
pub fn parse(bytes: Vec<u8>) -> Result<Validated, BadSpec> {
    parse_with(bytes, fields::VIEW_WORK)
}

/// [`parse`], the views' check reading at most `view_work` ids (the
/// tests' small graphs reach a lower budget's edge).
#[doc(hidden)]
pub fn parse_with(bytes: Vec<u8>, view_work: u64) -> Result<Validated, BadSpec> {
    use sha2::{Digest, Sha256};
    let spec_sha256 = Sha256::digest(&bytes).into();
    let text = String::from_utf8(bytes).map_err(|_| BadSpec::new(Rule::NotUtf8, ""))?;
    let (mut spec, lists, sites) = {
        let parsed = parse::parse(&text).map_err(|_| BadSpec::new(Rule::Syntax, ""))?;
        let Some(top) = parsed.top else {
            return Err(BadSpec::new(Rule::Type, ""));
        };
        parsed
            .version
            .map_err(|r| BadSpec::new(r, "/spec_version"))?;
        if let Some(e) = parsed.err {
            return Err(e);
        }
        build::spec(top, parsed.store)?
    };
    spec.strings = text;
    #[cfg(test)]
    tests::VIEW_WORK_USED.set(Some(view_work));
    let graph = validate::cross::check(&spec, lists, &sites, view_work)?;
    spec.compact();
    Ok(Validated {
        spec_sha256,
        spec,
        graph,
    })
}

/// Reads and validates `JOB.json` at `path` (`docs/plans/batch.md` 2.11
/// step 2): its bytes are dropped when the parse ends.
pub fn load(path: &Path) -> Result<Validated, BadSpec> {
    parse(read::read_job(path)?)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{BadSpec, MAX_DETAIL_BYTES, Rule, escaped_len, fields, pointer};

    thread_local! {
        /// The views' budget the last cross-field pass on this thread ran
        /// under.
        pub(super) static VIEW_WORK_USED: Cell<Option<u64>> = const { Cell::new(None) };
    }

    /// `parse` and `load` run the views' check under `VIEW_WORK` (review
    /// round 2's T3: the measurements reach it only when asked).
    #[test]
    fn parse_and_load_take_the_views_budget() {
        let text = r#"{"spec_version":1,"run_id":"r","attempt":1,"threads":1,
            "download_concurrency":1,"max_inflight_uncompressed_bytes":268435456,
            "max_decoded_bytes":16777216,"dep_cache_bytes":33554432,
            "spool":{"dir":"/spool","size_bytes":1073741824,"reserve_bytes":0},
            "inputs":[{"id":"a","path":"/in/a"}],
            "outputs":{"textures":{"key":"{input}/{name}.png"}},
            "sink":{"kind":"directory","root":"/out"}}"#;
        VIEW_WORK_USED.set(None);
        super::parse(text.as_bytes().to_vec()).expect("valid");
        assert_eq!(VIEW_WORK_USED.get(), Some(fields::VIEW_WORK));
        let dir = unityrs_testkit::scratch::Scratch::new("batch-view-work");
        let path = dir.join("JOB.json");
        std::fs::write(&path, text).expect("write");
        VIEW_WORK_USED.set(None);
        super::load(&path).expect("valid");
        assert_eq!(VIEW_WORK_USED.get(), Some(fields::VIEW_WORK));
    }

    /// `s`'s bytes as `serde_json` writes it in a string, the quotes left
    /// out, checked against [`escaped_len`].
    fn escaped(s: &str) -> usize {
        let written = serde_json::to_string(s).expect("a string serializes").len() - 2;
        assert_eq!(written, s.chars().map(escaped_len).sum::<usize>(), "{s:?}");
        written
    }

    #[test]
    fn a_pointer_escapes_its_tokens() {
        assert_eq!(pointer(&["a/b", "c~d", "0"]), "/a~1b/c~0d/0");
        assert_eq!(pointer(&[]), "");
    }

    #[test]
    fn a_detail_is_the_rule_alone_for_the_document() {
        assert_eq!(BadSpec::new(Rule::Syntax, "").detail(), "syntax");
        assert_eq!(
            BadSpec::new(Rule::Missing, "/run_id").detail(),
            "missing /run_id"
        );
    }

    #[test]
    fn a_long_pointer_is_cut_to_fit_and_ends_in_a_tilde() {
        for filler in [
            "a",
            "\u{e9}",
            "\"",
            "\\",
            "\n",
            "\u{1}",
            "\u{7f}",
            "\u{1f600}",
        ] {
            let p = format!("/prefer/{}", filler.repeat(2000));
            let d = BadSpec::new(Rule::UnknownInput, p.clone()).detail();
            assert!(escaped(&d) <= MAX_DETAIL_BYTES, "{filler:?}");
            assert!(escaped(&d) > MAX_DETAIL_BYTES - 7, "{filler:?}");
            assert!(d.ends_with('~'));
            let kept = &d["unknown_input ".len()..d.len() - 1];
            assert!(p.starts_with(kept));
        }
        // A pointer that just fits is whole.
        let p = format!("/{}", "a".repeat(MAX_DETAIL_BYTES - "range /".len()));
        let d = BadSpec::new(Rule::Range, p.clone()).detail();
        assert_eq!(d, format!("range {p}"));
        assert_eq!(d.len(), MAX_DETAIL_BYTES);
        let p = format!("{p}a");
        let d = BadSpec::new(Rule::Range, p).detail();
        assert_eq!(d.len(), MAX_DETAIL_BYTES);
        assert!(d.ends_with("a~"));
    }
}
