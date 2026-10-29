//! The decoders under test. Each takes one Unity LZMA block (5 property bytes,
//! then raw LZMA1 with no size and, in practice, no end marker) and fills an
//! output buffer of the size the blocks-info gives, then stops.
//!
//! That is narrower than `UnityPy`'s contract. Its `decompress_lzma` ignores
//! the blocks-info size and returns whatever liblzma yields from all of the
//! block, and liblzma validates the properties (lc + lp <= 4, pb <= 4) but
//! takes any 32-bit dictionary size. The two agree whenever a stream yields
//! exactly the blocks-info size, as every corpus block does. The tests below
//! pin four other shapes, and `unitypy_lzma.py synth` gives them to the
//! oracle (docs/spikes/lzma.md, section 2). On two of them every decoder here
//! differs from `UnityPy`:
//! - a truncated stream: `UnityPy` returns the bytes it could decode, and
//!   every decoder here fails;
//! - a stream longer than the blocks-info size: `UnityPy` returns all of it.
//!   liblzma here returns its first bytes and ignores the rest. lzma-rust2
//!   does the same when the size falls between two LZMA symbols, and fails
//!   when it falls inside a match.
//!
//! On the other two the lzma-rust2 crate alone would differ, and its paths
//! here adjust so that every decoder does what `UnityPy` does:
//! - properties with lc + lp > 4: `UnityPy` raises. liblzma rejects them
//!   itself; lzma-rust2 accepts lc up to 8, so its paths check first
//!   ([`check_lc_lp`]);
//! - a dictionary size above 0xFFFFFFF0: `UnityPy` decodes the block. So does
//!   liblzma; lzma-rust2 refuses the size, so its paths clamp it first
//!   ([`dict_size`]).

/// A decoder under test, by the name the reports use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// The `liblzma` crate: xz's C liblzma, built from source (`static`), through
    /// the raw LZMA1 decoder, which is the path `UnityPy` takes through Python's
    /// `lzma` module.
    #[cfg(feature = "liblzma")]
    Liblzma,
    /// `lzma-rust2`'s pull API (`LzmaReader`, a `std::io::Read`).
    #[cfg(feature = "lzma-rust2")]
    LzmaRust2Reader,
    /// `lzma-rust2`'s push API (`LzmaStream`), the same shape as `lzma_code`.
    #[cfg(feature = "lzma-rust2")]
    LzmaRust2Stream,
}

impl Codec {
    /// Every decoder this build carries.
    pub const ALL: &[Codec] = &[
        #[cfg(feature = "liblzma")]
        Codec::Liblzma,
        #[cfg(feature = "lzma-rust2")]
        Codec::LzmaRust2Reader,
        #[cfg(feature = "lzma-rust2")]
        Codec::LzmaRust2Stream,
    ];

    pub fn name(self) -> &'static str {
        match self {
            #[cfg(feature = "liblzma")]
            Codec::Liblzma => "liblzma",
            #[cfg(all(feature = "lzma-rust2", feature = "lzma-rust2-optimization"))]
            Codec::LzmaRust2Reader => "lzma-rust2-reader",
            #[cfg(all(feature = "lzma-rust2", feature = "lzma-rust2-optimization"))]
            Codec::LzmaRust2Stream => "lzma-rust2-stream",
            #[cfg(all(feature = "lzma-rust2", not(feature = "lzma-rust2-optimization")))]
            Codec::LzmaRust2Reader => "lzma-rust2-reader-safe",
            #[cfg(all(feature = "lzma-rust2", not(feature = "lzma-rust2-optimization")))]
            Codec::LzmaRust2Stream => "lzma-rust2-stream-safe",
        }
    }

    /// Decodes `block` into all of `out`, or fails.
    // A build with no decoder (the binary-size baseline) never reads `out`.
    #[cfg_attr(
        not(any(feature = "liblzma", feature = "lzma-rust2")),
        allow(unused_variables)
    )]
    pub fn decode(self, block: &[u8], out: &mut [u8]) -> Result<(), String> {
        if block.len() < 5 {
            return Err("block shorter than its 5 property bytes".into());
        }
        match self {
            #[cfg(feature = "liblzma")]
            Codec::Liblzma => liblzma(block, out),
            #[cfg(feature = "lzma-rust2")]
            Codec::LzmaRust2Reader => lzma_rust2_reader(block, out),
            #[cfg(feature = "lzma-rust2")]
            Codec::LzmaRust2Stream => lzma_rust2_stream(block, out),
        }
    }
}

/// The block's dictionary size, clamped for lzma-rust2. liblzma takes any
/// 32-bit size ("All dictionary sizes are accepted", `lzma_decoder.c`), so
/// `UnityPy` decodes a block that declares more than `DICT_SIZE_MAX`
/// (0xFFFFFFF0). lzma-rust2's constructors refuse that ("dict size too
/// large") before they clamp the size to the output size themselves. Only
/// such a size is clamped here, first and the same way: to the output size,
/// but at least 4096. No decoder can see the difference, since a match never
/// reaches back past the start of the output. Every other size passes as is.
#[cfg(feature = "lzma-rust2")]
fn dict_size(block: &[u8], out_len: usize) -> u32 {
    let declared = u32::from_le_bytes(block[1..5].try_into().unwrap());
    if declared <= lzma_rust2::DICT_SIZE_MAX {
        return declared;
    }
    let enough = u32::try_from(out_len)
        .unwrap_or(u32::MAX)
        .max(lzma_rust2::DICT_SIZE_MIN);
    enough.min(lzma_rust2::DICT_SIZE_MAX)
}

/// liblzma, and so `UnityPy`, rejects lc + lp > 4 (`is_lclppb_valid`).
/// lzma-rust2 takes lc up to 8 and lp up to 4 and would decode such a block,
/// so a reader built on it needs this check to fail where `UnityPy` fails.
/// The split of the byte is `UnityPy`'s: lc = props % 9, lp = props / 9 % 5.
#[cfg(feature = "lzma-rust2")]
fn check_lc_lp(props: u8) -> Result<(), String> {
    let (lc, lp) = (props % 9, props / 9 % 5);
    if lc + lp > 4 {
        return Err(format!(
            "properties {props:#04x}: lc {lc} + lp {lp} is over 4, which liblzma rejects"
        ));
    }
    Ok(())
}

#[cfg(feature = "liblzma")]
fn liblzma(block: &[u8], out: &mut [u8]) -> Result<(), String> {
    use liblzma::stream::{Action, Filters, Stream};
    let mut filters = Filters::new();
    filters
        .lzma1_properties(&block[..5])
        .map_err(|e| e.to_string())?;
    let mut stream = Stream::new_raw_decoder(&filters).map_err(|e| e.to_string())?;
    let input = &block[5..];
    // Run, not Finish, as Python's LZMADecompressor does: a stream with no end
    // marker never reaches StreamEnd, so it is done when the output is full.
    loop {
        let (read, written) = (stream.total_in(), stream.total_out());
        let (in_pos, out_pos) = (as_usize(read), as_usize(written));
        if out_pos == out.len() {
            return Ok(());
        }
        stream
            .process(&input[in_pos..], &mut out[out_pos..], Action::Run)
            .map_err(|e| e.to_string())?;
        if stream.total_in() == read && stream.total_out() == written {
            return Err(format!("stalled at {written} of {} bytes", out.len()));
        }
    }
}

#[cfg(feature = "lzma-rust2")]
fn lzma_rust2_reader(block: &[u8], out: &mut [u8]) -> Result<(), String> {
    use std::io::Read;
    check_lc_lp(block[0])?;
    let (size, dict) = (out.len() as u64, dict_size(block, out.len()));
    let mut reader =
        lzma_rust2::LzmaReader::new_with_props(&block[5..], size, block[0], dict, None)
            .map_err(|e| e.to_string())?;
    reader.read_exact(out).map_err(|e| e.to_string())
}

#[cfg(feature = "lzma-rust2")]
fn lzma_rust2_stream(block: &[u8], out: &mut [u8]) -> Result<(), String> {
    use lzma_rust2::{Action, LzmaStream};
    check_lc_lp(block[0])?;
    let (size, dict) = (out.len() as u64, dict_size(block, out.len()));
    let mut stream =
        LzmaStream::new_with_props(size, block[0], dict, None).map_err(|e| e.to_string())?;
    let (mut in_pos, mut out_pos) = (0, 0);
    while out_pos < out.len() {
        let step = stream
            .process(&block[5 + in_pos..], &mut out[out_pos..], Action::Finish)
            .map_err(|e| e.to_string())?;
        if step.bytes_consumed == 0 && step.bytes_produced == 0 {
            return Err(format!("stalled at {out_pos} of {} bytes", out.len()));
        }
        in_pos += step.bytes_consumed;
        out_pos += step.bytes_produced;
    }
    Ok(())
}

#[cfg(feature = "liblzma")]
fn as_usize(n: u64) -> usize {
    usize::try_from(n).expect("a position inside an in-memory buffer")
}

/// The four shapes of the module documentation; `unitypy_lzma.py`'s tests pin
/// `UnityPy`'s side of each.
#[cfg(all(test, feature = "liblzma"))]
mod tests {
    use super::Codec;
    use crate::synth::{lzma_block, sample};

    #[test]
    fn a_truncated_stream_fails_every_decoder() {
        let data = sample(200_000);
        let block = lzma_block(&data, 0x5D);
        let cut = &block[..block.len() / 2];
        for &codec in Codec::ALL {
            let mut out = vec![0u8; data.len()];
            let outcome = codec.decode(cut, &mut out);
            assert!(
                outcome.is_err(),
                "{} decoded a truncated stream",
                codec.name()
            );
        }
    }

    /// A buffer shorter than the stream. `sample` repeats one word in the
    /// first half of every 4096 bytes, which the encoder writes as long
    /// matches, and is random in the second half, which it writes as
    /// literals. 100,000 falls in a run (100,000 % 4096 = 1,696), 101,304 in
    /// a random span (3,000).
    #[test]
    fn a_stream_longer_than_the_buffer() {
        let data = sample(200_000);
        let block = lzma_block(&data, 0x5D);
        for &codec in Codec::ALL {
            for size in [100_000, 101_304] {
                let mut out = vec![0u8; size];
                let outcome = codec.decode(&block, &mut out);
                let name = codec.name();
                if outcome.is_ok() {
                    assert!(out == data[..size], "{name} at {size}: other bytes");
                }
                // liblzma stops wherever the buffer ends. lzma-rust2 fails
                // when its declared size ends inside a match: "end reached
                // but not decoder finished".
                let inside_a_match = size == 100_000;
                let fails = name.starts_with("lzma-rust2") && inside_a_match;
                assert_eq!(outcome.is_err(), fails, "{name} at {size}: {outcome:?}");
            }
        }
    }

    /// Bytes under 8 put every literal in context 0 for lc 3 and for lc 5
    /// alike (the context is the previous byte shifted right by 8 - lc), so a
    /// stream liblzma encodes with lc 3 decodes to the same bytes as lc 5,
    /// properties 0x5F. That makes a block liblzma can neither write nor read.
    #[test]
    fn lc_plus_lp_over_4_fails_every_decoder() {
        let data: Vec<u8> = sample(200_000).iter().map(|b| b & 7).collect();
        let mut block = lzma_block(&data, 0x5D);
        block[0] = 0x5F; // lc 5, lp 0, pb 2
        for &codec in Codec::ALL {
            let mut out = vec![0u8; data.len()];
            let outcome = codec.decode(&block, &mut out);
            assert!(outcome.is_err(), "{} decoded properties 0x5F", codec.name());
        }
        // Without the check, lzma-rust2 decodes it.
        #[cfg(feature = "lzma-rust2")]
        {
            use std::io::Read;
            let dict = crate::synth::DICT;
            let size = data.len() as u64;
            let mut reader =
                lzma_rust2::LzmaReader::new_with_props(&block[5..], size, 0x5F, dict, None)
                    .unwrap();
            let mut out = vec![0u8; data.len()];
            reader.read_exact(&mut out).unwrap();
            assert!(out == data, "lzma-rust2 read 0x5F as other bytes");
        }
    }

    /// liblzma, and so `UnityPy`, takes any 32-bit dictionary size, and the
    /// size only bounds how far back a match may reach, so a stream that
    /// declares a bigger one decodes to the same bytes. lzma-rust2's
    /// constructors refuse one above 0xFFFFFFF0.
    #[test]
    fn a_dictionary_size_above_0xfffffff0_decodes_in_every_decoder() {
        let data = sample(200_000);
        let block = lzma_block(&data, 0x5D);
        for declared in [0xFFFF_FFF0u32, 0xFFFF_FFF1, u32::MAX] {
            let mut big = block.clone();
            big[1..5].copy_from_slice(&declared.to_le_bytes());
            for &codec in Codec::ALL {
                let mut out = vec![0u8; data.len()];
                let name = codec.name();
                let outcome = codec.decode(&big, &mut out);
                assert!(outcome.is_ok(), "{name}, {declared:#x}: {outcome:?}");
                assert!(out == data, "{name}, {declared:#x}: other bytes");
            }
            // Without the clamp, lzma-rust2 refuses the size.
            #[cfg(feature = "lzma-rust2")]
            {
                let size = data.len() as u64;
                let alone =
                    lzma_rust2::LzmaReader::new_with_props(&big[5..], size, 0x5D, declared, None);
                let refused = alone.is_err_and(|e| e.to_string().contains("dict size too large"));
                assert_eq!(refused, declared > 0xFFFF_FFF0, "{declared:#x}");
            }
        }
    }

    #[cfg(feature = "lzma-rust2")]
    #[test]
    fn the_dictionary_clamp_keeps_what_fits() {
        let block = |dict: u32| {
            let mut block = vec![0x5D];
            block.extend_from_slice(&dict.to_le_bytes());
            block
        };
        // Sizes lzma-rust2 takes pass as they are.
        assert_eq!(super::dict_size(&block(1 << 19), 200_000), 1 << 19);
        assert_eq!(super::dict_size(&block(100), 0), 100);
        assert_eq!(super::dict_size(&block(0xFFFF_FFF0), 200_000), 0xFFFF_FFF0);
        // Larger ones become the output size, at least 4096, at most the cap.
        assert_eq!(super::dict_size(&block(0xFFFF_FFF1), 200_000), 200_000);
        assert_eq!(super::dict_size(&block(u32::MAX), 0), 4096);
        let all = usize::try_from(u32::MAX).unwrap();
        assert_eq!(super::dict_size(&block(u32::MAX), all), 0xFFFF_FFF0);
    }

    #[cfg(feature = "lzma-rust2")]
    #[test]
    fn the_lc_lp_check_reads_the_byte_as_it_is_written() {
        // The properties byte is (pb * 5 + lp) * 9 + lc.
        for pb in 0..5u8 {
            for lp in 0..5u8 {
                for lc in 0..9u8 {
                    let props = (pb * 5 + lp) * 9 + lc;
                    let ok = super::check_lc_lp(props).is_ok();
                    assert_eq!(ok, lc + lp <= 4, "lc {lc}, lp {lp}, pb {pb}");
                }
            }
        }
    }
}
