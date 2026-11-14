//! `classify` (`docs/plans/batch.md` 2.7) as far as the cutter meets it:
//! every error a read, a dereference, a resource read, a charge or a decode
//! returns, to an object's skip, the input's failure or a run failure, with
//! the reason and detail `docs/BATCH_SPEC.md` 7.3 and 9.3 name.
//!
//! The site rule for an `Error::Entry` inside a dereference or a resource
//! read (the fourth carried question): it is the reading object's skip, with
//! the innermost error's code; only selection's open of the input's own
//! top-level rows, or the reach of its containers at registration, makes an
//! entry's failure the input's `entry_error` ([`entry_error`]; `batch.md`
//! 2.7). A string from the bytes is under 7.3's text rule ([`text`]).

use std::collections::TryReserveError;

use unityrs_core::budget::{BudgetExceeded, Reservation};
use unityrs_core::env::DerefError;
use unityrs_core::limits::ChargeError;
use unityrs_core::typetree::TreeError;
use unityrs_core::walk::{ParseError, ReadDetail};
use unityrs_core::{Error, LimitError, LimitKind, NotFound, RawStr};
use unityrs_meshdecode::MeshError;
use unityrs_texture::sprite::SpriteError;
use unityrs_texture::{DecodeError, MAX_PIXELS};

/// The longest detail or reason argument, in bytes (7.3's cut).
pub const CUT: usize = 1024;

/// A reason of 9.3 and its 7.3 detail, each string taken from the bytes
/// already under 7.3's text rule ([`text`]), and whether one was cut (the
/// row's `cut`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Reason {
    /// The reason: `name` or `name(argument, ...)`.
    pub reason: String,
    /// The detail, or none.
    pub detail: Option<String>,
    /// Whether the reason's argument or the detail was cut to [`CUT`] bytes.
    pub cut: bool,
}

impl Reason {
    /// A reason with no detail.
    #[must_use]
    pub fn plain(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            detail: None,
            cut: false,
        }
    }

    /// `decode_error` with `code` as its detail.
    #[must_use]
    pub fn decode_error(code: impl Into<String>) -> Self {
        Self {
            reason: "decode_error".to_owned(),
            detail: Some(code.into()),
            cut: false,
        }
    }
}

/// Where a failure lands (2.7's classes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Class {
    /// The object's skip: a data reason, the budget class's object-level
    /// reasons (`budget(input)`, a meter's `limit`), or a decode's.
    Skip(Reason),
    /// The input fails (`budget(dependency)`, and the site rules' input
    /// failures).
    Input(Reason),
    /// The run fails: `host_io`, `host_memory` or `internal`.
    Run(Reason),
}

/// 7.3's text rule: `bytes` cut to their first [`CUT`] bytes at a code
/// point boundary, an invalid byte counting as one code point, then each
/// invalid byte replaced by U+FFFD; and whether it was cut. The string is
/// allocated once, at the length [`text_len`] measures.
#[must_use]
pub fn text(bytes: &[u8]) -> (String, bool) {
    let (n, _) = text_len(&[bytes]);
    let mut out = String::with_capacity(n);
    let cut = walk(&[bytes], |c| out.push(c));
    (out, cut)
}

/// The text rule over `parts` read as one string (each part but the last
/// ending at a code point boundary), each code point it keeps handed to
/// `put`: whether it cut. Nothing is copied.
fn walk(parts: &[&[u8]], mut put: impl FnMut(char)) -> bool {
    let mut used = 0usize;
    for part in parts {
        for chunk in part.utf8_chunks() {
            for c in chunk.valid().chars() {
                if used + c.len_utf8() > CUT {
                    return true;
                }
                used += c.len_utf8();
                put(c);
            }
            for _ in chunk.invalid() {
                if used + 1 > CUT {
                    return true;
                }
                used += 1;
                put(char::REPLACEMENT_CHARACTER);
            }
        }
    }
    false
}

/// The bytes the text rule's string of `parts` takes (an invalid byte's
/// U+FFFD three of them, so at most 3 x [`CUT`]), and whether it cuts:
/// measured, never made.
#[must_use]
pub fn text_len(parts: &[&[u8]]) -> (usize, bool) {
    let mut n = 0usize;
    let cut = walk(parts, |c| n += c.len_utf8());
    (n, cut)
}

/// [`text`] of `parts` read as one string, as [`walk`] reads them, into a
/// string reserved fallibly at exactly [`text_len`]'s length.
///
/// # Errors
///
/// The allocator's refusal of that reservation.
pub fn try_text(parts: &[&[u8]]) -> Result<(String, bool), TryReserveError> {
    let (n, _) = text_len(parts);
    let mut out = String::new();
    out.try_reserve_exact(n)?;
    let cut = walk(parts, |c| out.push(c));
    Ok((out, cut))
}

/// `name(<argument>)`, the argument from the bytes under [`text`].
#[must_use]
pub fn with_argument(name: &str, argument: &[u8]) -> Reason {
    let (arg, cut) = text(argument);
    Reason {
        reason: format!("{name}({arg})"),
        detail: None,
        cut,
    }
}

/// `outside_view(<name>)`, the name as given (a resource's `path` as read).
#[must_use]
pub fn outside_view(name: &RawStr) -> Reason {
    with_argument("outside_view", name.as_bytes())
}

/// `outside_view(<cab>)` for a pointer's external that no file of the view
/// holds: the CAB, what follows its path's last `/` (9.3; UnityPy's `PPtr`
/// splits on `/` alone, so a `\` stays in the name).
#[must_use]
pub fn outside_view_cab(path: &RawStr) -> Reason {
    let p = path.as_bytes();
    let start = p.iter().rposition(|&b| b == b'/').map_or(0, |i| i + 1);
    with_argument("outside_view", p.get(start..).unwrap_or_default())
}

fn internal() -> Class {
    Class::Run(Reason::plain("internal"))
}

/// A limit kind's name in `limit(<kind>, ..)` (9.3's table).
#[must_use]
pub fn limit_kind(kind: LimitKind) -> Option<&'static str> {
    Some(match kind {
        LimitKind::InputMemory => "input_memory",
        LimitKind::InputBytes => "input_bytes",
        LimitKind::DecodeWork => "decode_work",
        LimitKind::BlockOutput => "block_output",
        LimitKind::Inflate => "inflate",
        LimitKind::ContainerDepth => "container_depth",
        LimitKind::TreeDepth => "tree_depth",
        LimitKind::ValueDepth => "value_depth",
        LimitKind::ValuesPerObject => "values_per_object",
        LimitKind::ZeroSizeElements => "zero_size_elements",
        LimitKind::Count => "count",
        _ => return None,
    })
}

/// A limit: `limit(<kind>, <configured value>)`, an object's skip whether
/// its kind is the budget class's or data (2.7: both at the same sites).
#[must_use]
pub fn limit(l: &LimitError) -> Class {
    match limit_kind(l.kind()) {
        Some(kind) => Class::Skip(Reason::plain(format!("limit({kind}, {})", l.max()))),
        None => internal(),
    }
}

/// A budget's refusal: the input's own is an object's skip; a dependency's
/// fails the input (2.6 step 7); a provider's, which no input owns, is the
/// allocator's (`host_memory`).
#[must_use]
pub fn budget(b: &BudgetExceeded) -> Class {
    match b.what {
        Reservation::Input => Class::Skip(Reason::plain("budget(input)")),
        Reservation::Dependency => Class::Input(Reason::plain("budget(dependency)")),
        Reservation::Provider => Class::Run(Reason::plain("host_memory")),
    }
}

/// A refused charge.
#[must_use]
pub fn charge(c: &ChargeError) -> Class {
    match c {
        ChargeError::Limit(l) => limit(l),
        ChargeError::Budget(b) => budget(b),
        ChargeError::Allocator(_) => Class::Run(Reason::plain("host_memory")),
        _ => Class::Run(Reason::plain("internal")),
    }
}

/// A tree's failure: rows that make no tree are `tree` (the first carried
/// question: 7.3 lists the code, and the core's `TreeError` is it).
#[must_use]
pub fn tree(t: &TreeError) -> Class {
    match t {
        TreeError::Empty | TreeError::LevelUnderflow { .. } | TreeError::TooManyNodes(_) => {
            Class::Skip(Reason::decode_error("tree"))
        }
        TreeError::Limit(l) => limit(l),
        TreeError::Charge(c) => charge(c),
        _ => internal(),
    }
}

/// A typetree read's failure.
#[must_use]
pub fn parse(e: &ParseError) -> Class {
    match e {
        ParseError::Provider(p) if p.allocator.is_some() => {
            Class::Run(Reason::plain("host_memory"))
        }
        ParseError::NoTypeTree { .. } | ParseError::CheckRead { .. } | ParseError::Provider(_) => {
            Class::Skip(Reason::decode_error("asset_bundle"))
        }
        ParseError::Read(r) => match &r.detail {
            // A tree failure met during the read (a peek's, a nested
            // compile's) is the read's, but for a limit or a charge: `tree`
            // is only the object's own tree failing (`ParseError::Tree`, 7.3).
            ReadDetail::Limit(l) | ReadDetail::Tree(TreeError::Limit(l)) => limit(l),
            ReadDetail::Charge(c) | ReadDetail::Tree(TreeError::Charge(c)) => charge(c),
            ReadDetail::Eof { .. }
            | ReadDetail::NegativeLength(_)
            | ReadDetail::ArrayChildren
            | ReadDetail::PairChildren
            | ReadDetail::NoRefTypes
            | ReadDetail::RefTypeNotFound
            | ReadDetail::RefTypeNoTree
            | ReadDetail::Python(_)
            | ReadDetail::Stream(_)
            | ReadDetail::Tree(
                TreeError::Empty | TreeError::LevelUnderflow { .. } | TreeError::TooManyNodes(_),
            ) => Class::Skip(Reason::decode_error("read")),
            _ => internal(),
        },
        ParseError::Tree(t) => tree(t),
        ParseError::Deref(d) => deref(d),
        _ => internal(),
    }
}

/// A dereference's failure, by `DerefError` (2.7).
#[must_use]
pub fn deref(e: &DerefError) -> Class {
    match e {
        DerefError::Null
        | DerefError::FileIdPastEnd { .. }
        | DerefError::FileIdBelowStart { .. }
        | DerefError::RawTarget { .. }
        | DerefError::MissingObject { .. } => Class::Skip(Reason::plain("bad_pointer")),
        DerefError::NotFound { path } => Class::Skip(outside_view_cab(path)),
        DerefError::Load(e) => error(e),
        _ => internal(),
    }
}

/// The code of an error the bytes decide (7.3), through its wrappers.
#[must_use]
pub fn error(e: &Error) -> Class {
    let code = match e {
        Error::Entry { source, .. } => return error(source),
        Error::AssetBundle(p) => return parse(p),
        Error::Deref(d) => return deref(d),
        Error::Tree(t) => return tree(t),
        Error::Limit(l) => return limit(l),
        Error::Budget(b) => return budget(b),
        Error::Allocator(_) | Error::Detect(unityrs_core::detect::DetectError::Allocator(_)) => {
            return Class::Run(Reason::plain("host_memory"));
        }
        Error::Io(_) => return Class::Run(Reason::plain("host_io")),
        Error::Detect(_) => "detect",
        Error::OutOfBounds { .. } => "out_of_bounds",
        Error::Malformed { .. } => "malformed",
        Error::Unsupported(_) => "unsupported",
        Error::Decompress { .. } => "decompress",
        Error::Version(_) => "version",
        Error::TypeIndex { .. } => "type_index",
        Error::NotFound(_) => "not_found",
        _ => return internal(),
    };
    Class::Skip(Reason::decode_error(code))
}

/// A resource read's failure from the Environment's fallback: a resource no
/// candidate finds is `outside_view(<path>)`, its path as read (E65).
#[must_use]
pub fn resource(e: &Error, path: &RawStr) -> Class {
    match e {
        Error::NotFound(NotFound::Resource(_)) => Class::Skip(outside_view(path)),
        other => error(other),
    }
}

/// A texture decode's failure (2.7's decode row; section 3's answers).
#[must_use]
pub fn decode(e: &DecodeError) -> Class {
    let code = match e {
        DecodeError::NoImageData => "no_image_data",
        DecodeError::NegativeSize { .. } => "negative_size",
        DecodeError::PillowMemoryError { .. } => "pillow_memory_error",
        DecodeError::NotEnoughData { .. } => "not_enough_data",
        DecodeError::DecompressionBomb { .. } => "decompression_bomb",
        DecodeError::TooLarge { .. } => {
            return Class::Skip(Reason::plain(format!(
                "limit(texture_pixels, {MAX_PIXELS})"
            )));
        }
        DecodeError::Charge(c) => return charge(c),
        DecodeError::Panicked { payload } => return panicked(payload),
        _ => return internal(),
    };
    Class::Skip(Reason::decode_error(code))
}

/// A contained panic: `decoder_panic`, its payload the detail. The payload
/// is cut where the panic was contained, to `MESSAGE_BYTES` (1,024) at a
/// code point boundary: its flag is the row's `cut`.
fn panicked(payload: &unityrs_texture::Payload) -> Class {
    Class::Skip(Reason {
        reason: "decoder_panic".to_owned(),
        detail: Some(payload.message().to_owned()),
        cut: payload.is_cut(),
    })
}

/// A Sprite's render failure (E17; 7.3's codes for a Sprite): Pillow's
/// refusals of a crop or an image as an atlas crop's are named
/// (`decompression_bomb`, `crop_overflow` for a box edge outside a C
/// `int` or `long`, `pillow_memory_error` for an image wider than Pillow
/// makes), a field of another shape is `read` (4.4 step 2's rule), a
/// canvas past the renderer's bound is `limit(texture_pixels, ..)` as a
/// texture's is, a refused charge or work count is its own class, and a
/// panic `decoder_panic`; every other exception UnityPy 1.25.2's
/// `SpriteHelper` raises is `render`.
#[must_use]
pub fn render(e: &SpriteError) -> Class {
    let code = match e {
        SpriteError::DecompressionBomb { .. } => "decompression_bomb",
        SpriteError::CIntOverflow { .. } | SpriteError::CLongOverflow => "crop_overflow",
        SpriteError::ImageTooWide => "pillow_memory_error",
        SpriteError::Malformed(_) => "read",
        SpriteError::AtlasUnresolved => return Class::Skip(Reason::plain("bad_pointer")),
        SpriteError::TooLarge { .. } => {
            return Class::Skip(Reason::plain(format!(
                "limit(texture_pixels, {MAX_PIXELS})"
            )));
        }
        SpriteError::Charge(c) => return charge(c),
        SpriteError::Panicked(payload) => return panicked(payload),
        SpriteError::Mesh(m) => return mesh(m),
        SpriteError::CropRightLeft
        | SpriteError::CropLowerUpper
        | SpriteError::RoundNan
        | SpriteError::RoundInfinity
        | SpriteError::SolveCast
        | SpriteError::Rotation(_)
        | SpriteError::AlphaSizeMismatch
        | SpriteError::NoVertices
        | SpriteError::Unpack { .. }
        | SpriteError::Index { .. }
        | SpriteError::Sprite3d
        | SpriteError::NoTriangles
        | SpriteError::NegativeSize
        | SpriteError::NoAtlasData => "render",
        _ => return internal(),
    };
    Class::Skip(Reason::decode_error(code))
}

/// A Sprite's mesh failure (E16 inside E17): a refused charge or work count
/// is its own class, a part `unityrs-meshdecode` leaves to a later phase
/// `unsupported`, a field of another shape `read`, and every exception
/// UnityPy's `MeshHandler` raises `render`, as the render's own.
fn mesh(e: &MeshError) -> Class {
    let code = match e {
        MeshError::Charge(c) => return charge(c),
        MeshError::Resource(e) => return error(e),
        MeshError::Unsupported(_) => "unsupported",
        MeshError::Malformed(_) => "read",
        MeshError::Assertion(_)
        | MeshError::NoAttribute(_)
        | MeshError::UnknownMeshType
        | MeshError::VersionType(_)
        | MeshError::InvalidFormat { .. }
        | MeshError::UnknownChannel(_)
        | MeshError::NoChannels
        | MeshError::OutOfBounds
        | MeshError::Struct(_)
        | MeshError::Index(_)
        | MeshError::Unpack { .. }
        | MeshError::ByteRange
        | MeshError::Topology
        | MeshError::WrappedAccess => "render",
        _ => return internal(),
    };
    Class::Skip(Reason::decode_error(code))
}

/// Selection's open of a top-level row of the input that fails: the input's
/// `entry_error`, its detail the code, a space and the row's path, the whole
/// cut to [`CUT`] bytes, written by the text rule from the three parts in
/// place, so the path is never copied whole (review round 3's P2); a class
/// other than a code (a budget, a stop) stays what it is, the input's or
/// the run's.
#[must_use]
pub fn entry_error(e: &Error, path: &RawStr) -> Class {
    match error(e) {
        Class::Skip(r) if r.reason == "decode_error" => {
            let code = r.detail.unwrap_or_default();
            // The code is ASCII, so each part but the path ends at a code
            // point boundary.
            let parts = [code.as_bytes(), b" ".as_slice(), path.as_bytes()];
            let Ok((detail, cut)) = try_text(&parts) else {
                return Class::Run(Reason::plain("host_memory"));
            };
            Class::Input(Reason {
                reason: "entry_error".to_owned(),
                detail: Some(detail),
                cut,
            })
        }
        Class::Skip(r) => Class::Input(r),
        other => other,
    }
}

/// Whether a prepared unsupported-format argument is a pinned format name.
#[must_use]
pub fn known_texture_format(name: &str) -> bool {
    unityrs_texture::TEXTURE_FORMATS
        .iter()
        .any(|(_, n)| *n == name)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use unityrs_core::budget::{BudgetExceeded, Reservation};
    use unityrs_core::env::DerefError;
    use unityrs_core::typetree::TreeError;
    use unityrs_core::walk::{ParseError, ReadDetail, ReadError, ReadErrorKind};
    use unityrs_core::{Error, LimitError, LimitKind, NotFound, RawStr};
    use unityrs_texture::DecodeError;

    use super::{Class, Reason, entry_error, error, parse, resource, text, text_len, try_text};

    fn skip(reason: &str, detail: Option<&str>) -> Class {
        Class::Skip(Reason {
            reason: reason.to_owned(),
            detail: detail.map(str::to_owned),
            cut: false,
        })
    }

    fn read(detail: ReadDetail) -> ParseError {
        ParseError::Read(ReadError {
            kind: ReadErrorKind::Other,
            at: 0,
            detail,
        })
    }

    #[test]
    fn each_wrapper_reaches_the_innermost_code() {
        let malformed = Error::Malformed {
            what: "x",
            detail: "y".into(),
        };
        let entry = Error::Entry {
            name: RawStr::from(b"CAB-a".as_slice()),
            source: Arc::new(malformed.clone()),
        };
        let load = Error::Deref(Arc::new(DerefError::Load(entry.clone())));
        let in_parse = Error::AssetBundle(Arc::new(ParseError::Deref(Arc::new(DerefError::Load(
            load.clone(),
        )))));
        for e in [&malformed, &entry, &load, &in_parse] {
            assert_eq!(error(e), skip("decode_error", Some("malformed")), "{e}");
        }
        let missing = DerefError::NotFound {
            path: RawStr::from(b"archive:/CAB-z/CAB-z".as_slice()),
        };
        assert_eq!(
            error(&Error::Deref(Arc::new(missing))),
            skip("outside_view(CAB-z)", None)
        );
        assert_eq!(
            error(&Error::Deref(Arc::new(DerefError::Null))),
            skip("bad_pointer", None)
        );
    }

    #[test]
    fn parse_errors_take_their_codes_and_limits_their_kinds() {
        let tree = TreeError::LevelUnderflow { row: 1, level: 3 };
        assert_eq!(
            parse(&ParseError::Tree(tree.clone())),
            skip("decode_error", Some("tree"))
        );
        assert_eq!(
            parse(&read(ReadDetail::Tree(tree))),
            skip("decode_error", Some("read"))
        );
        assert_eq!(
            parse(&read(ReadDetail::Eof { need: 4 })),
            skip("decode_error", Some("read"))
        );
        let depth = LimitError::refused(LimitKind::ValueDepth, 600, 512);
        assert_eq!(
            parse(&read(ReadDetail::Limit(depth))),
            skip("limit(value_depth, 512)", None)
        );
        assert_eq!(
            parse(&ParseError::NoTypeTree {
                class_id: 28,
                why: None
            }),
            skip("decode_error", Some("asset_bundle"))
        );
        let dep = BudgetExceeded {
            requested: 1,
            what: Reservation::Dependency,
        };
        assert_eq!(
            error(&Error::Budget(dep)),
            Class::Input(Reason::plain("budget(dependency)"))
        );
        let input = BudgetExceeded {
            requested: 1,
            what: Reservation::Input,
        };
        assert_eq!(error(&Error::Budget(input)), skip("budget(input)", None));
    }

    #[test]
    fn a_missing_resource_names_its_path_and_a_decode_its_code() {
        let path = RawStr::from(b"archive:/CAB-x/r.resS".as_slice());
        let e = Error::NotFound(NotFound::Resource(RawStr::from(b"r.resS".as_slice())));
        assert_eq!(
            resource(&e, &path),
            skip("outside_view(archive:/CAB-x/r.resS)", None)
        );
        assert_eq!(
            super::decode(&DecodeError::TooLarge { pixels: 1, max: 1 }),
            skip("limit(texture_pixels, 268435456)", None)
        );
        assert_eq!(
            super::decode(&DecodeError::NegativeSize {
                width: -1,
                height: 1
            }),
            skip("decode_error", Some("negative_size"))
        );
    }

    #[test]
    fn an_open_of_an_own_row_is_the_inputs_entry_error() {
        let e = Error::Unsupported(unityrs_core::Unsupported::WebFile);
        let got = entry_error(&e, &RawStr::from(b"web.data".as_slice()));
        assert_eq!(
            got,
            Class::Input(Reason {
                reason: "entry_error".to_owned(),
                detail: Some("unsupported web.data".to_owned()),
                cut: false,
            })
        );
        let long = vec![b'a'; 2000];
        let Class::Input(r) = entry_error(&e, &RawStr::from(long.as_slice())) else {
            panic!("an input failure");
        };
        assert_eq!((r.detail.map(|d| d.len()), r.cut), (Some(1024), true));
    }

    #[test]
    fn text_is_cut_by_its_bytes_then_each_invalid_byte_replaced() {
        // A truncated sequence is two invalid bytes, two U+FFFDs (review
        // round 1: the lossy decoding gave one).
        assert_eq!(text(b"\xe2\x82"), ("\u{fffd}\u{fffd}".to_owned(), false));
        assert_eq!(text(b"a\xffb"), ("a\u{fffd}b".to_owned(), false));
        // 1,024 bytes are kept whole; 1,025 are cut, and say so.
        let a = |n| vec![b'a'; n];
        assert_eq!(text(&a(1024)), ("a".repeat(1024), false));
        assert_eq!(text(&a(1025)), ("a".repeat(1024), true));
        // A code point is never split: 1,023 bytes and a two-byte one.
        let mut split = a(1023);
        split.extend_from_slice("é".as_bytes());
        assert_eq!(text(&split), ("a".repeat(1023), true));
        // 500 invalid bytes are 500 code points, under the cut, each 3 bytes
        // once replaced.
        let (t, cut) = text(&[0xff; 500]);
        assert_eq!((t.chars().count(), t.len(), cut), (500, 1500, false));
        let (t, cut) = text(&[0xff; 1025]);
        assert_eq!((t.chars().count(), cut), (1024, true));
    }

    #[test]
    fn the_text_rule_is_measured_then_made_at_that_length() {
        // Review round 3's P2: 1,024 invalid bytes are 3,072 bytes of
        // U+FFFD, measured before anything is made, and made at exactly
        // that capacity; parts read as one string.
        assert_eq!(text_len(&[&[0xff; 1100]]), (3072, true));
        let (t, cut) = try_text(&[&[0xff; 1024]]).unwrap();
        assert_eq!((t.len(), t.capacity(), cut), (3072, 3072, false));
        let path = vec![b'p'; 30_000];
        let parts: [&[u8]; 3] = [b"unsupported", b" ", &path];
        assert_eq!(text_len(&parts), (1024, true));
        let (t, cut) = try_text(&parts).unwrap();
        assert_eq!((&t[..13], t.len(), cut), ("unsupported p", 1024, true));
        // A long path's `entry_error` keeps its first 1,024 bytes.
        let e = Error::Unsupported(unityrs_core::Unsupported::WebFile);
        let path = RawStr::from(vec![b'q'; 30_000]);
        let Class::Input(r) = entry_error(&e, &path) else {
            panic!("an input failure");
        };
        assert_eq!(r.detail.map(|d| d.len()), Some(1024));
    }

    #[test]
    fn an_outside_view_pointer_names_its_cabs_basename() {
        let path = RawStr::from(b"archive:/CAB-x/CAB-x".as_slice());
        let got = super::deref(&DerefError::NotFound { path });
        assert_eq!(got, skip("outside_view(CAB-x)", None));
        let long = RawStr::from(vec![b'p'; 1100]);
        let Class::Skip(r) = super::deref(&DerefError::NotFound { path: long }) else {
            panic!("a skip");
        };
        assert!(r.cut);
        assert_eq!(r.reason.len(), "outside_view()".len() + 1024);
        // Split on `/` alone, as UnityPy's `PPtr` splits (review round 2's
        // C8): a backslash is part of the name.
        let path = RawStr::from(b"archive:/CAB-a\\b".as_slice());
        let got = super::deref(&DerefError::NotFound { path });
        assert_eq!(got, skip("outside_view(CAB-a\\b)", None));
        let path = RawStr::from(b"CAB-c".as_slice());
        let got = super::deref(&DerefError::NotFound { path });
        assert_eq!(got, skip("outside_view(CAB-c)", None));
    }

    #[test]
    fn a_contained_panic_is_decoder_panic_with_its_cut() {
        // Review round 2's T3: through `unityrs_texture::contain`, as the
        // decoders' panics come.
        let panicked = |f: fn()| {
            let e = unityrs_texture::contain(|| -> Result<(), DecodeError> {
                f();
                Ok(())
            })
            .unwrap_err();
            let Class::Skip(r) = super::decode(&e) else {
                panic!("a skip");
            };
            assert_eq!(r.reason, "decoder_panic");
            (r.detail.unwrap_or_default(), r.cut)
        };
        let short = panicked(|| panic!("short"));
        let long = panicked(|| panic!("{}\u{e9}", "a".repeat(1023)));
        let other = panicked(|| std::panic::panic_any(7u32));
        assert_eq!(short, ("short".to_owned(), false));
        // 1,023 bytes and a two-byte character: cut before it.
        assert_eq!(long, ("a".repeat(1023), true));
        assert_eq!(
            other,
            (unityrs_texture::NON_STRING_PAYLOAD.to_owned(), false)
        );
    }

    #[test]
    fn a_tree_failure_inside_a_read_is_read_and_the_objects_own_is_tree() {
        assert_eq!(
            parse(&read(ReadDetail::Tree(TreeError::Empty))),
            skip("decode_error", Some("read"))
        );
        assert_eq!(
            parse(&ParseError::Tree(TreeError::Empty)),
            skip("decode_error", Some("tree"))
        );
    }
}
