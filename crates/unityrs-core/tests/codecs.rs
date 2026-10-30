//! The three codecs against liblz4 1.9.4's and liblzma's verdicts as UnityPy
//! 1.25.2 sees them (plan section 7.4, codecs).

use std::path::Path;
use std::sync::Arc;

use serde_json::Value as J;
use unityrs_core::Bytes;
use unityrs_core::budget::{BudgetExceeded, DecodeBuf, Reservation, Unbounded};
use unityrs_core::codec::{BlockCodec, Codec, Codecs, Decoded, Lz4, Lzma, Stored, decode_block};
use unityrs_core::error::{Error, LimitKind};
use unityrs_core::limits::Meter;
use unityrs_testkit::address_space;
use unityrs_testkit::bundle::{LZMA_DICT, TestCodec, compress};

fn load(dir: &str, name: &str) -> J {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(dir)
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

fn meter(limit: u64) -> Arc<Meter> {
    Meter::new(limit, Arc::new(Unbounded), Reservation::Input)
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// Decodes `input` with `codec` through `decode_block` at a cap, on its own
/// meter: the bytes, or the error.
fn run(codec: &dyn BlockCodec, input: &[u8], declared: u32, cap: u64) -> Result<Vec<u8>, Error> {
    run_on(&Meter::unlimited(), codec, input, declared, cap)
}

fn run_on(
    meter: &Arc<Meter>,
    codec: &dyn BlockCodec,
    input: &[u8],
    declared: u32,
    cap: u64,
) -> Result<Vec<u8>, Error> {
    let input = Bytes::copy_from_slice(input);
    let mut out = DecodeBuf::new(meter, 0, u64::MAX, LimitKind::BlockOutput)?;
    match decode_block(Codec::Other(9), codec, &input, declared, &mut out, cap)? {
        Some(slice) => Ok(slice.to_vec()),
        None => Ok(out.as_slice().to_vec()),
    }
}

/// `lz4_blocks.json` (python-lz4 4.4.4 over liblz4 1.9.4): every block at
/// every declared size gives liblz4's verdict and output, offset 0 included.
#[test]
fn lz4_replays_liblz4_on_every_boundary_block() {
    let doc = load("scaffold", "lz4_blocks.json");
    let mut checked = 0;
    for c in doc["cases"].as_array().unwrap() {
        let block = hex(c["block"].as_str().unwrap());
        for r in c["declared"].as_array().unwrap() {
            let declared = u32::try_from(r[0].as_u64().unwrap()).unwrap();
            let got = run(&Lz4, &block, declared, u64::MAX);
            match r[1].as_u64() {
                Some(n) => {
                    let got = got.unwrap_or_else(|e| panic!("{c} at {declared}: {e}"));
                    assert_eq!(got.len(), usize::try_from(n).unwrap(), "{c} at {declared}");
                    assert_eq!(got, hex(c["output"].as_str().unwrap()), "{c} at {declared}");
                }
                None => assert!(
                    matches!(got, Err(Error::Decompress { .. })),
                    "{c} at {declared}: {got:?}"
                ),
            }
            checked += 1;
        }
    }
    assert!(checked > 8000, "{checked}");
}

#[test]
fn lz4_takes_the_declared_size_as_a_maximum() {
    let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
    let block = compress(TestCodec::Lz4, &data);
    // Exact, larger (a short output is accepted), smaller (refused).
    assert_eq!(run(&Lz4, &block, 5000, u64::MAX).unwrap(), data);
    assert_eq!(run(&Lz4, &block, 9000, u64::MAX).unwrap(), data);
    assert!(matches!(
        run(&Lz4, &block, 4999, u64::MAX),
        Err(Error::Decompress { .. })
    ));
    // An empty block is refused whatever the declared size.
    for declared in [0, 1, 100] {
        assert!(matches!(
            run(&Lz4, &[], declared, u64::MAX),
            Err(Error::Decompress { .. })
        ));
    }
    // Above `i32::MAX`, python-lz4 raises `OverflowError` before decoding.
    let one = [0x10, 0x01];
    assert_eq!(run(&Lz4, &one, 0x7FFF_FFFF, u64::MAX).unwrap(), [1]);
    assert!(matches!(
        run(&Lz4, &one, 0x8000_0000, u64::MAX),
        Err(Error::Decompress { .. })
    ));
}

#[test]
fn lz4_allocates_the_output_it_decodes_not_the_declared_size() {
    // A two-byte block declaring 2 GiB - 1 charges its one byte.
    let meter = meter(4096);
    assert_eq!(
        run_on(&meter, &Lz4, &[0x10, 0x01], 0x7FFF_FFFF, u64::MAX).unwrap(),
        [1]
    );
    // The block's cap refuses an output that passes it.
    let data = vec![7u8; 1000];
    let block = compress(TestCodec::Lz4, &data);
    let got = run(&Lz4, &block, 1000, 999);
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::BlockOutput),
        "{got:?}"
    );
    assert_eq!(run(&Lz4, &block, 1000, 1000).unwrap(), data);
}

fn lzma_stream(data: &[u8]) -> Vec<u8> {
    compress(TestCodec::Lzma, data)
}

#[test]
fn lzma_runs_to_the_end_of_the_input_whatever_the_declared_size() {
    let data: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 253) as u8).collect();
    let block = lzma_stream(&data);
    for declared in [0, 1, 100_000, 200_000, 400_000, u32::MAX] {
        assert_eq!(
            run(&Lzma, &block, declared, u64::MAX).unwrap(),
            data,
            "{declared}"
        );
    }
    // Truncated: what decoded, no error.
    let cut = &block[..block.len() / 2];
    let got = run(&Lzma, cut, 200_000, u64::MAX).unwrap();
    assert!(!got.is_empty() && got.len() < data.len());
    assert_eq!(got[..], data[..got.len()]);
    // Exactly the five header bytes: nothing, no error.
    assert_eq!(run(&Lzma, &block[..5], 10, u64::MAX).unwrap(), b"");
    // Fewer than five bytes raise (`struct.unpack("<BI", ...)`).
    for n in 0..5 {
        assert!(matches!(
            run(&Lzma, &block[..n], 10, u64::MAX),
            Err(Error::Decompress { .. })
        ));
    }
}

#[test]
fn lzma_bad_properties_and_corrupt_data_raise() {
    let block = lzma_stream(b"hello hello hello hello");
    // lc + lp > 4 (lc 4, lp 1: 0x5D -> 9 * (5 * 0 + 1) + 4).
    let mut bad = block.clone();
    bad[0] = 13;
    assert!(matches!(
        run(&Lzma, &bad, 23, u64::MAX),
        Err(Error::Decompress { .. })
    ));
    // pb 5 (props 225 and up).
    bad[0] = 225;
    assert!(matches!(
        run(&Lzma, &bad, 23, u64::MAX),
        Err(Error::Decompress { .. })
    ));
    // A dictionary size near 4 GiB decodes as the named one would.
    let mut huge = block.clone();
    huge[1..5].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        run(&Lzma, &huge, 23, u64::MAX).unwrap(),
        b"hello hello hello hello"
    );
}

#[test]
fn lzma_matches_beyond_the_first_dictionary_decode_again_with_a_larger_one() {
    // 3 MiB of noise repeated: the second copy matches 3 MiB back, past the
    // first attempt's 4 KiB dictionary (a declared size of 0).
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let noise: Vec<u8> = (0..3 << 20)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed.to_le_bytes()[0]
        })
        .collect();
    let mut data = noise.clone();
    data.extend_from_slice(&noise);
    let mut opts = liblzma::stream::LzmaOptions::new_preset(0).unwrap();
    opts.dict_size(8 << 20);
    let mut filters = liblzma::stream::Filters::new();
    filters.lzma1(&opts);
    let mut enc = liblzma::stream::Stream::new_raw_encoder(&filters).unwrap();
    let mut body = Vec::with_capacity(data.len() + 4096);
    enc.process_vec(&data, &mut body, liblzma::stream::Action::Run)
        .unwrap();
    while enc
        .process_vec(&[], &mut body, liblzma::stream::Action::Finish)
        .unwrap()
        != liblzma::stream::Status::StreamEnd
    {
        body.reserve(1 << 16);
    }
    assert!(body.len() < data.len() * 3 / 4, "the copy matched");
    let mut block = vec![0x5D];
    block.extend_from_slice(&(8u32 << 20).to_le_bytes());
    block.extend_from_slice(&body);
    assert_eq!(run(&Lzma, &block, 0, u64::MAX).unwrap(), data);
}

#[test]
fn lzma_output_is_capped_per_block_and_charged() {
    let data = vec![3u8; 100_000];
    let block = lzma_stream(&data);
    // Exactly at the cap fits; one byte under refuses.
    assert_eq!(run(&Lzma, &block, 0, 100_000).unwrap(), data);
    let got = run(&Lzma, &block, 0, 99_999);
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::BlockOutput),
        "{got:?}"
    );
    // A meter smaller than the output refuses it; one that holds it and the
    // dictionary does not, whatever the windows the decoder asked for.
    let small = meter(50_000);
    let got = run_on(&small, &Lzma, &block, 100_000, u64::MAX);
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::InputMemory),
        "{got:?}"
    );
    // The dictionary liblzma allocates (the declared 100,000 bytes here) is
    // charged beside the output (C request 1): a meter that holds the output
    // alone refuses the block, and returns every byte.
    let output_only = meter(100_000 + 64);
    let got = run_on(&output_only, &Lzma, &block, 100_000, u64::MAX);
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::InputMemory),
        "{got:?}"
    );
    assert_eq!(output_only.used(), 0);
    let fits = meter(2 * (100_000 + 64));
    assert_eq!(
        run_on(&fits, &Lzma, &block, 100_000, u64::MAX).unwrap(),
        data
    );
    assert_eq!(
        fits.used(),
        0,
        "the dictionary's charge ends with the decode"
    );
}

const DICTIONARY_PROBE: &str = "UNITYRS_CORE_LZMA_DICTIONARY_PROBE";

/// Run by `liblzmas_refused_dictionary_is_the_meters_reservation_refused`,
/// in a child: a block naming an 8 MiB dictionary, declared 8 MiB (so the
/// first attempt's dictionary is 8 MiB, `Lzma`'s docs), decoded on a meter
/// of each reservation under an address-space cap 4 MiB above what the
/// child maps; then, the cap lifted, decoded.
#[test]
#[ignore = "run by liblzmas_refused_dictionary_is_the_meters_reservation_refused"]
fn lzma_dictionary_probe() {
    if std::env::var(DICTIONARY_PROBE).is_err() {
        return;
    }
    let mut block = lzma_stream(b"hello hello hello hello");
    block[1..5].copy_from_slice(&(8_u32 << 20).to_le_bytes());
    let input = Bytes::from(block);
    let decode = |meter: &Arc<Meter>| {
        let mut out = DecodeBuf::new(meter, 0, u64::MAX, LimitKind::BlockOutput).unwrap();
        Lzma.decode(&input, 8 << 20, &mut out).map(|_| out.len())
    };
    for what in [
        Reservation::Input,
        Reservation::Dependency,
        Reservation::Provider,
    ] {
        let meter = Meter::new(u64::MAX, Arc::new(Unbounded), what);
        let got = {
            let _cap = address_space::cap_above_current(4 << 20).unwrap();
            decode(&meter)
        };
        match got {
            Err(Error::Budget(b)) => {
                // 8 MiB in one piece, glibc's `mmap`ped chunk: the request
                // and its 8-byte header rounded up to a 4 KiB page.
                let want = BudgetExceeded {
                    requested: (8 << 20) + 4096,
                    what,
                };
                assert_eq!(b, want, "{what:?}");
            }
            got => panic!("{what:?}: not the reservation's refusal: {got:?}"),
        }
        assert_eq!(meter.used(), 0, "{what:?}: the dictionary's charge ends");
    }
    let meter = Meter::new(u64::MAX, Arc::new(Unbounded), Reservation::Provider);
    assert_eq!(decode(&meter).unwrap(), 23, "the same block decodes");
}

/// Round 7 (claude7-a L1): liblzma's `LZMA_MEM_ERROR`, the C allocator
/// refusing the dictionary the meter granted, is that meter's reservation
/// refused (`Error::Budget`), for every reservation; only a test of the
/// mapping, with a reservation it chose, covered it, so a codec that named
/// another reservation passed every test. Here the C allocator itself
/// refuses, in a child under an address-space cap.
#[test]
fn liblzmas_refused_dictionary_is_the_meters_reservation_refused() {
    if !address_space::SUPPORTED {
        eprintln!("skipped: no address-space cap on this target");
        return;
    }
    address_space::run("lzma_dictionary_probe", DICTIONARY_PROBE, "1");
}

#[test]
fn stored_names_its_input() {
    let input = Bytes::from_static(b"abcdef");
    let m = Meter::unlimited();
    let mut out = DecodeBuf::new(&m, 0, u64::MAX, LimitKind::BlockOutput).unwrap();
    assert_eq!(
        Stored.decode(&input, 99, &mut out).unwrap(),
        Decoded::Input(0..6)
    );
    assert_eq!(run(&Stored, b"abc", 0, 3).unwrap(), b"abc");
    assert!(run(&Stored, b"abc", 0, 2).is_err());
}

#[test]
fn unitypy_registers_none_lzma_and_both_lz4s_only() {
    let c = Codecs::unitypy();
    for which in [Codec::None, Codec::Lzma, Codec::Lz4, Codec::Lz4Hc] {
        assert!(c.get(which).is_some(), "{which:?}");
    }
    assert!(c.get(Codec::Lzham).is_none());
    for n in 5..64 {
        assert!(c.get(Codec::Other(n)).is_none());
    }
    let _ = LZMA_DICT;
}
