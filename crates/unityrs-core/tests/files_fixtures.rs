//! Replays package C's oracle fixtures (`tests/fixtures/c/`, written by
//! `tools/fixtures/files.py` from UnityPy 1.25.2): version strings, detection,
//! and the testkit file matrix's L0 and L1 records (`docs/CANONICAL_DUMP.md`
//! section 4.2), which a small dumper here writes from the core's API the way
//! the oracle dump writes them from UnityPy's.

// SHA-256's constants, as the standard writes them; hex built by `format!`.
#![allow(clippy::unreadable_literal, clippy::format_collect)]

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value as J, json};
use unityrs_core::budget::Reservation;
use unityrs_core::detect::{FileType, check_file_type};
use unityrs_core::files::{BundleFile, File, SerializedFile, SerializedType, parse_file};
use unityrs_core::limits::Meter;
use unityrs_core::stream::Endian;
use unityrs_core::version::{FlagsClass, Packed, UnityVersion, flags_class, uses_block_alignment};
use unityrs_core::{Bytes, FileContext, RawStr};

fn load(name: &str) -> Option<J> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/c")
        .join(name);
    let text = std::fs::read_to_string(path).ok()?;
    Some(serde_json::from_str(&text).unwrap())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

// ------------------------------------------------------------------ sha256

fn sha256(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let mut v = h;
        for i in 0..64 {
            let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
            let ch = (v[4] & v[5]) ^ (!v[4] & v[6]);
            let t1 = v[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
            let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
            let t2 = s0.wrapping_add(maj);
            v = [
                t1.wrapping_add(t2),
                v[0],
                v[1],
                v[2],
                v[3].wrapping_add(t1),
                v[4],
                v[5],
                v[6],
            ];
        }
        for (a, b) in h.iter_mut().zip(v) {
            *a = a.wrapping_add(b);
        }
    }
    h.iter().map(|x| format!("{x:08x}")).collect()
}

#[test]
fn sha256_matches_known_digests() {
    assert_eq!(
        sha256(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256(&[b'a'; 1000]),
        "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
    );
}

// ------------------------------------------------------------------ dumper

/// A metadata string: a JSON string when UTF-8, else tagged hex.
fn name(b: &[u8]) -> J {
    match std::str::from_utf8(b) {
        Ok(s) => J::String(s.to_owned()),
        Err(_) => json!({"t": "str", "hex": hex(b)}),
    }
}

fn opt_name(s: Option<&RawStr>) -> J {
    s.map_or(J::Null, |s| name(s.as_bytes()))
}

/// An integer as the fixture writes it: a number within i64, else "big:N".
fn int(v: i128) -> J {
    match i64::try_from(v) {
        Ok(v) => json!(v),
        Err(_) => J::String(format!("big:{v}")),
    }
}

fn packed(p: &Packed) -> J {
    let s = p.to_string();
    match s.parse::<i64>() {
        Ok(v) => json!(v),
        Err(_) => J::String(format!("big:{s}")),
    }
}

fn type_name(t: &Result<FileType, unityrs_core::detect::DetectError>) -> &'static str {
    match t {
        Ok(FileType::AssetsFile) => "AssetsFile",
        Ok(FileType::BundleFile) => "BundleFile",
        Ok(FileType::WebFile) => "WebFile",
        Ok(FileType::ResourceFile) => "ResourceFile",
        Ok(FileType::Zip) => "ZIP",
        Err(_) => "error",
    }
}

fn parsed_name(f: &File) -> &'static str {
    match f {
        File::Serialized(_) => "SerializedFile",
        File::Bundle(_) => "BundleFile",
        File::Web(_) => "WebFile",
        File::Raw(_) => "raw",
    }
}

fn type_record(t: &SerializedType) -> J {
    let nodes = t.nodes.as_ref().map(|n| {
        n.rows()
            .iter()
            .map(|r| {
                json!([
                    r.level,
                    name(r.ty.as_bytes()),
                    name(r.name.as_bytes()),
                    r.byte_size,
                    r.version,
                    r.type_flags,
                    r.variable_count,
                    r.index,
                    r.meta_flag,
                    r.ref_type_hash.map(|h| int(i128::from(h))),
                ])
            })
            .collect::<Vec<_>>()
    });
    json!({
        "class_id": t.class_id,
        "is_stripped_type": t.is_stripped_type,
        "script_type_index": t.script_type_index,
        "script_id": t.script_id.map(|b| hex(&b)),
        "old_type_hash": t.old_type_hash.map(|b| hex(&b)),
        "nodes": nodes,
        "type_dependencies": t.type_dependencies.as_ref().map(|d| d.to_vec()),
        "class_name": opt_name(t.class_name.as_ref()),
        "namespace": opt_name(t.namespace.as_ref()),
        "assembly": opt_name(t.assembly.as_ref()),
    })
}

fn serialized_l1(sf: &Arc<SerializedFile>) -> J {
    let h = sf.header();
    let container: Vec<J> = sf
        .container()
        .unwrap()
        .rows()
        .iter()
        .map(|r| {
            json!([
                r.path.as_ref().map(|p| name(p.as_bytes())),
                r.preload_index,
                r.preload_size,
                r.file_id,
                r.path_id
            ])
        })
        .collect();
    json!({
        "kind": "serialized",
        "header": {
            "metadata_size": h.metadata_size,
            "file_size": h.file_size,
            "version": h.version,
            "data_offset": h.data_offset,
            "endian": if h.endian == Endian::Big { ">" } else { "<" },
            "reserved": h.reserved.map(|r| hex(&r)),
            "unknown": h.unknown,
        },
        "unity_version": sf.unity_version().map_or(json!("2.5.0f5"), |s| name(s.as_bytes())),
        "version_packed": sf.version().map(|v| packed(v.version.packed())),
        "target_platform": sf.target_platform(),
        "enable_type_tree": sf.enable_type_tree(),
        "big_id_enabled": sf.big_id_enabled(),
        "types": sf.types().iter().map(type_record).collect::<Vec<_>>(),
        "object_order": sf.object_order().iter()
            .map(|i| sf.object_infos()[*i as usize].path_id).collect::<Vec<_>>(),
        "script_types": sf.script_types().iter()
            .map(|s| json!([s.file_index, s.identifier])).collect::<Vec<_>>(),
        "externals": sf.externals().iter().map(|e| json!([
            opt_name(e.temp_empty.as_ref()),
            e.guid.map(|g| hex(&g)),
            e.kind,
            name(e.path.as_bytes()),
        ])).collect::<Vec<_>>(),
        "ref_types": sf.ref_types().map(|t| t.iter().map(type_record).collect::<Vec<_>>()),
        "user_information": opt_name(sf.user_information()),
        "container": container,
    })
}

fn bundle_l1(b: &BundleFile) -> J {
    let h = b.header();
    let row = |u: u32, c: u32, f: u32| json!([u, c, f]);
    json!({
        "kind": "bundle",
        "signature": name(h.signature.as_bytes()),
        "format": h.format,
        "version_player": name(h.version_player.as_bytes()),
        "version_engine": name(h.version_engine.as_bytes()),
        "engine_version_packed": match &h.engine_version {
            Some(Ok(v)) => packed(v.version.packed()),
            _ => J::Null,
        },
        "flags": h.data_flags,
        "flags_class": h.flags_class.map(|c| match c {
            FlagsClass::New => "ArchiveFlags",
            FlagsClass::Old => "ArchiveFlagsOld",
        }),
        "block_alignment": h.block_alignment,
        "encrypted": h.encrypted,
        "hash": h.hash.map(|x| hex(&x)),
        "crc": h.crc,
        "blocks_info": h.blocks_info.map(|i| row(i.uncompressed, i.bytes_read, i.flags)),
        "blocks": b.blocks().iter().map(|i| row(i.uncompressed, i.bytes_read, i.flags)).collect::<Vec<_>>(),
        "directory": b.directory().iter()
            .map(|d| json!([d.offset, d.size, d.flags, name(d.path.as_bytes())]))
            .collect::<Vec<_>>(),
    })
}

fn emit(f: &File, path: &[J], l0: &J, out: &mut Vec<J>) {
    let l1 = match f {
        File::Serialized(sf) => serialized_l1(sf),
        File::Bundle(b) => bundle_l1(b),
        _ => J::Null,
    };
    out.push(json!({"rec": "file", "file": path, "l0": l0, "l1": l1, "error": null}));
    if let File::Bundle(b) = f {
        for slot in b.files() {
            let bytes = b.entry_bytes(slot.dir_index).unwrap();
            let child = b.entry(slot.dir_index).unwrap();
            let l0 = json!({
                "type": type_name(&check_file_type(&bytes, &b.context().limits)),
                "parsed_as": parsed_name(&child),
                "size": bytes.len(),
                "sha256": sha256(&bytes),
            });
            let mut p = path.to_vec();
            p.push(name(slot.name.as_bytes()));
            emit(&child, &p, &l0, out);
        }
    }
}

/// One input's L0 and L1 records, as `unitypy_dump.py`'s `_dump_input`
/// writes them (notes left out).
fn dump(input: &str, data: &[u8]) -> Vec<J> {
    let ctx = Arc::new(FileContext::unitypy());
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let detected = check_file_type(data, &ctx.limits);
    let mut l0 = json!({
        "type": type_name(&detected),
        "parsed_as": null,
        "size": data.len(),
        "sha256": sha256(data),
    });
    let path = vec![name(input.as_bytes())];
    let mut out = Vec::new();
    let ty = match detected {
        Err(_) => {
            l0["parsed_as"] = json!("error");
            out.push(json!({"rec": "file", "file": path, "l0": l0, "l1": null, "error": {"kind": "detect"}}));
            return out;
        }
        Ok(FileType::Zip) => {
            l0["parsed_as"] = json!("zip");
            out.push(json!({"rec": "file", "file": path, "l0": l0, "l1": null, "error": null}));
            return out;
        }
        Ok(ty) => ty,
    };
    let loaded = parse_file(
        RawStr::from(input),
        Bytes::copy_from_slice(data),
        ty,
        None,
        &meter,
        &ctx,
    )
    .and_then(|f| {
        if let File::Bundle(b) = &f {
            b.load_all()?;
        }
        Ok(f)
    });
    match loaded {
        Err(_) => {
            l0["parsed_as"] = json!("error");
            out.push(json!({"rec": "file", "file": path, "l0": l0, "l1": null, "error": {"kind": "parse"}}));
        }
        Ok(f) => {
            l0["parsed_as"] = json!(parsed_name(&f));
            emit(&f, &path, &l0, &mut out);
        }
    }
    out
}

// ------------------------------------------------------------------- tests

/// The fixture's inputs are the testkit's matrix, byte for byte. With
/// `UNITYRS_WRITE_MATRIX=<path>` it writes the matrix there instead, as JSON
/// lines (`tools/fixtures/files.py` reads them).
#[test]
fn matrix_inputs_are_the_testkit_matrix() {
    let matrix = unityrs_testkit::bundle::matrix();
    if let Ok(out) = std::env::var("UNITYRS_WRITE_MATRIX") {
        let text: String = matrix
            .iter()
            .map(|(n, b)| format!("{}\n", json!({"name": n, "hex": hex(b)})))
            .collect();
        std::fs::write(out, text).unwrap();
        return;
    }
    let doc = load("files.json").expect("tests/fixtures/c/files.json");
    let cases = doc["cases"].as_array().unwrap();
    assert_eq!(cases.len(), matrix.len());
    for (c, (n, b)) in cases.iter().zip(&matrix) {
        assert_eq!(c["name"].as_str().unwrap(), n);
        assert_eq!(
            c["hex"].as_str().unwrap(),
            hex(b),
            "{n}: the builder changed"
        );
    }
}

/// G3's L0 and L1 rows on C's matrix: every record equal to UnityPy's.
#[test]
fn the_matrix_has_no_l0_l1_differences() {
    let doc = load("files.json").expect("tests/fixtures/c/files.json");
    let mut differences = Vec::new();
    let mut records = 0;
    for c in doc["cases"].as_array().unwrap() {
        let input = c["name"].as_str().unwrap();
        let ours = dump(input, &unhex(c["hex"].as_str().unwrap()));
        let theirs = c["records"].as_array().unwrap();
        records += theirs.len();
        if ours != *theirs {
            differences.push(format!(
                "{input}:\n  ours   {}\n  oracle {}",
                J::Array(ours),
                J::Array(theirs.clone())
            ));
        }
    }
    assert!(records > 100, "{records} records");
    assert!(
        differences.is_empty(),
        "{} of {} inputs differ:\n{}",
        differences.len(),
        doc["cases"].as_array().unwrap().len(),
        differences.join("\n")
    );
}

#[test]
fn detection_matches_the_oracle() {
    let doc = load("detect.json").expect("tests/fixtures/c/detect.json");
    let limits = unityrs_core::limits::Limits::default();
    for c in doc["cases"].as_array().unwrap() {
        let data = unhex(c["hex"].as_str().unwrap());
        assert_eq!(
            type_name(&check_file_type(&data, &limits)),
            c["type"].as_str().unwrap(),
            "{}",
            c["name"]
        );
    }
}

fn version_cases(ascii: bool) -> Vec<J> {
    let doc = load("versions.json").expect("tests/fixtures/c/versions.json");
    doc["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["ascii_digits"].as_bool() == Some(ascii))
        .cloned()
        .collect()
}

fn check_version(c: &J) {
    let raw = RawStr::new(unhex(c["hex"].as_str().unwrap()));
    let parsed = UnityVersion::parse(&raw);
    let Some(want) = c.get("packed").filter(|p| !p.is_null()) else {
        assert!(parsed.is_err(), "{c}: {parsed:?}");
        return;
    };
    let v = parsed.unwrap_or_else(|e| panic!("{c}: {e}"));
    assert_eq!(&packed(v.packed()), want, "{c}");
    let tuple = v.as_tuple().ok().map(|t| json!([t.0, t.1, t.2, t.3, t.4]));
    assert_eq!(tuple.unwrap_or(J::Null), c["tuple"], "{c}");
    let flags = flags_class(&v).ok().map(|f| match f {
        FlagsClass::New => "ArchiveFlags",
        FlagsClass::Old => "ArchiveFlagsOld",
    });
    assert_eq!(json!(flags), c["flags"], "{c}");
    assert_eq!(
        json!(uses_block_alignment(6, &v).ok()),
        c["align_f6"],
        "{c}"
    );
    assert_eq!(json!(v.major()), c["major"], "{c}");
}

#[test]
fn versions_match_the_oracle() {
    let cases = version_cases(true);
    assert!(cases.len() > 40);
    for c in &cases {
        check_version(c);
    }
}

/// Versions written with non-ASCII `Nd` digits, which Python's `int()`
/// reads, through package A's generated `tables::ND_ZEROS` (every `Nd` run
/// of Unicode 14.0, not the scaffold's ASCII seed).
#[test]
fn non_ascii_digit_versions_match_the_oracle() {
    let cases = version_cases(false);
    assert_ne!(cases.len(), 0);
    assert!(
        unityrs_core::tables::ND_ZEROS.len() > 1,
        "ND_ZEROS holds only the ASCII seed: run tools/gen/gen_tables.py"
    );
    for c in &cases {
        check_version(c);
    }
}
