//! Test-only inverse and synthetic inputs. No real key or bundle is used.
use aes::Aes128;
use aes::cipher::{BlockEncrypt, KeyInit};
use serde_json::{Value, json};
use unityrs_testkit::bundle::{TestCodec, compress};
use unityrs_testkit::writer::ByteWriter;

pub const KEY: &[u8; 16] = b"synthetic-key-01";
pub const INDEX: [u8; 16] = [3, 10, 1, 8, 15, 6, 13, 4, 11, 2, 9, 0, 7, 14, 5, 12];
pub const SUBSTITUTE: [u8; 16] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 0];

pub fn hex(data: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for b in data {
        write!(&mut s, "{b:02x}").unwrap();
    }
    s
}

pub fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn vector(w: &mut ByteWriter, plain: &[u8; 16], seed: u8) {
    let vector = [seed; 16];
    let mut mask = vector.into();
    Aes128::new(&(*KEY).into()).encrypt_block(&mut mask);
    for (p, m) in plain.iter().zip(mask) {
        w.u8(p ^ m);
    }
    w.bytes(&vector).u8(seed ^ 0xFF);
}

pub fn header(index: &[u8; 16], substitute: &[u8; 16]) -> Vec<u8> {
    let mut nibbles = index.to_vec();
    for i in 0..4 {
        for j in 0..4 {
            nibbles.push(substitute[j * 4 + i]);
        }
    }
    let mut tables = [0; 16];
    for (out, pair) in tables.iter_mut().zip(nibbles.as_chunks::<2>().0.iter()) {
        *out = pair[0] << 4 | pair[1];
    }
    let mut w = ByteWriter::be();
    w.u32(0x1020_3040);
    vector(&mut w, &tables, 0x32);
    vector(&mut w, b"#$unity3dchina!@", 0x57);
    w.into_inner()
}

/// Inverse of `decrypt_byte`, independently solving each substituted nibble.
fn encrypt_byte(byte: u8, index: usize, table: &Tables) -> u8 {
    let b = table.substitute[((index >> 2) & 3) + 4]
        + table.substitute[index & 3]
        + table.substitute[((index >> 4) & 3) + 8]
        + table.substitute[((index >> 6) & 3) + 12];
    let nibble = |n: u8| {
        u8::try_from(
            table
                .index
                .iter()
                .position(|i| *i == n.wrapping_add(b) & 15)
                .unwrap(),
        )
        .unwrap()
    };
    nibble(byte & 15) | nibble(byte >> 4) << 4
}

/// Walk the plaintext LZ4 sequences, encrypting only token/length/offset bytes.
/// Literal bytes are unchanged. The outer sequence index restarts at block index.
pub fn encrypt(block: &mut [u8], index: usize) {
    encrypt_with(block, index, &Tables::default());
}

struct Tables {
    index: [u8; 16],
    substitute: [u8; 16],
}

impl Default for Tables {
    fn default() -> Self {
        Self {
            index: INDEX,
            substitute: SUBSTITUTE,
        }
    }
}

fn encrypt_with(block: &mut [u8], mut index: usize, table: &Tables) {
    let mut at = 0;
    while at < block.len() {
        let mut byte_index = index;
        let take = |block: &mut [u8], at: &mut usize, byte_index: &mut usize| {
            let plain = block[*at];
            block[*at] = encrypt_byte(plain, *byte_index, table);
            *at += 1;
            *byte_index = byte_index.wrapping_add(1);
            plain
        };
        let token = take(block, &mut at, &mut byte_index);
        let mut literals = usize::from(token >> 4);
        if literals == 15 {
            loop {
                let b = take(block, &mut at, &mut byte_index);
                literals += usize::from(b);
                if b != 255 {
                    break;
                }
            }
        }
        at += literals;
        if at < block.len() {
            take(block, &mut at, &mut byte_index);
            take(block, &mut at, &mut byte_index);
            if token & 15 == 15 {
                loop {
                    if take(block, &mut at, &mut byte_index) != 255 {
                        break;
                    }
                }
            }
        }
        index = index.wrapping_add(1);
    }
}

pub fn plaintext() -> Vec<u8> {
    // Repetition supplies matches and match extensions; the suffix supplies
    // literals and literal extensions. More than one sequence per block.
    (0..3072)
        .map(|i| {
            if i % 257 < 240 {
                u8::try_from(i % 7).unwrap()
            } else {
                u8::try_from(i % 251).unwrap()
            }
        })
        .collect()
}

pub fn bundle(
    engine: &[u8],
    format: u32,
    mask: u32,
    info_encrypted: bool,
    at_end: bool,
    stored: bool,
) -> Vec<u8> {
    bundle_with(
        engine,
        format,
        mask,
        info_encrypted,
        at_end,
        stored,
        Content {
            plain: &plaintext(),
            name: b"synthetic.resS",
            tables: Tables::default(),
        },
    )
}

struct Content<'a> {
    plain: &'a [u8],
    name: &'a [u8],
    tables: Tables,
}

fn bundle_with(
    engine: &[u8],
    format: u32,
    mask: u32,
    info_encrypted: bool,
    at_end: bool,
    stored: bool,
    content: Content<'_>,
) -> Vec<u8> {
    let Content {
        plain,
        name,
        tables,
    } = content;
    let mut info = ByteWriter::be();
    let chunks: Vec<_> = plain.chunks(768).collect();
    info.bytes(&[0; 16])
        .i32(i32::try_from(chunks.len()).unwrap());
    let mut data = Vec::new();
    let codec = if stored {
        TestCodec::None
    } else {
        TestCodec::Lz4
    };
    for (i, chunk) in chunks.iter().enumerate() {
        let mut block = compress(codec, chunk);
        info.u32(u32::try_from(chunk.len()).unwrap())
            .u32(u32::try_from(block.len()).unwrap())
            .u16(u16::from(codec.id()) | 0x100);
        if !stored {
            encrypt_with(&mut block, i, &tables);
        }
        data.extend(block);
    }
    info.i32(1)
        .i64(0)
        .i64(i64::try_from(plain.len()).unwrap())
        .u32(0)
        .cstr(name);
    let info = info.into_inner();
    let info_codec = if info_encrypted {
        TestCodec::Lz4
    } else {
        TestCodec::None
    };
    let mut encoded_info = compress(info_codec, &info);
    if info_encrypted {
        encrypt_with(&mut encoded_info, 0, &tables);
    }
    let mut w = ByteWriter::be();
    w.cstr(b"UnityFS").u32(format).cstr(b"5.x.x").cstr(engine);
    let size_at = w.len();
    w.i64(0)
        .u32(u32::try_from(encoded_info.len()).unwrap())
        .u32(u32::try_from(info.len()).unwrap())
        .u32(
            mask | 0x40
                | u32::from(info_codec.id())
                | if info_encrypted { 0x100 } else { 0 }
                | if at_end { 0x80 } else { 0 },
        );
    w.bytes(&header(&tables.index, &tables.substitute));
    if format >= 7 || engine == b"2019.4.15f1" {
        w.align(16);
    }
    if at_end {
        w.bytes(&data).bytes(&encoded_info);
    } else {
        w.bytes(&encoded_info).bytes(&data);
    }
    let mut data = w.into_inner();
    let size = (i64::try_from(data.len()).unwrap()).to_be_bytes();
    data[size_at..size_at + 8].copy_from_slice(&size);
    data
}

/// Both levels encrypt their info and data, using different permutations.
/// Opening the inner entry must inherit the key, but build its own tables.
pub fn nested_bundle() -> Vec<u8> {
    let inner = bundle_with(
        b"2022.3.0f1",
        7,
        0x1000,
        true,
        false,
        false,
        Content {
            plain: &plaintext(),
            name: b"synthetic.resS",
            tables: Tables {
                index: std::array::from_fn(|i| u8::try_from((i * 5 + 7) % 16).unwrap()),
                substitute: std::array::from_fn(|i| u8::try_from((i * 3 + 2) % 16).unwrap()),
            },
        },
    );
    bundle_with(
        b"2022.3.0f1",
        7,
        0x1000,
        true,
        false,
        false,
        Content {
            plain: &inner,
            name: b"inner.bundle",
            tables: Tables::default(),
        },
    )
}

pub fn inputs() -> Value {
    let mut bundles: Vec<_> = [
        ("new", b"2022.3.0f1".as_slice(), 7, 0x1000, false, false, false),
        ("info-end", b"2022.3.0f1", 8, 0x1400, true, true, false),
        ("partial-mask", b"2022.3.0f1", 7, 0x400, true, false, false),
        ("old", b"2018.4.0f1", 6, 0x200, true, false, false),
        ("old-aligned", b"2019.4.15f1", 6, 0x200, false, false, false),
        ("stored", b"2022.3.0f1", 7, 0x1000, false, false, true),
    ].into_iter().map(|(name, engine, format, mask, enc, end, stored)| json!({
        "name": name, "hex": hex(&bundle(engine, format, mask, enc, end, stored)), "plain": hex(&plaintext())
    })).collect();
    bundles.push(
        json!({"name": "nested-distinct-tables", "hex": hex(&nested_bundle()),
        "plain": hex(&plaintext()), "path": ["inner.bundle", "synthetic.resS"]}),
    );
    let identity: [u8; 16] = std::array::from_fn(|i| u8::try_from(i).unwrap());
    let bad_index = unhex("020d000a070c090f040b0a060f030406");
    let bad_sub = unhex("0a08040d0b08020a060707010a0b0104");
    let mut blocks = Vec::new();
    for (name, index, substitute, bytes, tier_u) in [
        (
            "literal-jump",
            INDEX.as_slice(),
            SUBSTITUTE.as_slice(),
            "10",
            false,
        ),
        ("literal-jump-2", &INDEX, &SUBSTITUTE, "30", false),
        ("virtual-length", &INDEX, &SUBSTITUTE, "80", false),
        ("virtual-length-2", &INDEX, &SUBSTITUTE, "81", false),
        ("virtual-length-3", &INDEX, &SUBSTITUTE, "82", false),
        ("virtual-offset", &identity, &[0; 16], "0000", false),
        ("identity-length", &identity, &[0; 16], "f0", false),
        ("tier-u", &bad_index, &bad_sub, "80", true),
    ] {
        blocks.push(
            json!({"name": name, "index":hex(index), "substitute":hex(substitute),
            "hex":bytes, "block_index":0, "tier_u":tier_u}),
        );
    }
    let full = header(&INDEX, &SUBSTITUTE);
    json!({"key": hex(KEY), "header":hex(&full), "bundles":bundles, "blocks":blocks,
        "header_lengths":[4,19,20,35,36,37,52,53,68,69,70]})
}
