use std::ops::Range;
use std::sync::Arc;

use bytes::Bytes;

use super::{BlockCodec, Codec, Codecs, Decoded, Lz4, decode_block};
use crate::budget::DecodeBuf;
use crate::error::{Error, LimitKind};
use crate::limits::Meter;

/// A codec that writes `write` bytes, then returns `range` (or
/// `Written` when there is none).
struct Fake {
    write: usize,
    range: Option<Range<usize>>,
}

impl BlockCodec for Fake {
    fn decode(&self, _: &Bytes, _: u32, out: &mut DecodeBuf) -> Result<Decoded, Error> {
        out.grow_zeroed(self.write)?;
        Ok(self.range.clone().map_or(Decoded::Written, Decoded::Input))
    }
}

/// A codec that leaves its segment: cuts the run to `cut` (when set),
/// optionally begins a segment of its own, writes `write` bytes and
/// returns `Written`.
struct Escapes {
    cut: Option<usize>,
    own_segment: bool,
    write: usize,
}

impl BlockCodec for Escapes {
    fn decode(&self, _: &Bytes, _: u32, out: &mut DecodeBuf) -> Result<Decoded, Error> {
        if let Some(len) = self.cut {
            out.truncate(len);
        }
        if self.own_segment {
            out.begin_block(u64::MAX, LimitKind::InputBytes, u64::MAX);
        }
        out.grow_zeroed(self.write)?;
        Ok(Decoded::Written)
    }
}

#[test]
fn a_codec_that_leaves_its_segment_is_its_error() {
    // Round 6 (claude6 L1, K1): `Written` was trusted, so a codec could
    // erase earlier blocks of the run, lift its cap and escape the work
    // count by beginning a segment of its own, and `decode_block`
    // returned `Ok(None)`.
    let input = Bytes::from(vec![7u8; 8]);
    let run = |codec: Escapes| {
        let limits = crate::limits::Limits {
            max_decode_work: 1 << 20,
            ..crate::limits::Limits::default()
        };
        let m = Meter::for_input(
            &limits,
            Arc::new(crate::budget::Unbounded),
            crate::budget::Reservation::Input,
        );
        let mut out = DecodeBuf::new(&m, 0, u64::MAX, LimitKind::BlockOutput).unwrap();
        out.extend_from_slice(&[1; 8]).unwrap(); // block 0 of the run
        let got = decode_block(Codec::Other(9), &codec, &input, 16, &mut out, 16);
        (got, m.decoded())
    };
    for (codec, what) in [
        (
            Escapes {
                cut: Some(0),
                own_segment: true,
                write: 64,
            },
            "erases block 0, writes 64 bytes past a 16-byte cap",
        ),
        (
            Escapes {
                cut: None,
                own_segment: true,
                write: 64,
            },
            "a segment of its own, past its cap and uncounted",
        ),
        (
            Escapes {
                cut: Some(4),
                own_segment: false,
                write: 4,
            },
            "cuts into block 0",
        ),
    ] {
        let (got, _) = run(codec);
        assert!(
            matches!(&got, Err(Error::Decompress { .. })),
            "{what}: {got:?}"
        );
    }
    // Writing within its own segment (and cutting its own tail) is fine,
    // and every byte it wrote counts.
    let (got, decoded) = run(Escapes {
        cut: Some(8),
        own_segment: false,
        write: 16,
    });
    assert!(got.unwrap().is_none());
    assert_eq!(decoded, 8 + 16);
}

#[test]
fn a_codecs_range_is_checked_and_sliced_from_its_input() {
    // Round 5: a codec's returned bytes were trusted to be a slice of its
    // input. Now it can only name a range, which must lie in the input,
    // with nothing written, within the block's cap.
    let m = Meter::unlimited();
    let input = Bytes::from(vec![7u8; 100]);
    let run = |codec: Fake, cap: u64| {
        let mut out = DecodeBuf::new(&m, 0, u64::MAX, LimitKind::BlockOutput).unwrap();
        out.extend_from_slice(&[1, 2, 3]).unwrap(); // an earlier block of the run
        let got = decode_block(Codec::Other(9), &codec, &input, 100, &mut out, cap);
        (got, out.len())
    };
    let (got, len) = run(
        Fake {
            write: 0,
            range: Some(10..60),
        },
        100,
    );
    let slice = got.unwrap().unwrap();
    assert_eq!(slice.len(), 50);
    assert_eq!(slice.as_ptr(), input[10..].as_ptr(), "a zero-copy slice");
    assert_eq!(len, 3);
    let (got, len) = run(
        Fake {
            write: 40,
            range: None,
        },
        100,
    );
    assert!(got.unwrap().is_none());
    assert_eq!(len, 43);
    for (codec, cap, what) in [
        (
            Fake {
                write: 0,
                range: Some(90..101),
            },
            1000,
            "past the end",
        ),
        (
            Fake {
                write: 0,
                range: Some(1000..2000),
            },
            1 << 20,
            "outside",
        ),
        (
            Fake {
                write: 0,
                range: Some(Range { start: 60, end: 10 }),
            },
            100,
            "reversed",
        ),
        (
            Fake {
                write: 5,
                range: Some(0..10),
            },
            100,
            "written and a range",
        ),
    ] {
        let (got, _) = run(codec, cap);
        assert!(
            matches!(
                got,
                Err(Error::Decompress {
                    codec: Codec::Other(9),
                    ..
                })
            ),
            "{what}: {got:?}"
        );
    }
    let (got, _) = run(
        Fake {
            write: 0,
            range: Some(0..100),
        },
        99,
    );
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::BlockOutput && l.value == 100),
        "{got:?}"
    );
    // The segment was begun at the block's cap: a codec writing past it
    // is refused.
    let (got, _) = run(
        Fake {
            write: 50,
            range: None,
        },
        49,
    );
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::BlockOutput),
        "{got:?}"
    );
}

#[test]
fn every_id_has_one_canonical_codec() {
    for id in 0..64u32 {
        let c = Codec::from_flags(id);
        assert!(c.is_canonical(), "{c:?}");
        assert_eq!(u32::from(c.id()), id);
        assert_eq!(Codec::from_flags(u32::from(c.id())), c);
    }
    for n in [0u8, 1, 2, 3, 4, 64, 200, 255] {
        assert!(!Codec::Other(n).is_canonical(), "Other({n})");
    }
}

#[test]
fn an_alias_neither_replaces_nor_finds_a_codec() {
    let mut c = Codecs::unitypy();
    let lzma = c.get(Codec::Lzma).unwrap().clone();
    // `Other(1)` is not LZMA: registering it must not replace slot 1.
    let _ = c.set(Codec::Other(1), Arc::new(Lz4));
    assert!(Arc::ptr_eq(c.get(Codec::Lzma).unwrap(), &lzma));
    assert!(c.get(Codec::Other(1)).is_none());
    assert_eq!(
        format!("{c:?}"),
        "Codecs { registered: [0, 1, 2, 3], stored_builtin: true }"
    );
}

#[test]
fn an_id_past_the_registry_is_handed_back() {
    let mut c = Codecs::empty();
    assert_eq!(
        c.set(Codec::Other(64), Arc::new(Lz4)),
        Err(Codec::Other(64))
    );
    assert_eq!(
        c.set(Codec::Other(200), Arc::new(Lz4)),
        Err(Codec::Other(200))
    );
    assert_eq!(c.set(Codec::Other(5), Arc::new(Lz4)), Ok(()));
    assert!(c.get(Codec::Other(5)).is_some());
}
