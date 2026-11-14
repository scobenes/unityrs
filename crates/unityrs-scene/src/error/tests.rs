use super::SceneError;
use std::sync::Arc;
use unityrs_core::budget::{AllocError, BudgetExceeded, Reservation};
use unityrs_core::codec::Codec;
use unityrs_core::detect::DetectError;
use unityrs_core::env::DerefError;
use unityrs_core::limits::ChargeError;
use unityrs_core::objmode::ViewError;
use unityrs_core::provider::ProviderError;
use unityrs_core::typetree::TreeError;
use unityrs_core::version::VersionError;
use unityrs_core::walk::{ParseError, ReadDetail, ReadError, ReadErrorKind};
use unityrs_core::{Error, LimitError, LimitKind, NotFound, RawStr, Unsupported};

fn memory() -> BudgetExceeded {
    BudgetExceeded {
        requested: 128,
        what: Reservation::Dependency,
    }
}

fn read(detail: ReadDetail) -> ParseError {
    ParseError::Read(ReadError {
        kind: ReadErrorKind::Other,
        at: 0,
        detail,
    })
}

fn provider(limit: Option<LimitError>, memory: Option<BudgetExceeded>) -> ParseError {
    let mut error = ProviderError::new("synthetic", Arc::from("synthetic failure"));
    error.limit = limit;
    error.memory = memory;
    ParseError::Provider(error)
}

fn assert_core(error: Error, want: bool) {
    // Each wrapper must preserve both refusals and ordinary failures.
    if let Error::AssetBundle(p) = &error {
        assert_eq!(
            SceneError::Parse((**p).clone()).resource_refusal(),
            want,
            "{p:?}"
        );
    }
    let entry = Error::Entry {
        name: RawStr::from_static(b"entry"),
        source: Arc::new(error.clone()),
    };
    let deref = Error::Deref(Arc::new(DerefError::Load(error.clone())));
    let parse = Error::AssetBundle(Arc::new(ParseError::Deref(Arc::new(DerefError::Load(
        error.clone(),
    )))));
    for e in [error, entry, deref, parse] {
        let scene = SceneError::Deref(DerefError::Load(e));
        assert_eq!(scene.resource_refusal(), want, "{scene:?}");
    }
}

#[test]
fn all_core_and_detection_variants_preserve_the_refusal_policy() {
    // Enumerate every current Error and DetectError variant. Both enums
    // are non_exhaustive: production must refuse unknown variants until
    // they have been audited, rather than silently turn them into issues.
    for kind in [
        LimitKind::InputMemory,
        LimitKind::InputBytes,
        LimitKind::BlockOutput,
        LimitKind::DecodeWork,
        LimitKind::Inflate,
        LimitKind::ContainerDepth,
        LimitKind::TreeDepth,
        LimitKind::ValueDepth,
        LimitKind::ValuesPerObject,
        LimitKind::ZeroSizeElements,
        LimitKind::Count,
    ] {
        let limit = LimitError::refused(kind, 2, 1);
        for error in [
            Error::Limit(limit),
            Error::Detect(DetectError::InflateLimit(limit)),
            Error::Detect(DetectError::Work(limit)),
            Error::Tree(TreeError::Limit(limit)),
            Error::Tree(TreeError::Charge(ChargeError::Limit(limit))),
            Error::AssetBundle(Arc::new(read(ReadDetail::Limit(limit)))),
            Error::AssetBundle(Arc::new(read(ReadDetail::Charge(ChargeError::Limit(
                limit,
            ))))),
            Error::AssetBundle(Arc::new(read(ReadDetail::Tree(TreeError::Limit(limit))))),
            Error::AssetBundle(Arc::new(ParseError::Tree(TreeError::Limit(limit)))),
            Error::AssetBundle(Arc::new(provider(Some(limit), None))),
        ] {
            assert_core(error, true);
        }
    }
    for error in [
        Error::Budget(memory()),
        Error::Allocator(AllocError {
            requested: 128,
            what: Reservation::Dependency,
        }),
        Error::Detect(DetectError::Allocator(AllocError {
            requested: 128,
            what: Reservation::Dependency,
        })),
        Error::Tree(TreeError::Charge(ChargeError::Allocator(AllocError {
            requested: 128,
            what: Reservation::Dependency,
        }))),
        Error::AssetBundle(Arc::new(read(ReadDetail::Charge(ChargeError::Allocator(
            AllocError {
                requested: 128,
                what: Reservation::Dependency,
            },
        ))))),
        Error::Tree(TreeError::Charge(ChargeError::Budget(memory()))),
        Error::Tree(TreeError::TooManyNodes(usize::MAX)),
        Error::AssetBundle(Arc::new(read(ReadDetail::Charge(ChargeError::Budget(
            memory(),
        ))))),
        Error::AssetBundle(Arc::new(provider(None, Some(memory())))),
    ] {
        assert_core(error, true);
    }
    for error in [
        Error::Detect(DetectError::Unterminated),
        Error::Detect(DetectError::Gzip(Arc::from("corrupt gzip"))),
        Error::OutOfBounds {
            what: "test",
            pos: 0,
            need: 1,
            have: 0,
        },
        Error::Malformed {
            what: "test",
            detail: Arc::from("bad input"),
        },
        Error::Unsupported(Unsupported::WebFile),
        Error::Decompress {
            codec: Codec::Lz4,
            detail: Arc::from("corrupt block"),
        },
        Error::Version(VersionError::NoFallback),
        Error::TypeIndex { index: 1, len: 0 },
        Error::Tree(TreeError::Empty),
        Error::Tree(TreeError::LevelUnderflow { row: 0, level: -1 }),
        Error::AssetBundle(Arc::new(ParseError::NoTypeTree {
            class_id: 1,
            why: None,
        })),
        Error::NotFound(NotFound::Source(0)),
        Error::Deref(Arc::new(DerefError::Null)),
        Error::from(std::io::Error::from(std::io::ErrorKind::UnexpectedEof)),
    ] {
        assert_core(error, false);
    }
}

#[test]
fn all_object_view_variants_preserve_the_refusal_policy() {
    // Tier U shape refusals and missing attributes are data failures;
    // all memory, allocator and work refusals must remain fatal.
    for error in [
        ViewError::Malformed("wrong class".to_owned()),
        ViewError::NoAttribute("m_Mesh"),
        ViewError::ByteRange,
    ] {
        assert!(!SceneError::from(error).resource_refusal());
    }
    for error in [
        ChargeError::Budget(memory()),
        ChargeError::Allocator(AllocError {
            requested: 128,
            what: Reservation::Dependency,
        }),
        ChargeError::Limit(LimitError::refused(LimitKind::InputMemory, 2, 1)),
        ChargeError::Limit(LimitError::refused(LimitKind::DecodeWork, 2, 1)),
    ] {
        assert!(SceneError::from(ViewError::Charge(error)).resource_refusal());
    }
}

#[test]
fn provider_allocator_refusals_remain_fatal() {
    let mut error = ProviderError::new("synthetic", Arc::from("allocator refusal"));
    error.allocator = Some(AllocError {
        requested: 128,
        what: Reservation::Provider,
    });
    assert_core(
        Error::AssetBundle(Arc::new(ParseError::Provider(error))),
        true,
    );
}

#[test]
fn ordinary_parse_and_pointer_failures_remain_recoverable() {
    use unityrs_core::limits::Meter;
    use unityrs_core::stream::StreamRefusal;
    use unityrs_core::typetree::{NodeList, NodeRow};
    use unityrs_core::value::{Value, ValueBuilder};
    let rows = Arc::new(NodeList::trusted(vec![NodeRow::new(0, "bool", "value")]).unwrap());
    let builder = ValueBuilder::new(&Meter::unlimited(), rows).unwrap();
    let value = builder.scalar(Value::Bool(true)).unwrap();
    assert_core(
        Error::AssetBundle(Arc::new(ParseError::CheckRead {
            expected: 2,
            parsed: Arc::new(builder.finish(value, 1)),
        })),
        false,
    );
    for detail in [
        ReadDetail::Eof { need: 1 },
        ReadDetail::NegativeLength(-1),
        ReadDetail::ArrayChildren,
        ReadDetail::PairChildren,
        ReadDetail::NoRefTypes,
        ReadDetail::RefTypeNotFound,
        ReadDetail::RefTypeNoTree,
        ReadDetail::Python("bad input"),
        ReadDetail::Stream(StreamRefusal::NegativePosition),
        ReadDetail::Stream(StreamRefusal::PositionTooLarge),
        ReadDetail::Stream(StreamRefusal::NegativeRead),
        ReadDetail::Stream(StreamRefusal::ReadTooLarge),
        ReadDetail::Stream(StreamRefusal::ReadSizeOverflow),
        ReadDetail::Tree(TreeError::Empty),
    ] {
        assert_core(Error::AssetBundle(Arc::new(read(detail))), false);
    }
    assert_core(Error::AssetBundle(Arc::new(provider(None, None))), false);
    for error in [
        DerefError::Null,
        DerefError::FileIdPastEnd { file_id: 1 },
        DerefError::FileIdBelowStart { file_id: -1 },
        DerefError::NotFound {
            path: RawStr::from_static(b"missing"),
        },
        DerefError::RawTarget {
            name: RawStr::from_static(b"raw"),
        },
        DerefError::MissingObject {
            file: RawStr::from_static(b"file"),
            path_id: 1,
        },
        DerefError::ContainerDropped {
            name: RawStr::from_static(b"bundle"),
        },
    ] {
        assert!(!SceneError::Deref(error.clone()).resource_refusal());
        assert_core(Error::Deref(Arc::new(error)), false);
    }
    for error in [
        SceneError::Malformed("field"),
        SceneError::WrongClass(43),
        SceneError::NoTransform,
    ] {
        assert!(!error.resource_refusal());
    }
    assert!(SceneError::Charge(ChargeError::Budget(memory())).resource_refusal());
}
