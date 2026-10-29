//! Synthetic bundles and LZMA blocks for the tests, built in code: no game
//! data, and nothing read from disk.

/// How a test bundle is laid out. `align` and `pad` are where `UnityPy` reads
/// the 16-byte alignment after the header and the padding before the first
/// data block. The bundle is built that way whatever its flags say, so a
/// finder that applies the wrong rule reads the wrong bytes.
pub struct Header<'a> {
    pub format: u32,
    pub engine: &'a str,
    pub flags: u32,
    pub align: bool,
    pub pad: bool,
}

/// A bundle holding `blocks` (stored bytes, uncompressed size, block flags)
/// and no directory nodes. Returns the file and where each block starts.
pub fn bundle(header: &Header, blocks: &[(&[u8], usize, u16)]) -> (Vec<u8>, Vec<usize>) {
    let be32 = |n: usize| u32::try_from(n).unwrap().to_be_bytes();
    let mut info = vec![0u8; 16]; // uncompressed data hash
    info.extend_from_slice(&be32(blocks.len()));
    for &(stored, uncompressed, flags) in blocks {
        info.extend_from_slice(&be32(uncompressed));
        info.extend_from_slice(&be32(stored.len()));
        info.extend_from_slice(&flags.to_be_bytes());
    }
    info.extend_from_slice(&be32(0)); // no directory nodes
    let stored_info = match header.flags & 0x3F {
        0 => info.clone(),
        // LZ4HC differs from LZ4 only in how hard the writer searched.
        2 | 3 => lz4_flex::block::compress(&info),
        other => panic!("blocks-info compression {other}"),
    };

    let mut file = b"UnityFS\0".to_vec();
    file.extend_from_slice(&header.format.to_be_bytes());
    file.extend_from_slice(b"5.x.x\0");
    file.extend_from_slice(header.engine.as_bytes());
    file.push(0);
    file.extend_from_slice(&0u64.to_be_bytes()); // total size, never read
    file.extend_from_slice(&be32(stored_info.len()));
    file.extend_from_slice(&be32(info.len()));
    file.extend_from_slice(&header.flags.to_be_bytes());
    if header.align {
        pad16(&mut file);
    }
    let at_end = header.flags & 0x80 != 0;
    if !at_end {
        file.extend_from_slice(&stored_info);
    }
    if header.pad {
        pad16(&mut file);
    }
    let mut starts = Vec::new();
    for &(stored, ..) in blocks {
        starts.push(file.len());
        file.extend_from_slice(stored);
    }
    if at_end {
        file.extend_from_slice(&stored_info);
    }
    (file, starts)
}

fn pad16(file: &mut Vec<u8>) {
    file.resize(file.len().next_multiple_of(16), 0);
}

/// `len` bytes: runs of one short word, for long matches, between
/// pseudo-random spans, for literals. In every 4096 bytes the first 2048 are
/// the run. `unitypy_lzma.py synth` makes the same bytes.
#[cfg(feature = "liblzma")]
pub fn sample(len: usize) -> Vec<u8> {
    let mut seed = 0x2545_F491u32;
    (0..len)
        .map(|i| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            if i % 4096 < 2048 {
                b"unityrs"[i % 7]
            } else {
                seed.to_be_bytes()[0]
            }
        })
        .collect()
}

/// The corpus blocks' dictionary size, 512 KiB.
#[cfg(feature = "liblzma")]
pub const DICT: u32 = 1 << 19;

/// A Unity LZMA block: the properties byte `props`, the dictionary size, then
/// `data` as raw LZMA1 from liblzma's encoder. `props` 0x5D (lc 3, lp 0,
/// pb 2) is what `UnityPy`'s `compress_lzma` writes.
#[cfg(feature = "liblzma")]
pub fn lzma_block(data: &[u8], props: u8) -> Vec<u8> {
    use liblzma::stream::{Action, Filters, LzmaOptions, Status, Stream};
    let mut options = LzmaOptions::new_preset(6).unwrap();
    options
        .dict_size(DICT)
        .literal_context_bits(u32::from(props % 9))
        .literal_position_bits(u32::from(props / 9 % 5))
        .position_bits(u32::from(props / 45));
    let mut filters = Filters::new();
    filters.lzma1(&options);
    let mut stream = Stream::new_raw_encoder(&filters).unwrap();
    let mut block = vec![props];
    block.extend_from_slice(&DICT.to_le_bytes());
    block.reserve(data.len());
    loop {
        let read = usize::try_from(stream.total_in()).unwrap();
        if block.len() == block.capacity() {
            block.reserve(1 << 16);
        }
        let status = stream
            .process_vec(&data[read..], &mut block, Action::Finish)
            .unwrap();
        if status == Status::StreamEnd {
            return block;
        }
    }
}
