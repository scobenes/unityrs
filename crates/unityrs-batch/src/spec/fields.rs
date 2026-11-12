//! The parser's field table: every object of version 1 with its members,
//! types, presence, defaults, ranges, closed values and formats, as
//! `docs/BATCH_SPEC.md` section 1's tables give them.
//!
//! Validation's per-value pass (`validate`) walks the document against
//! this table, and `tests/schema.rs` holds `docs/batch/spec-v1.schema.json`
//! equal to it: each object's properties, required members and
//! `additionalProperties: false`, each value's bounds, closed values and
//! patterns. A length here is in bytes; the schema states it as a
//! code-point bound, which is looser (section 1). What the schema cannot
//! say (the byte caps, the header rules, a URL's serialized length, the
//! cross-field rules) is the parser's alone.

/// The largest integer any field takes, 2^53 - 1 (`BATCH_SPEC`'s
/// conventions).
pub const MAX_INT: u64 = (1 << 53) - 1;

const MIB: u64 = 1 << 20;

/// One member of an object.
#[derive(Debug)]
pub struct Field {
    /// Its name.
    pub name: &'static str,
    /// Its type and constraints.
    pub ty: Ty,
    /// Whether an absent member is `missing`.
    pub required: bool,
    /// Whether `null` is allowed (meaning the field's null value).
    pub nullable: bool,
    /// Its default as JSON text, where section 1 states one as a value.
    pub default: Option<&'static str>,
}

/// A value's type.
#[derive(Debug)]
pub enum Ty {
    /// `true` or `false`.
    Bool,
    /// An integer token in `min..=max`.
    Int {
        /// The least value.
        min: u64,
        /// The greatest value.
        max: u64,
    },
    /// A string.
    Str(Str),
    /// An object with exactly these members.
    Object(&'static [Field]),
    /// An array.
    Array(&'static Array),
    /// `inputs` (`true`) or `dependencies`: an array of [`BUNDLE`] objects,
    /// which the parse keeps in the spec's compact tables (`store`).
    Bundles(&'static Array, bool),
    /// A bundle's `deps`: an array of strings of any count, kept in phase
    /// 1's id table (`store`).
    Deps,
    /// `prefer`: output key to a [`PREFER`] object.
    Prefer,
    /// A bundle's `headers`: name to a literal string or an [`ENV`] object
    /// (`docs/BATCH_SPEC.md` 1.2).
    Headers,
    /// The sink: [`DIRECTORY`] or [`OBJECT_STORE`], by its `kind`.
    Sink,
}

/// A string's constraints.
#[derive(Debug)]
pub struct Str {
    /// The fewest bytes.
    pub min: u64,
    /// The most bytes, if capped.
    pub max: Option<u64>,
    /// The closed set of values, if any (outside it: `range`).
    pub values: &'static [&'static str],
    /// The form it must have (outside it: `format`).
    pub format: Option<Format>,
}

/// An array's constraints.
#[derive(Debug)]
pub struct Array {
    /// Its items' type.
    pub item: Ty,
    /// The fewest items.
    pub min: u64,
    /// The most items, if capped.
    pub max: Option<u64>,
    /// Whether an item may not repeat (a repeat: `range`).
    pub unique: bool,
}

/// A string's form, checked as `format`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// `run_id`: `[A-Za-z0-9._-]`, not `.` or `..`.
    RunId,
    /// An RFC 3339 date-time with `Z` or an offset.
    DateTime,
    /// An absolute path: a leading `/`, no NUL.
    AbsPath,
    /// A bundle's `url`: `http` or `https`, no userinfo.
    Url,
    /// A sink's `endpoint`: `http` or `https`, no userinfo, query or
    /// fragment, its path empty or `/`.
    Endpoint,
    /// 64 lowercase hex digits.
    Sha256,
    /// 32 lowercase hex digits.
    Md5,
    /// ASCII letters, digits, `.`, `-` and `_`.
    Bucket,
    /// ASCII letters, digits, `-` and `_`.
    Region,
    /// Empty, or ending in `/`.
    Prefix,
    /// No control character: below U+0020, U+007F (2.3's `control_char`)
    /// or U+0080 to U+009F.
    Id,
    /// An environment variable's name: `[A-Za-z_][A-Za-z0-9_]*`.
    EnvName,
}

impl Format {
    /// The schema's `pattern` for this format, where a regular expression
    /// states it (the URL forms and the date are the parser's alone; the
    /// schema gives the date as `format: date-time`).
    #[must_use]
    pub fn pattern(self) -> Option<&'static str> {
        Some(match self {
            // Not `.` or `..`, without a lookahead, so every engine reads
            // it as ECMA-262 does.
            Self::RunId => {
                r"^(?:[A-Za-z0-9_-][A-Za-z0-9._-]*|\.[A-Za-z0-9_-][A-Za-z0-9._-]*|\.\.[A-Za-z0-9._-]+)?$"
            }
            Self::AbsPath => r"^/[^\u0000]*$",
            Self::Sha256 => "^[0-9a-f]{64}$",
            Self::Md5 => "^[0-9a-f]{32}$",
            Self::Bucket => "^[A-Za-z0-9._-]*$",
            Self::Region => "^[A-Za-z0-9_-]*$",
            // ECMA-262's `.` stops at a line terminator; `[\s\S]` does not.
            Self::Prefix => r"^(?:[\s\S]*/)?$",
            Self::Id => r"^[^\u0000-\u001f\u007f-\u009f]*$",
            Self::EnvName => "^[A-Za-z_][A-Za-z0-9_]*$",
            // ASCII; the URL's form is the parser's.
            Self::Url | Self::Endpoint => r"^[\u0000-\u007f]*$",
            Self::DateTime => return None,
        })
    }

    /// A URL form's cap as written, which the schema states as its
    /// `maxLength` (ASCII, so code points are bytes).
    #[must_use]
    pub fn written_cap(self) -> Option<u64> {
        match self {
            Self::Url => Some(MAX_URL),
            Self::Endpoint => Some(MAX_ENDPOINT),
            _ => None,
        }
    }
}

const fn field(name: &'static str, ty: Ty) -> Field {
    Field {
        name,
        ty,
        required: false,
        nullable: false,
        default: None,
    }
}

const fn required(name: &'static str, ty: Ty) -> Field {
    Field {
        name,
        ty,
        required: true,
        nullable: false,
        default: None,
    }
}

const fn with_default(name: &'static str, ty: Ty, default: &'static str) -> Field {
    Field {
        name,
        ty,
        required: false,
        nullable: false,
        default: Some(default),
    }
}

const fn nullable(name: &'static str, ty: Ty, default: Option<&'static str>) -> Field {
    Field {
        name,
        ty,
        required: false,
        nullable: true,
        default,
    }
}

const fn int(min: u64, max: u64) -> Ty {
    Ty::Int { min, max }
}

const fn string(min: u64, max: Option<u64>, format: Option<Format>) -> Ty {
    Ty::Str(Str {
        min,
        max,
        values: &[],
        format,
    })
}

const fn one_of(values: &'static [&'static str]) -> Ty {
    Ty::Str(Str {
        min: 0,
        max: None,
        values,
        format: None,
    })
}

/// A template: a string, kept raw until its rules (B2's `template`).
const TEMPLATE: Ty = string(0, None, None);

/// The most headers one bundle sends.
pub const MAX_HEADERS: u64 = 32;
/// The most bytes of one bundle's header names and literal values.
pub const MAX_HEADER_BYTES: u64 = 64 << 10;
/// The most bytes of one header's literal value.
pub const MAX_HEADER_VALUE: u64 = 16 << 10;
/// The most bytes of a header's environment variable name.
pub const MAX_ENV_NAME: u64 = 256;
/// The header names a bundle may not send, compared ASCII
/// case-insensitively (`refused_header`).
pub const REFUSED_HEADERS: &[&str] = &[
    "Host",
    "Content-Length",
    "Transfer-Encoding",
    "Connection",
    "Range",
    "Accept-Encoding",
];
/// The schema's `pattern` for a header's name: an HTTP token.
pub const TOKEN_PATTERN: &str = "^[!#$%&'*+.^_`|~0-9A-Za-z-]+$";
/// The schema's `pattern` for a header's literal value: tab, space,
/// visible ASCII and any non-ASCII character (its bytes 0x80-0xFF).
pub const HEADER_VALUE_PATTERN: &str = r"^[^\u0000-\u0008\u000a-\u001f\u007f]*$";
/// The most bytes of a bundle's `url`, as the `url` crate serializes it.
pub const MAX_URL: u64 = 16 << 10;
/// The most bytes of a sink's `endpoint`, as the `url` crate serializes it.
pub const MAX_ENDPOINT: u64 = 1 << 10;
/// The most bytes of a `url`'s or `endpoint`'s host as written (1.2).
pub const MAX_HOST: u64 = 253;
/// `max_view_bundles`'s greatest value.
pub const MAX_VIEW_BUNDLES: u64 = 65_536;
/// The most ids the views' check reads (`docs/BATCH_SPEC.md` 1.1): the sum,
/// over the inputs in document order, of the `deps` lengths of each member
/// of the input's view. Past it, the input is `view_size`. Each id read
/// is a load from the bundles' tables at a place the graph chooses, about
/// 250 ns where the walk visits millions of id-only bundles in a
/// pseudo-random order, so the budget bounds phase 1's time: at 2^24 the
/// slowest shape measured validates in about 6 s (`batch.md` section 8).
pub const VIEW_WORK: u64 = 1 << 24;
/// The most items of `script_classes` and of `skip_name_suffixes` (1.5).
pub const MAX_ATLAS_NAMES: u64 = 256;
/// The most bytes of one of their items (1.5).
pub const MAX_ATLAS_NAME: u64 = 1024;
/// The sink kinds.
pub const SINK_KINDS: &[&str] = &["directory", "object_store"];

/// A header's `{"env": NAME}` value.
pub static ENV: [Field; 1] = [required(
    "env",
    string(1, Some(MAX_ENV_NAME), Some(Format::EnvName)),
)];

/// An item of `inputs` or `dependencies` (1.2).
pub static BUNDLE: [Field; 7] = [
    required("id", string(1, Some(1024), Some(Format::Id))),
    field("path", string(0, None, Some(Format::AbsPath))),
    // Its caps, `MAX_URL` as written and serialized and `MAX_HOST`, are
    // checked after its ASCII form (`formats`).
    field("url", string(0, None, Some(Format::Url))),
    with_default("headers", Ty::Headers, "{}"),
    field("size", int(0, MAX_INT)),
    field("sha256", string(0, None, Some(Format::Sha256))),
    with_default("deps", Ty::Deps, "[]"),
];

static BUNDLES: Array = Array {
    item: Ty::Object(&BUNDLE),
    min: 0,
    max: None,
    unique: false,
};

static INPUTS: Array = Array {
    item: Ty::Object(&BUNDLE),
    min: 1,
    max: Some(100_000),
    unique: false,
};

/// `spool` (1.3).
pub static SPOOL: [Field; 3] = [
    required("dir", string(0, Some(256), Some(Format::AbsPath))),
    required("size_bytes", int(MIB, MAX_INT)),
    required("reserve_bytes", int(0, MAX_INT)),
];

/// `http.retry_on`'s members.
pub const RETRY_CLASSES: &[&str] = &[
    "connect",
    "no_response",
    "timeout",
    "truncated",
    "status_5xx",
    "status_429",
];

/// `http` (1.4).
pub static HTTP: [Field; 8] = [
    with_default("idle_timeout_ms", int(1000, 600_000), "30000"),
    with_default("min_rate_bytes", int(1, 1 << 30), "1024"),
    with_default("connect_timeout_ms", int(100, 60_000), "10000"),
    with_default("attempts", int(1, 10), "3"),
    with_default("backoff_initial_ms", int(0, 60_000), "500"),
    with_default("backoff_max_ms", int(0, 600_000), "30000"),
    with_default(
        "retry_on",
        Ty::Array(&Array {
            item: one_of(RETRY_CLASSES),
            min: 0,
            max: None,
            unique: true,
        }),
        r#"["connect","no_response","timeout","truncated","status_5xx","status_429"]"#,
    ),
    with_default(
        "redirects",
        one_of(&["same_origin", "none"]),
        r#""same_origin""#,
    ),
];

/// `{"key": Template}`, of `outputs.textures` and `outputs.sprites`.
pub static KEY_ONLY: [Field; 1] = [required("key", TEMPLATE)];

static SCRIPT_CLASSES: Array = Array {
    item: string(1, Some(MAX_ATLAS_NAME), None),
    min: 1,
    max: Some(MAX_ATLAS_NAMES),
    unique: false,
};

static SKIP_NAME_SUFFIXES: Array = Array {
    item: string(1, Some(MAX_ATLAS_NAME), None),
    min: 0,
    max: Some(MAX_ATLAS_NAMES),
    unique: false,
};

/// `outputs.atlases` (1.5).
pub static ATLASES: [Field; 4] = [
    required("sprite_key", TEMPLATE),
    required("json_key", TEMPLATE),
    with_default(
        "script_classes",
        Ty::Array(&SCRIPT_CLASSES),
        r#"["NGUIAtlas"]"#,
    ),
    with_default("skip_name_suffixes", Ty::Array(&SKIP_NAME_SUFFIXES), "[]"),
];

/// `outputs` (1.5).
pub static OUTPUTS: [Field; 3] = [
    field("textures", Ty::Object(&KEY_ONLY)),
    field("sprites", Ty::Object(&KEY_ONLY)),
    field("atlases", Ty::Object(&ATLASES)),
];

/// A Prefer (1.5): each field nullable, at least one not null
/// (`prefer_empty`, a cross-field rule).
pub static PREFER: [Field; 2] = [
    nullable("input", string(0, None, None), Some("null")),
    nullable("md5", string(0, None, Some(Format::Md5)), Some("null")),
];

/// A directory sink (1.6).
pub static DIRECTORY: [Field; 2] = [
    required("kind", one_of(&["directory"])),
    required("root", string(0, None, Some(Format::AbsPath))),
];

/// An object store sink (1.6).
pub static OBJECT_STORE: [Field; 8] = [
    required("kind", one_of(&["object_store"])),
    required("bucket", string(3, Some(255), Some(Format::Bucket))),
    required("region", string(1, Some(64), Some(Format::Region))),
    with_default(
        "prefix",
        string(0, Some(512), Some(Format::Prefix)),
        r#""""#,
    ),
    nullable(
        "endpoint",
        // Its caps, `MAX_ENDPOINT` as written and serialized and
        // `MAX_HOST`, are checked after its ASCII form (`formats`).
        string(0, None, Some(Format::Endpoint)),
        Some("null"),
    ),
    with_default("path_style", Ty::Bool, "false"),
    with_default("timeout_ms", int(1000, 300_000), "30000"),
    with_default("attempts", int(1, 10), "3"),
];

/// An archive member's kinds (1.7).
pub const MEMBER_KINDS: &[&str] = &["texture", "sprite", "atlas_sprite", "atlas_json"];

/// An archive member (1.7).
pub static MEMBER: [Field; 2] = [
    required("kind", one_of(MEMBER_KINDS)),
    required("path", TEMPLATE),
];

/// An archive (1.7).
pub static ARCHIVE: [Field; 2] = [
    required("key", string(0, None, None)),
    required(
        "members",
        Ty::Array(&Array {
            item: Ty::Object(&MEMBER),
            min: 1,
            max: Some(4),
            unique: false,
        }),
    ),
];

/// The top level (1.1).
pub static TOP: [Field; 26] = [
    required("spec_version", int(1, 1)),
    required("run_id", string(1, Some(64), Some(Format::RunId))),
    required("attempt", int(1, (1 << 31) - 1)),
    nullable(
        "deadline",
        string(0, None, Some(Format::DateTime)),
        Some("null"),
    ),
    with_default("shutdown_grace_ms", int(10_000, 600_000), "20000"),
    with_default("stop_on_stdin_eof", Ty::Bool, "false"),
    nullable("parent_pid", int(2, 1 << 22), Some("null")),
    required("threads", int(1, 1024)),
    required("download_concurrency", int(1, 256)),
    required("max_inflight_uncompressed_bytes", int(64 * MIB, 1 << 50)),
    required("max_decoded_bytes", int(MIB, 1 << 40)),
    required("dep_cache_bytes", int(MIB, 1 << 40)),
    with_default("max_view_bundles", int(1, MAX_VIEW_BUNDLES), "1024"),
    with_default("max_attempt_evictions", int(0, 1 << 20), "1024"),
    // Its default, 2 x `max_decoded_bytes`, is no literal.
    field("max_export_work", int(MIB, MAX_INT)),
    required("spool", Ty::Object(&SPOOL)),
    with_default("http", Ty::Object(&HTTP), "{}"),
    required("inputs", Ty::Bundles(&INPUTS, true)),
    with_default("dependencies", Ty::Bundles(&BUNDLES, false), "[]"),
    required("outputs", Ty::Object(&OUTPUTS)),
    with_default("prefer", Ty::Prefer, "{}"),
    with_default("publish_incomplete", Ty::Bool, "true"),
    with_default("resume", Ty::Bool, "false"),
    required("sink", Ty::Sink),
    with_default(
        "archives",
        Ty::Array(&Array {
            item: Ty::Object(&ARCHIVE),
            min: 0,
            max: Some(256),
            unique: false,
        }),
        "[]",
    ),
    with_default(
        "manifest_fencing",
        one_of(&["conditional", "none"]),
        r#""conditional""#,
    ),
];
