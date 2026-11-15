//! A Sprite's failures as 7.3's classes (`docs/BATCH_SPEC.md` 7.3; B3's
//! review round 1, T2): every `SpriteError` and every `MeshError` variant,
//! each with the class `classify::render` gives it. The tables below name
//! each variant once, in their declaration order. That is a convention, not
//! a check: the enums are `#[non_exhaustive]`, so this crate cannot match
//! them exhaustively, and a variant added later compiles unseen here; the
//! one who adds it adds its row (B3's review round 2, T5).

use std::sync::Arc;

use unityrs_core::budget::{BudgetExceeded, Reservation};
use unityrs_core::limits::ChargeError;
use unityrs_core::{Error, LimitError, LimitKind};
use unityrs_export::ngui::classify::render;
use unityrs_export::ngui::{Class, Reason};
use unityrs_meshdecode::{FormatTable, IndexKind, MeshError, StructError, Unsupported};
use unityrs_texture::sprite::SpriteError;
use unityrs_texture::{DecodeError, contain};

fn decode(code: &str) -> Class {
    Class::Skip(Reason::decode_error(code))
}

fn plain(reason: &str) -> Class {
    Class::Skip(Reason::plain(reason))
}

fn limit() -> ChargeError {
    ChargeError::Limit(LimitError::refused(LimitKind::InputMemory, 9, 8))
}

fn budget() -> ChargeError {
    ChargeError::Budget(BudgetExceeded {
        requested: 1,
        what: Reservation::Input,
    })
}

#[test]
fn every_sprite_error_takes_its_class() {
    let payload = match contain::<(), _>(|| panic!("boom")) {
        Err(DecodeError::Panicked { payload }) => payload,
        other => panic!("{other:?}"),
    };
    let table: Vec<(SpriteError, Class)> = vec![
        (SpriteError::CropRightLeft, decode("render")),
        (SpriteError::CropLowerUpper, decode("render")),
        (SpriteError::RoundNan, decode("render")),
        (SpriteError::RoundInfinity, decode("render")),
        (
            SpriteError::DecompressionBomb {
                pixels: "400000000".to_owned(),
            },
            decode("decompression_bomb"),
        ),
        (
            SpriteError::CIntOverflow { above: true },
            decode("crop_overflow"),
        ),
        (
            SpriteError::CIntOverflow { above: false },
            decode("crop_overflow"),
        ),
        (SpriteError::CLongOverflow, decode("crop_overflow")),
        (SpriteError::ImageTooWide, decode("pillow_memory_error")),
        (SpriteError::SolveCast, decode("render")),
        (SpriteError::Rotation(5), decode("render")),
        (SpriteError::AlphaSizeMismatch, decode("render")),
        (SpriteError::AlphaBandCount, decode("render")),
        (
            SpriteError::UnverifiedMode { mode: "L" },
            decode("unsupported"),
        ),
        (SpriteError::NoVertices, decode("render")),
        (
            SpriteError::Unpack {
                expected: 2,
                got: Some(1),
            },
            decode("render"),
        ),
        (SpriteError::Index { tuple: true }, decode("render")),
        (SpriteError::Sprite3d, decode("render")),
        (SpriteError::NoTriangles, decode("render")),
        (SpriteError::NegativeSize, decode("render")),
        (SpriteError::NoAtlasData, decode("render")),
        (SpriteError::AtlasUnresolved, plain("bad_pointer")),
        (SpriteError::Malformed("m_RD"), decode("read")),
        (SpriteError::Mesh(MeshError::Topology), decode("render")),
        (
            SpriteError::TooLarge { what: "image" },
            plain("limit(texture_pixels, 268435456)"),
        ),
        (
            SpriteError::Charge(limit()),
            plain("limit(input_memory, 8)"),
        ),
        (SpriteError::Charge(budget()), plain("budget(input)")),
        (
            SpriteError::Panicked(payload),
            Class::Skip(Reason {
                reason: "decoder_panic".to_owned(),
                detail: Some("boom".to_owned()),
                cut: false,
            }),
        ),
    ];
    for (e, want) in &table {
        assert_eq!(render(e), *want, "{e:?}");
    }
}

#[test]
fn every_mesh_error_takes_its_class() {
    let malformed = Error::Malformed {
        what: "x",
        detail: Arc::from("y"),
    };
    let table: Vec<(MeshError, Class)> = vec![
        (MeshError::Assertion("assert"), decode("render")),
        (MeshError::NoAttribute("m_RD"), decode("render")),
        (MeshError::UnknownMeshType, decode("render")),
        (MeshError::VersionType(9), decode("render")),
        (
            MeshError::InvalidFormat {
                format: 99,
                table: FormatTable::V2019,
            },
            decode("render"),
        ),
        (MeshError::UnknownChannel(20), decode("render")),
        (MeshError::NoChannels, decode("render")),
        (MeshError::OutOfBounds, decode("render")),
        (MeshError::Struct(StructError::ZeroLength), decode("render")),
        (MeshError::Index(IndexKind::List), decode("render")),
        (
            MeshError::Unpack {
                expected: 3,
                got: 2,
            },
            decode("render"),
        ),
        (MeshError::ByteRange, decode("render")),
        (MeshError::Topology, decode("render")),
        (MeshError::Resource(malformed), decode("malformed")),
        (MeshError::Charge(limit()), plain("limit(input_memory, 8)")),
        (MeshError::Charge(budget()), plain("budget(input)")),
        (MeshError::WrappedAccess, decode("render")),
        (
            MeshError::Unsupported(Unsupported::FixedChannels),
            decode("unsupported"),
        ),
        (
            MeshError::Malformed("m_VertexData".to_owned()),
            decode("read"),
        ),
    ];
    for (m, want) in table {
        let e = SpriteError::Mesh(m);
        assert_eq!(render(&e), want, "{e:?}");
    }
}

/// Every format a version 2 spec may list (`outputs.formats`: this
/// binary's `Format::ALL`, ASTC's included) decodes to a sheet the Sprite
/// renderer either takes or refuses as an object's skip, alone and beside
/// an alpha texture: no listed format makes a Sprite its input's failure
/// (`sprites/export.rs` routes each refusal through its group's skips).
#[test]
fn no_decoded_format_makes_a_sheet_an_inputs_failure() {
    use unityrs_core::limits::Meter;
    use unityrs_texture::sprite::Sheet;
    use unityrs_texture::{Format, Mode, decode};

    let meter = Meter::unlimited();
    let data = [0x5a_u8; 64];
    let crunch: serde_json::Value = serde_json::from_str(include_str!(
        "../../unityrs-texture/tests/fixtures/crunch_native.json"
    ))
    .unwrap();
    let unity_crunch: serde_json::Value = serde_json::from_str(include_str!(
        "../../unityrs-texture/tests/fixtures/unitycrunch_native.json"
    ))
    .unwrap();
    let skip_or_ok = |r: Result<Sheet<'_>, SpriteError>, f: Format, how: &str| match r {
        Ok(_) => true,
        Err(e) => {
            let class = render(&e);
            assert!(
                matches!(class, Class::Skip(_)),
                "{f:?} {how}: {e:?} is {class:?}"
            );
            false
        }
    };
    for f in Format::ALL
        .into_iter()
        .chain([Format::UnityDxt1Crunched, Format::UnityDxt5Crunched])
    {
        // CRN needs a valid stream, unlike the fixed-size block/raw formats.
        let crn = f.is_crunched().then(|| {
            let fixture = if f.is_unity_crunched() {
                &unity_crunch
            } else {
                &crunch
            };
            let case = fixture["cases"]
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["format"] == f.texture_format() && c["width"] == 1 && c["height"] == 1)
                .unwrap();
            let hex = case["data"]["hex"].as_str().unwrap();
            (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
                .collect::<Vec<_>>()
        });
        // YUY2 decodes whole pixel pairs only (an odd width is Tier U), so it
        // is the one format taken two wide, beside an alpha texture as wide.
        let width = if matches!(f, Format::Yuy2) { 2 } else { 1 };
        let alpha = decode(Format::Rgba32, &data, width, 1, &meter).unwrap();
        let image = decode(f, crn.as_deref().unwrap_or(&data), width, 1, &meter)
            .unwrap_or_else(|e| panic!("{f:?}: {e:?}"));
        assert_eq!(image.mode(), f.mode(), "{f:?}");
        let alone = skip_or_ok(Sheet::new(&image, None, &meter), f, "alone");
        let merged = skip_or_ok(Sheet::new(&image, Some(&alpha), &meter), f, "merged");
        // Only `L` (BC4's) is refused, alone and merged alike.
        assert_eq!(alone, f.mode() != Mode::L, "{f:?}");
        assert_eq!(merged, f.mode() != Mode::L, "{f:?}");
    }
}
