//! W2 replay: every default writer's whole-file length and SHA-256 or typed
//! Pillow refusal, with full hex for small files and a head for diagnostics.
use std::fmt::Write;

use serde_json::Value;
use sha2::{Digest, Sha256};
use unityrs_imageio::writers::{Format, Outcome, SaveError, save, save_by_extension};
use unityrs_imageio::{Image, Mode, Unmetered};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/writers.json")).unwrap()
}
fn pixels(case: &Value, mode: Mode) -> Vec<u8> {
    let width = usize::try_from(case["width"].as_u64().unwrap()).unwrap();
    let height = usize::try_from(case["height"].as_u64().unwrap()).unwrap();
    if matches!(case["kind"].as_str(), Some("flat" | "solid" | "overlap")) {
        return vec![73; width * height * mode.channels()];
    }
    (0..width * height * mode.channels())
        .map(|i| (i * 37 + i / mode.channels() * 11 + i / width.max(1) * 7).to_le_bytes()[0])
        .collect()
}
fn palette() -> [u8; 768] {
    std::array::from_fn(|i| (i * 19 + i / 3 * 7).to_le_bytes()[0])
}
fn image<'a>(case: &Value, mode: Mode, px: &'a [u8], palette: &'a [u8; 768]) -> Image<'a> {
    let width = u32::try_from(case["width"].as_u64().unwrap()).unwrap();
    let height = u32::try_from(case["height"].as_u64().unwrap()).unwrap();
    if mode == Mode::P && case["implicit_palette"] != true {
        Image::with_palette(width, height, px, palette)
    } else {
        Image::new(width, height, mode, px)
    }
    .unwrap()
}

fn unhex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}

fn file_mismatch(got: &[u8], want: &Value) -> Option<String> {
    let len = usize::try_from(want["len"].as_u64().unwrap()).unwrap();
    let sha = want["sha256"].as_str().unwrap();
    let mut got_sha = String::with_capacity(64);
    for byte in Sha256::digest(got) {
        let _ = write!(got_sha, "{byte:02x}");
    }
    if got.len() == len && got_sha == sha {
        return None;
    }
    let prefix = unhex(
        want["hex"]
            .as_str()
            .or_else(|| want["head"].as_str())
            .unwrap(),
    );
    let at = (0..prefix.len())
        .find(|&at| got.get(at) != prefix.get(at))
        .or_else(|| (prefix.len() == len && got.len() > len).then_some(len));
    let detail = at.map_or_else(
        || {
            format!(
                "no differing byte in recorded prefix ({} bytes)",
                prefix.len()
            )
        },
        |at| {
            let byte = |bytes: &[u8]| {
                bytes
                    .get(at)
                    .map_or_else(|| "EOF".to_owned(), |b| format!("{b:02x}"))
            };
            format!(
                "first mismatch at byte {at}: got {}, want {}",
                byte(got),
                byte(&prefix)
            )
        },
    );
    Some(format!(
        "bytes: got len {} SHA-256 {got_sha}, want len {len} SHA-256 {sha}; {detail}",
        got.len()
    ))
}

#[test]
fn compact_files_check_length_and_tail_bytes_and_diagnose_the_head() {
    // Independently computed SHA-256 of 257 zero bytes.
    let mut want = serde_json::json!({
        "len": 257,
        "sha256": "6c934d0cdf9dba94b474d6d1929f16739bd9a8ed31d0c3bcaf82c283fb7a3568",
        "head": "00".repeat(64),
    });
    let mut got = vec![0; 257];
    assert_eq!(file_mismatch(&got, &want), None);
    got[256] = 1;
    let wrong = file_mismatch(&got, &want).unwrap();
    assert!(wrong.contains("SHA-256"), "{wrong}");
    assert!(
        wrong.contains("no differing byte in recorded prefix (64 bytes)"),
        "{wrong}"
    );
    got[63] = 2;
    got[7] = 3;
    let wrong = file_mismatch(&got, &want).unwrap();
    assert!(
        wrong.contains("first mismatch at byte 7: got 03, want 00"),
        "{wrong}"
    );
    got.fill(0);
    let wrong = file_mismatch(&got[..32], &want).unwrap();
    assert!(
        wrong.contains("first mismatch at byte 32: got EOF, want 00"),
        "{wrong}"
    );
    // A matching digest must not bypass the separate length check.
    want["len"] = 258.into();
    let wrong = file_mismatch(&got, &want).unwrap();
    assert!(wrong.contains("got len 257"), "{wrong}");
    assert!(wrong.contains("want len 258"), "{wrong}");
}

#[test]
fn small_files_diagnose_bytes_beyond_the_head_and_extra_output() {
    // Independently computed SHA-256 of 256 zero bytes.
    let want = serde_json::json!({
        "len": 256,
        "sha256": "5341e6b2646979a70e57653007a1f310169421ec9bdd9f1a5648f75ade005af1",
        "hex": "00".repeat(256),
    });
    let mut got = vec![0; 256];
    assert_eq!(file_mismatch(&got, &want), None);
    got[255] = 1;
    let wrong = file_mismatch(&got, &want).unwrap();
    assert!(
        wrong.contains("first mismatch at byte 255: got 01, want 00"),
        "{wrong}"
    );
    got[255] = 0;
    got.push(1);
    let wrong = file_mismatch(&got, &want).unwrap();
    assert!(
        wrong.contains("first mismatch at byte 256: got 01, want EOF"),
        "{wrong}"
    );
}

#[test]
fn every_writer_replays_pillow_bytes_and_refusals() {
    let fixture = fixture();
    assert_eq!(fixture["pillow"], "9.5.0");
    assert_eq!(fixture["zlib"], "1.2.13");
    let cases = fixture["cases"].as_array().unwrap();
    assert!(cases.len() > 500);
    let mut failures = Vec::new();
    for case in cases {
        if case["policy"] != "writer" {
            continue;
        }
        let mode = Mode::from_name(case["mode"].as_str().unwrap()).unwrap();
        let width = u32::try_from(case["width"].as_u64().unwrap()).unwrap();
        let height = u32::try_from(case["height"].as_u64().unwrap()).unwrap();
        let px = pixels(case, mode);
        let palette = palette();
        let image = image(case, mode, &px, &palette);
        let format = Format::from_name(case["format"].as_str().unwrap()).unwrap();
        let result = save(
            &image,
            format,
            case["name"].as_str().unwrap(),
            &mut Unmetered,
        );
        let wrong = match (result, case["exception"].as_str()) {
            (Ok(out), None) => file_mismatch(out.bytes(), case),
            (Err(SaveError::Pillow(error)), Some(class)) if error.class.name() == class => {
                if class == "UnicodeEncodeError"
                    || error.message == case["message"].as_str().unwrap()
                {
                    None
                } else {
                    Some(format!(
                        "message {:?}, expected {}",
                        error.message, case["message"]
                    ))
                }
            }
            (Ok(out), _) => Some(format!(
                "saved {} bytes, expected {}",
                out.bytes().len(),
                case["exception"]
            )),
            (Err(error), _) => Some(format!("error {error:?}, expected {}", case["exception"])),
        };
        if let Some(wrong) = wrong {
            failures.push(format!(
                "{} {} {width}x{height} {}: {wrong}",
                case["format"], mode, case["name"]
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn png_fallbacks_and_no_file_replay_the_oracle() {
    let fixture = fixture();
    let mut count = 0;
    for case in fixture["cases"].as_array().unwrap() {
        if case.get("fallback").is_none() {
            continue;
        }
        count += 1;
        let mode = Mode::from_name(case["mode"].as_str().unwrap()).unwrap();
        let px = pixels(case, mode);
        let palette = palette();
        let image = image(case, mode, &px, &palette);
        let name = case["name"].as_str().unwrap();
        let extension = if case["format"].as_str().unwrap().starts_with('.') {
            case["format"].as_str().unwrap()
        } else {
            name.rfind('.').map_or("", |i| &name[i..])
        };
        let out = save_by_extension(&image, extension, name, &mut Unmetered).unwrap();
        match out {
            Outcome::PngFallback(out) if case["fallback"].is_object() => {
                if let Some(wrong) = file_mismatch(out.bytes(), &case["fallback"]) {
                    panic!("{name} {mode}: {wrong}");
                }
            }
            Outcome::NoFile if case["fallback"].is_null() => {}
            _ => panic!("{name} {mode}: unexpected fallback outcome"),
        }
    }
    assert!(count > 200, "{count}");
}

#[test]
fn decision_seven_writers_are_named_unsupported() {
    let fixture = fixture();
    let mut count = 0;
    for case in fixture["cases"].as_array().unwrap() {
        if case["policy"] != "unsupported" {
            continue;
        }
        count += 1;
        let mode = Mode::from_name(case["mode"].as_str().unwrap()).unwrap();
        let px = pixels(case, mode);
        let palette = palette();
        let image = image(case, mode, &px, &palette);
        let name = case["name"].as_str().unwrap();
        let extension = &name[name.rfind('.').unwrap()..];
        let expected = Format::from_name(case["format"].as_str().unwrap()).unwrap();
        assert!(
            matches!(save_by_extension(&image,extension,name,&mut Unmetered),Err(SaveError::Unsupported{format}) if format==expected)
        );
    }
    assert_eq!(count, 25);
}

#[test]
fn extension_selection_replays_original_files() {
    let fixture = fixture();
    let mut count = 0;
    for case in fixture["cases"].as_array().unwrap() {
        let typical = case["width"] == 3 && case["height"] == 2;
        let ico = case["format"] == "ICO" && case["width"] == 16 && case["height"] == 16;
        if case["policy"] != "writer" || (!typical && !ico) || case.get("fallback").is_some() {
            continue;
        }
        if case.get("len").is_none() {
            continue;
        }
        let name = case["name"].as_str().unwrap();
        if name
            != format!(
                "image.{}",
                case["format"].as_str().unwrap().to_ascii_lowercase()
            )
        {
            continue;
        }
        let extension = &name[name.rfind('.').unwrap()..];
        let mode = Mode::from_name(case["mode"].as_str().unwrap()).unwrap();
        let px = pixels(case, mode);
        let palette = palette();
        let image = image(case, mode, &px, &palette);
        // Pillow selects extensions case-insensitively. Only IM embeds
        // the filename's spelling, so pass the same original filename.
        for extension in [extension.to_owned(), extension.to_ascii_uppercase()] {
            match save_by_extension(&image, &extension, name, &mut Unmetered).unwrap() {
                Outcome::Original(out) => {
                    if let Some(wrong) = file_mismatch(out.bytes(), case) {
                        panic!("{name} {mode}: {wrong}");
                    }
                }
                out => panic!("{name} {mode}: unexpected {out:?}"),
            }
        }
        count += 1;
    }
    assert_eq!(count, 56);
}

#[test]
fn implicit_palette_png_uses_pillows_one_bit_packer() {
    // Pillow 9.5.0: Image.frombytes("P", (3,2), bytes(range(6))).save(fp,"PNG").
    let image = Image::new(3, 2, Mode::P, &[0, 1, 2, 3, 4, 5]).unwrap();
    let saved = save(&image, Format::Png, "", &mut Unmetered).unwrap();
    assert_eq!(
        saved.bytes(),
        unhex(
            "89504e470d0a1a0a0000000d4948445200000003000000020103000000a7baf45900000003504c5445000000a77a3dda0000000c49444154789c634860780000020401415ec1721d0000000049454e44ae426082"
        )
    );
}
