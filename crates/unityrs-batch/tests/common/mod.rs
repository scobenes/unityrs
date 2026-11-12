//! A valid spec to break one rule at a time, and the refusal's detail.
//!
//! Every fixture these helpers parse goes through a JSON Schema 2020-12
//! validator too, on `docs/batch/spec-v1.schema.json`, and the two must
//! agree (`docs/plans/extract.md` section 3, item 4): the parser accepts
//! what the validator accepts, and refuses by a per-value rule what it
//! refuses, but for the differences item 4 lists, each of which is checked
//! to hold where it applies ([`Difference`]). A cross-field rule is the
//! parser's alone, so the validator accepts what only one breaks.

#![allow(dead_code)]

use serde_json::{Value, json};
use unityrs_batch::spec::{self, BadSpec, Rule, Validated};

/// The schema, compiled.
struct Validator {
    schemas: boon::Schemas,
    index: boon::SchemaIndex,
}

impl Validator {
    fn new() -> Self {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/batch/spec-v1.schema.json"
        );
        let text = std::fs::read_to_string(path).expect("the schema");
        let schema: Value = serde_json::from_str(&text).expect("the schema is JSON");
        let id = schema["$id"].as_str().expect("an $id").to_owned();
        let mut schemas = boon::Schemas::new();
        let mut compiler = boon::Compiler::new();
        compiler
            .add_resource(&id, schema)
            .expect("the schema loads");
        let index = compiler
            .compile(&id, &mut schemas)
            .expect("the schema compiles");
        Self { schemas, index }
    }

    fn valid(&self, doc: &Value) -> bool {
        self.schemas.validate(doc, self.index).is_ok()
    }
}

thread_local! {
    static VALIDATOR: Validator = Validator::new();
}

/// Whether the schema holds `doc`.
pub fn schema_valid(doc: &Value) -> bool {
    VALIDATOR.with(|v| v.valid(doc))
}

/// Where the validator and the parser part by design
/// (`docs/plans/extract.md` section 3, item 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Difference {
    /// An integer field takes only an integer token; JSON Schema's
    /// `integer` holds any number whose value is one (`1.0`, `1e0`, `-0`).
    IntegerToken,
    /// `format` is an annotation in 2020-12: a date's or a URL's form, a
    /// URL's cap on its serialization and its host's cap as written are
    /// the parser's. (A URL's ASCII form and its cap as written are the
    /// schema's too: its `pattern` and `maxLength`.)
    Format,
    /// A byte cap is the parser's; the schema's `maxLength` counts code
    /// points, so it is looser past ASCII.
    ByteLength,
    /// `headers`' 64 KiB total of names and literal values.
    HeaderTotal,
    /// A repeated member, which `serde_json`'s `Value` drops: the
    /// validator never sees it.
    Unseen,
}

/// The cross-field rules (`docs/BATCH_SPEC.md` 1.1), the parser's alone.
fn cross_field(rule: Rule) -> bool {
    matches!(
        rule,
        Rule::Budgets
            | Rule::DuplicateId
            | Rule::DuplicateHeader
            | Rule::Backoff
            | Rule::Deps
            | Rule::ViewSize
            | Rule::SpoolInSink
            | Rule::PathStyle
            | Rule::DeadlineRequired
            | Rule::UnknownInput
            | Rule::ArchiveKey
            | Rule::ArchiveKind
            | Rule::Source
            | Rule::HeadersWithoutUrl
            | Rule::OutputsEmpty
            | Rule::PreferEmpty
    )
}

/// The difference, if any, that a refusal `e` of `doc` falls under.
pub fn difference(doc: &Value, e: &BadSpec) -> Option<Difference> {
    let at = doc.pointer(&e.pointer);
    let last = e.pointer.rsplit('/').next().unwrap_or_default();
    let integral =
        |v: &Value| v.as_f64().is_some_and(|f| f.fract() == 0.0) && !v.is_i64() && !v.is_u64();
    match e.rule {
        Rule::DuplicateMember => Some(Difference::Unseen),
        Rule::Type if at.is_some_and(integral) => Some(Difference::IntegerToken),
        Rule::Format | Rule::Range if last == "deadline" => Some(Difference::Format),
        Rule::Format | Rule::Range
            if matches!(last, "url" | "endpoint")
                && at.and_then(Value::as_str).is_some_and(|s| {
                    let cap = if last == "url" { 16 << 10 } else { 1 << 10 };
                    s.is_ascii() && s.len() <= cap
                }) =>
        {
            Some(Difference::Format)
        }
        Rule::Range if at.and_then(Value::as_str).is_some_and(|s| !s.is_ascii()) => {
            Some(Difference::ByteLength)
        }
        Rule::Range
            if last == "headers"
                && at.and_then(Value::as_object).is_some_and(|h| h.len() <= 32) =>
        {
            Some(Difference::HeaderTotal)
        }
        _ => None,
    }
}

/// Checks the validator's verdict on `text` against the parser's.
fn agree(text: &str, parsed: &Result<Validated, BadSpec>) {
    // Text `serde_json` cannot read (`syntax`) reaches no validator.
    let Ok(doc) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let valid = schema_valid(&doc);
    let Err(e) = parsed else {
        assert!(valid, "the parser accepts, the schema refuses: {text:.300}");
        return;
    };
    match difference(&doc, e) {
        Some(Difference::Unseen) => {}
        Some(Difference::IntegerToken) => {
            // The validator reads the value as its integer.
            let mut whole = doc.clone();
            if let Some(v) = whole.pointer_mut(&e.pointer) {
                let f = v.as_f64().unwrap_or_default();
                #[allow(clippy::cast_possible_truncation)]
                let i = f as i64;
                *v = json!(i);
            }
            assert_eq!(valid, schema_valid(&whole), "{e}: {text:.300}");
        }
        Some(Difference::HeaderTotal) => {
            // The validator's verdict does not hang on the total: with
            // every literal emptied it is the same (a member's own error,
            // which the total comes before, the schema still sees).
            let mut short = doc.clone();
            if let Some(Value::Object(h)) = short.pointer_mut(&e.pointer) {
                for v in h.values_mut().filter(|v| v.is_string()) {
                    *v = json!("");
                }
            }
            assert_eq!(valid, schema_valid(&short), "{e}: {text:.300}");
        }
        Some(d) => assert!(valid, "{e} ({d:?}) is the parser's alone: {text:.300}"),
        None if cross_field(e.rule) => {
            assert!(valid, "{e} is a cross-field rule: {text:.300}");
        }
        None => assert!(
            !valid,
            "the parser refuses {e}, the schema accepts: {text:.300}"
        ),
    }
}

/// A small valid spec. `serde_json`'s `Value` sorts members, so a test of
/// document order writes its text by hand.
pub fn base() -> Value {
    json!({
        "spec_version": 1,
        "run_id": "r1",
        "attempt": 1,
        "threads": 2,
        "download_concurrency": 2,
        "max_inflight_uncompressed_bytes": 268_435_456,
        "max_decoded_bytes": 16_777_216,
        "dep_cache_bytes": 33_554_432,
        "spool": {"dir": "/spool", "size_bytes": 1_073_741_824, "reserve_bytes": 0},
        "inputs": [{"id": "a", "path": "/in/a", "deps": ["d"]}],
        "dependencies": [{"id": "d", "url": "https://example.net/d"}],
        "outputs": {"textures": {"key": "{input}/{name}.png"}},
        "sink": {"kind": "directory", "root": "/out"},
    })
}

/// An object store sink, valid with [`base`] once a deadline is set.
pub fn store() -> Value {
    json!({"kind": "object_store", "bucket": "b-1", "region": "eu-west-1"})
}

/// Validates `text`, and checks the schema's verdict against it.
pub fn parse_text(text: &str) -> Result<Validated, spec::BadSpec> {
    let parsed = spec::parse(text.as_bytes().to_vec());
    agree(text, &parsed);
    parsed
}

/// The refusal's detail for `v`, or a panic when it is accepted.
pub fn refused(v: &Value) -> String {
    refused_text(&v.to_string())
}

/// The refusal's detail for `text`.
pub fn refused_text(text: &str) -> String {
    match parse_text(text) {
        Ok(_) => panic!("accepted: {text}"),
        Err(e) => e.detail(),
    }
}

/// `v` validated, or a panic naming its refusal.
pub fn accepted(v: &Value) -> Validated {
    match parse_text(&v.to_string()) {
        Ok(s) => s,
        Err(e) => panic!("refused: {e}"),
    }
}

/// `base()` with `f` applied.
pub fn with(f: impl FnOnce(&mut Value)) -> Value {
    let mut v = base();
    f(&mut v);
    v
}

/// Removes `key` from the object at `v`.
pub fn remove(v: &mut Value, key: &str) {
    v.as_object_mut().expect("an object").remove(key);
}
