//! Gate rows X3 and X4 (`docs/plans/extract.md` section 5): the encoder's
//! PNG against Pillow 9.5.0's `Image.save(fp, "PNG")` over the oracle
//! image's zlib 1.2.13, byte for byte, and the visible-pixel hash against
//! the one `tools/fixtures/imageio.py` computes from Pillow's
//! `convert("RGBA")`, on the images that script builds in code (its
//! docstring lists them). The pixels are rebuilt here from each case's kind
//! and seed with the script's generator, and checked against its sha256
//! first, so a failure says whether the image or the encoder differs.

use std::io::Read;
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};
use unityrs_imageio::{Image, Mode, Unmetered, encode, plan, visible_hash};

/// splitmix64's outputs as bytes, low byte first (the script's `Draws`).
struct Draws {
    state: u64,
    word: [u8; 8],
    used: usize,
}

impl Draws {
    fn new(seed: u64) -> Self {
        Self {
            state: seed,
            word: [0; 8],
            used: 8,
        }
    }

    fn byte(&mut self) -> u8 {
        if self.used == 8 {
            self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = self.state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            self.word = (z ^ (z >> 31)).to_le_bytes();
            self.used = 0;
        }
        self.used += 1;
        self.word[self.used - 1]
    }

    fn take(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.byte()).collect()
    }
}

/// The script's `pixels`; `prefix` is a "prefix" case's.
fn pixels(kind: &str, mode: Mode, width: u32, height: u32, seed: u64, prefix: usize) -> Vec<u8> {
    let channels = mode.channels();
    let count = width as usize * height as usize;
    let len = count * channels;
    let mut draws = Draws::new(seed);
    match kind {
        "prefix" => {
            let mut out = draws.take(prefix);
            out.resize(len, 0);
            out
        }
        "flat" => draws.take(channels).repeat(count),
        "random" => draws.take(len),
        "gradient" | "noisy" | "smooth" => {
            let [p0, p1, p2, base] = [0; 4].map(|_| u64::from(draws.byte()));
            let mut out = Vec::with_capacity(len);
            for y in 0..u64::from(height) {
                for x in 0..u64::from(width) {
                    for c in 0..channels as u64 {
                        let value = if kind == "smooth" {
                            ((x * x * p0 + y * y * p1) >> 5) + c * p2 + base
                        } else {
                            x * p0 + y * p1 + c * p2 + base
                        };
                        out.push(value.to_le_bytes()[0]);
                    }
                }
            }
            if kind == "noisy" {
                for (value, noise) in out.iter_mut().zip(draws.take(len)) {
                    *value = value.wrapping_add(noise & 3);
                }
            }
            out
        }
        "sparse" => draws
            .take(2 * len)
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&[first, second]| if first < 32 { second } else { 0 })
            .collect(),
        "stripes" => {
            let row = draws.take(width as usize * channels);
            (0..height)
                .flat_map(|y| {
                    let add = (y / 2).to_le_bytes()[0];
                    row.iter().map(move |&v| v.wrapping_add(add))
                })
                .collect()
        }
        "alpha" => {
            let mut out = Vec::with_capacity(len);
            for _ in 0..count {
                out.extend(draws.take(channels - 1));
                let alpha = draws.byte();
                out.push(if alpha < 96 { 0 } else { alpha });
            }
            out
        }
        other => panic!("unknown kind {other}"),
    }
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Each chunk's type and data.
fn chunks(png: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut pos = 8;
    while pos < png.len() {
        let n = u32::from_be_bytes(png[pos..pos + 4].try_into().unwrap()) as usize;
        let kind = png[pos + 4..pos + 8].try_into().unwrap();
        out.push((kind, &png[pos + 8..pos + 8 + n]));
        pos += 12 + n;
    }
    out
}

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pillow_png.json");
    let text = std::fs::read_to_string(&path).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// What one case got wrong, if anything.
fn check(case: &Value) -> Vec<String> {
    let s = |k: &str| case[k].as_str().unwrap().to_owned();
    let u = |k: &str| case[k].as_u64().unwrap();
    let mode = Mode::from_name(&s("mode")).unwrap();
    let width = u32::try_from(u("width")).unwrap();
    let height = u32::try_from(u("height")).unwrap();
    let prefix = case["prefix"]
        .as_u64()
        .map_or(0, |n| usize::try_from(n).unwrap());
    let data = pixels(&s("kind"), mode, width, height, u("seed"), prefix);
    if hex(&Sha256::digest(&data)) != s("pixels_sha256") {
        return vec!["the generator's pixels differ from the script's".to_owned()];
    }
    let image = Image::new(width, height, mode, &data).unwrap();
    let mut wrong = Vec::new();

    // X4: the visible hash.
    if hex(&visible_hash(&image)) != s("visible") {
        wrong.push("visible hash".to_owned());
    }

    // X3: the PNG.
    let png = encode(&image, &mut Unmetered).unwrap();
    let (bytes, ()) = png.into_parts();
    // Its capacity; tests/charges.rs counts that it is one allocation.
    if bytes.capacity() != plan(width, height, mode).unwrap().output {
        wrong.push("the output's capacity is not the plan's".to_owned());
    }
    let parts = chunks(&bytes);
    let idat: Vec<&[u8]> = parts
        .iter()
        .filter(|(k, _)| k == b"IDAT")
        .map(|(_, d)| *d)
        .collect();
    let kinds: Vec<[u8; 4]> = parts.iter().map(|(k, _)| *k).collect();
    let mut want = vec![*b"IHDR"];
    want.extend(std::iter::repeat_n(*b"IDAT", idat.len()));
    want.push(*b"IEND");
    if kinds != want {
        wrong.push("chunks".to_owned());
    }
    let mut filtered = Vec::new();
    flate2::read::ZlibDecoder::new(&idat.concat()[..])
        .read_to_end(&mut filtered)
        .unwrap();
    if hex(&Sha256::digest(&filtered)) != s("filtered_sha256") {
        wrong.push("filtered rows".to_owned());
    }
    let stride = width as usize * mode.channels() + 1;
    let mut filters = [0_u64; 5];
    for row in filtered.chunks(stride) {
        filters[row[0] as usize] += 1;
    }
    let want: Vec<u64> = case["filters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    if filters[..] != want[..] {
        wrong.push(format!("filters {filters:?}, Pillow's {want:?}"));
    }
    let sizes: Vec<u64> = idat.iter().map(|d| d.len() as u64).collect();
    let want: Vec<u64> = case["idat"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    if sizes != want {
        wrong.push(format!("IDAT sizes {sizes:?}, Pillow's {want:?}"));
    }
    if bytes.len() as u64 != u("png_len") || hex(&Sha256::digest(&bytes)) != s("png_sha256") {
        wrong.push("PNG bytes".to_owned());
    }
    if let Some(want) = case["png_hex"].as_str()
        && hex(&bytes) != want
    {
        wrong.push(format!("PNG {} against Pillow's {want}", hex(&bytes)));
    }
    wrong
}

#[test]
fn pngs_and_visible_hashes_equal_pillows() {
    let fixture = fixture();
    assert_eq!(fixture["pillow"], "9.5.0");
    assert_eq!(fixture["zlib"], "1.2.13");
    let cases = fixture["cases"].as_array().unwrap();
    assert!(cases.len() >= 400, "{} cases", cases.len());
    let mut failed = Vec::new();
    for case in cases {
        let wrong = check(case);
        if !wrong.is_empty() {
            failed.push(format!("{}: {}", case["id"], wrong.join("; ")));
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {} cases differ:\n{}",
        failed.len(),
        cases.len(),
        failed.join("\n")
    );
}

/// A case's IDAT chunks' data sizes.
fn idat(case: &Value) -> Vec<u64> {
    case["idat"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect()
}

#[test]
fn the_fixture_covers_every_mode_kind_and_chunk_size() {
    let fixture = fixture();
    let cases = fixture["cases"].as_array().unwrap();
    for mode in Mode::EXTRACTOR {
        let of_mode: Vec<&Value> = cases.iter().filter(|c| c["mode"] == mode.name()).collect();
        // Several IDAT chunks, of the encoder buffer's size but the last: at
        // widths past 16384 (4 x width bytes) and below it (65,536).
        for size in [80000, 65540, 65536] {
            assert!(
                of_mode.iter().any(|c| {
                    let sizes = idat(c);
                    sizes.len() > 1 && sizes[0] == size
                }),
                "{mode}: no case of several chunks of {size} bytes"
            );
        }
        let mut kinds = vec![
            "flat", "gradient", "random", "noisy", "smooth", "sparse", "stripes",
        ];
        if matches!(mode, Mode::La | Mode::Rgba) {
            kinds.push("alpha");
        }
        for kind in kinds {
            assert!(of_mode.iter().any(|c| c["kind"] == kind), "{mode} {kind}");
        }
    }
    // Streams that end at a chunk's edge, or one to four bytes past it (the
    // last call returns only some of zlib's trailer), below 16384 wide and
    // past it: every chunk but the last is the buffer's size.
    for (bufsize, wide) in [(65536, false), (65540, true)] {
        for ends in [0, 1, 2, 3, 4, bufsize].map(|past| bufsize + past) {
            assert!(
                cases.iter().any(|c| {
                    let sizes = idat(c);
                    let (last, full) = sizes.split_last().unwrap();
                    (c["width"].as_u64().unwrap() > 16384) == wide
                        && full.iter().all(|&s| s == bufsize)
                        && full.iter().sum::<u64>() + last == ends
                }),
                "no stream of {ends} bytes in chunks of {bufsize}"
            );
        }
    }
    // Every filter but average (only with `optimize`) is chosen somewhere.
    let mut used = [0_u64; 5];
    for case in cases {
        for (n, v) in used.iter_mut().zip(case["filters"].as_array().unwrap()) {
            *n += v.as_u64().unwrap();
        }
    }
    assert!(
        used[0] > 0 && used[1] > 0 && used[2] > 0 && used[4] > 0,
        "{used:?}"
    );
    assert_eq!(used[3], 0);
}

/// Inflate and reverse PNG filters independently of the writer. Check framing
/// and CRCs too, including multi-IDAT files and transparent pixel RGB values.
fn decoded(bytes: &[u8], width: usize, height: usize, bpp: usize) -> Vec<u8> {
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    let parts = chunks(bytes);
    assert_eq!(parts.first().unwrap().0, *b"IHDR");
    assert_eq!(parts.last().unwrap().0, *b"IEND");
    let mut at = 8;
    let mut compressed = Vec::new();
    for (kind, data) in parts {
        let mut crc = flate2::Crc::new();
        crc.update(&kind);
        crc.update(data);
        let end = at + 8 + data.len();
        assert_eq!(
            crc.sum(),
            u32::from_be_bytes(bytes[end..end + 4].try_into().unwrap())
        );
        at = end + 4;
        if kind == *b"IDAT" {
            compressed.extend_from_slice(data);
        }
    }
    assert_eq!(at, bytes.len());
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(&compressed[..])
        .read_to_end(&mut raw)
        .unwrap();
    let stride = width * bpp;
    assert_eq!(raw.len(), height * (stride + 1));
    let mut out = vec![0u8; height * stride];
    for y in 0..height {
        let filter = raw[y * (stride + 1)];
        for x in 0..stride {
            let a = if x >= bpp {
                out[y * stride + x - bpp]
            } else {
                0
            };
            let b = if y > 0 { out[(y - 1) * stride + x] } else { 0 };
            let c = if y > 0 && x >= bpp {
                out[(y - 1) * stride + x - bpp]
            } else {
                0
            };
            let pred = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => a.midpoint(b),
                4 => {
                    let p = i16::from(a) + i16::from(b) - i16::from(c);
                    let (pa, pb, pc) = (
                        (p - i16::from(a)).abs(),
                        (p - i16::from(b)).abs(),
                        (p - i16::from(c)).abs(),
                    );
                    if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    }
                }
                _ => panic!("invalid filter {filter}"),
            };
            out[y * stride + x] = raw[y * (stride + 1) + 1 + x].wrapping_add(pred);
        }
    }
    out
}

#[test]
#[cfg(feature = "test-reference")]
fn fast_png_idat_matches_zlib_1_2_13_level_1_filtered_sub_rows() {
    // Structured data distinguishes level 1 from level 6 and Huffman-only
    // compression; repeated rows distinguish Sub from adaptive filtering.
    // Random RGBA data also forces concatenation of multiple IDAT chunks.
    for (kind, mode) in [
        ("gradient", Mode::L),
        ("sparse", Mode::La),
        ("stripes", Mode::Rgb),
        ("random", Mode::Rgba),
    ] {
        let (width, height) = (257, 127);
        let data = pixels(kind, mode, width, height, 0x1234_5678, 0);
        let bpp = mode.channels();
        let stride = width as usize * bpp;
        // Construct Sub scanlines independently of the encoder's Rows.
        let mut filtered = Vec::new();
        for row in data.chunks_exact(stride) {
            filtered.push(1);
            for (x, &value) in row.iter().enumerate() {
                let left = if x < bpp { 0 } else { row[x - bpp] };
                filtered.push(value.wrapping_sub(left));
            }
        }
        let expected =
            unityrs_imageio::test_reference::deflate_fast_png(&filtered, stride + 1, 65536);
        let image = Image::new(width, height, mode, &data).unwrap();
        let png = unityrs_imageio::encode_fast(&image, &mut Unmetered).unwrap();
        let idats: Vec<_> = chunks(png.bytes())
            .into_iter()
            .filter_map(|(kind, data)| (kind == *b"IDAT").then_some(data))
            .collect();
        if kind == "random" {
            assert!(idats.len() > 1, "fixture must exercise multiple IDATs");
        }
        let actual = idats.concat();
        assert_eq!(
            actual.len(),
            expected.len(),
            "{mode}-{width}x{height}-{kind}: concatenated IDAT length"
        );
        // Compare every byte, reporting only the first mismatch.
        assert_eq!(
            actual.iter().zip(&expected).position(|(a, b)| a != b),
            None,
            "{mode}-{width}x{height}-{kind}: concatenated IDAT bytes"
        );
    }
}

#[test]
#[cfg(feature = "test-reference")]
fn fast_palette_png_uses_level_one_sub_rows_for_both_palette_depths() {
    // Literal scanlines cover both 8-bit indices and odd-width packed bits.
    // Comparing against pristine zlib catches a lost fast level or filter
    // when the palette path is combined with the fast encoder.
    let pixels = [0, 1, 2, 3, 4, 5, 6, 7, 8, 8, 7, 6, 5, 4, 3, 2, 1, 0];
    let palette = std::array::from_fn(|i| i.to_le_bytes()[0]);
    let explicit = Image::with_palette(9, 2, &pixels, &palette).unwrap();
    let implicit = Image::new(9, 2, Mode::P, &pixels).unwrap();
    for (image, depth, filtered, packed) in [
        (
            explicit,
            8,
            [
                1, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 8, 255, 255, 255, 255, 255, 255, 255, 255,
            ]
            .as_slice(),
            pixels.as_slice(),
        ),
        (
            implicit,
            1,
            [1, 0x7f, 1, 1, 0xff, 1].as_slice(),
            [0x7f, 0x80, 0xff, 0].as_slice(),
        ),
    ] {
        let png = unityrs_imageio::encode_fast(&image, &mut Unmetered).unwrap();
        let parts = chunks(png.bytes());
        assert_eq!(parts[0].1[8..10], [depth, 3]);
        assert_eq!(parts[1].0, *b"PLTE");
        assert_eq!(parts[1].1, image.palette().map_or(&[0; 3][..], |p| &p[..]));
        let compressed = parts
            .iter()
            .filter_map(|(kind, data)| (*kind == *b"IDAT").then_some(*data))
            .collect::<Vec<_>>()
            .concat();
        let expected = unityrs_imageio::test_reference::deflate_fast_png(filtered, 10, 65536);
        assert_eq!(compressed, expected, "palette depth {depth}");
        assert_eq!(decoded(png.bytes(), packed.len() / 2, 2, 1), packed);
    }
}

#[test]
fn fast_png_decodes_to_the_same_oracle_pixels_as_default() {
    let fixture = fixture();
    for case in fixture["cases"].as_array().unwrap() {
        let mode = Mode::from_name(case["mode"].as_str().unwrap()).unwrap();
        let width = u32::try_from(case["width"].as_u64().unwrap()).unwrap();
        let height = u32::try_from(case["height"].as_u64().unwrap()).unwrap();
        let data = pixels(
            case["kind"].as_str().unwrap(),
            mode,
            width,
            height,
            case["seed"].as_u64().unwrap(),
            usize::try_from(case["prefix"].as_u64().unwrap_or(0)).unwrap(),
        );
        let image = Image::new(width, height, mode, &data).unwrap();
        let fast = unityrs_imageio::encode_fast(&image, &mut Unmetered).unwrap();
        let default = encode(&image, &mut Unmetered).unwrap();
        let decode =
            |bytes: &[u8]| decoded(bytes, width as usize, height as usize, mode.channels());
        let got = decode(fast.bytes());
        assert_eq!(got, decode(default.bytes()), "{}", case["id"]);
        assert_eq!(got, data, "{}", case["id"]);
        assert_eq!(
            hex(&Sha256::digest(&got)),
            case["pixels_sha256"].as_str().unwrap()
        );
        assert_eq!(
            visible_hash(&Image::new(width, height, mode, &got).unwrap()),
            visible_hash(&image)
        );
    }
}
