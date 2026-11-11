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

/// The script's `pixels`.
fn pixels(kind: &str, mode: Mode, width: u32, height: u32, seed: u64) -> Vec<u8> {
    let channels = mode.channels();
    let count = width as usize * height as usize;
    let len = count * channels;
    let mut draws = Draws::new(seed);
    match kind {
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
    let data = pixels(&s("kind"), mode, width, height, u("seed"));
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
    assert!(cases.len() >= 388, "{} cases", cases.len());
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

#[test]
fn the_fixture_covers_every_mode_kind_and_chunk_size() {
    let fixture = fixture();
    let cases = fixture["cases"].as_array().unwrap();
    for mode in Mode::ALL {
        let of_mode: Vec<&Value> = cases.iter().filter(|c| c["mode"] == mode.name()).collect();
        // Several IDAT chunks, at a width past 16384 (chunks of 4 x width
        // bytes) and below it (65,536).
        for (width, size) in [(20000, 80000), (16385, 65540), (257, 65536)] {
            let case = of_mode
                .iter()
                .find(|c| c["width"] == width && c["idat"].as_array().unwrap().len() > 1);
            if mode != Mode::L || width == 20000 {
                let case = case.unwrap_or_else(|| panic!("{mode} {width}: no case of two chunks"));
                assert_eq!(case["idat"][0], size, "{mode} {width}");
            }
        }
        for kind in [
            "flat", "gradient", "random", "noisy", "smooth", "sparse", "stripes",
        ] {
            assert!(of_mode.iter().any(|c| c["kind"] == kind), "{mode} {kind}");
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
