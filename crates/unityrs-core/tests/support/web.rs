//! Inputs and compact L0/L1/L2 summaries shared by C2's recorder and replay.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use serde_json::{Value as J, json};
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::files::WebPacker;
use unityrs_core::value::Value;
use unityrs_core::{Error, File, Parsed, RawStr};
use unityrs_testkit::web;
#[path = "loading.rs"]
mod loading;
pub use loading::hex;
pub fn unhex(s: &str) -> Vec<u8> {
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}
pub fn inputs() -> J {
    let asset = unityrs_testkit::env::serialized(
        &[],
        &[unityrs_testkit::env::Obj::named(7, "synthetic-λ", &[])],
    );
    let inner = web::plain("TuanjieWebData1.0", &[("folder/CAB-N", &asset)]);
    let nested = unityrs_testkit::env::bundle(&[("CAB-B", asset.clone())]);
    let p = web::plain(
        "UnityWebData1.0",
        &[
            ("folder/inner", &inner),
            ("a\\b", &nested),
            ("raw", b"first"),
            ("raw", b"last"),
        ],
    );
    let mut cases = vec![];
    let mut add = |name: String, bytes: Vec<u8>, reference: Option<Vec<u8>>, tier: &str| {
        cases.push(json!({"name":name,"hex":hex(&bytes),"reference":reference.map(|b|hex(&b)),"tier":tier}));
    };
    for f in 1..=6 {
        for sig in ["UnityWeb", "UnityRaw"] {
            add(
                format!("{sig}-{f}"),
                web::legacy(
                    sig.as_bytes(),
                    f,
                    &[("folder/CAB-A", &asset), ("raw.resS", b"payload")],
                ),
                None,
                "A",
            );
        }
    }
    add("plain".into(), p.clone(), None, "A");
    add("gzip".into(), web::gzip(&p), Some(p.clone()), "B");
    add("brotli".into(), web::brotli(&p), None, "A");
    for level in [2, 3] {
        for sig in ["UnityWeb", "UnityRaw"] {
            add(
                format!("{sig}-levels-{level}"),
                web::legacy_levels(sig.as_bytes(), 5, &[("CAB-A", &asset)], level),
                None,
                "A",
            );
        }
    }
    add("brotli-huffman".into(), web::huffman(), None, "A");
    let base = web::plain("UnityWebData1.0", &[("raw", b"payload")]);
    for (label, at, n) in [
        ("name-rest", 28, -1i32),
        ("name-rewind", 28, -12),
        ("name-huge", 28, i32::MAX),
        ("offset-negative", 20, -7),
        ("size-rest", 24, -1),
        ("size-negative", 24, -2),
        ("header-huge", 16, 1000),
        ("offset-huge", 20, i32::MAX),
        ("header-negative", 16, -1),
    ] {
        let mut bytes = base.clone();
        bytes[at..at + 4].copy_from_slice(&n.to_le_bytes());
        add(
            label.into(),
            bytes,
            None,
            if label == "name-rewind" { "U" } else { "A" },
        );
    }
    let mut invalid = base.clone();
    invalid[32] = 255;
    add("invalid-utf8".into(), invalid, None, "A");
    for level in [-100i32, -1, 0, 2, i32::MAX] {
        let mut bytes = web::legacy(b"UnityRaw", 5, &[("raw", &[112; 4000])]);
        // Common strings occupy 30 bytes; hash/CRC 20; three u32 fields.
        let level_at = bytes
            .windows(11)
            .position(|x| x == b"2022.3.0f1\0")
            .unwrap()
            + 11
            + 20
            + 12;
        bytes[level_at..level_at + 4].copy_from_slice(&level.to_be_bytes());
        add(format!("level-{level}"), bytes, None, "A");
    }
    for n in [0, 1, 4, 5, 12, 20, 40] {
        add(
            format!("truncated-{n}"),
            web::legacy(b"UnityWeb", 5, &[("raw", b"abc")])[..n].to_vec(),
            None,
            "A",
        );
    }
    json!(cases)
}
fn value(p: &Parsed, v: &Value) -> J {
    match v {
        Value::Str(s) => json!(s.to_str().unwrap()),
        Value::Dict(m) => p
            .entries(m)
            .map(|(k, v)| (std::str::from_utf8(k).unwrap().to_owned(), value(p, v)))
            .collect(),
        _ => panic!("synthetic value {v:?}"),
    }
}
fn tree(f: &File) -> Result<J, Error> {
    let rows: Vec<_> = f
        .container_directory()
        .iter()
        .map(|d| json!([d.offset, d.size, d.flags, d.path.to_str().unwrap()]))
        .collect();
    let l1 = match f {
        File::Bundle(b) => {
            let h = b.header();
            json!({"signature":h.signature.to_str().unwrap(),"format":h.format,"player":h.version_player.to_str().unwrap(),"engine":h.version_engine.to_str().unwrap(),"hash":h.hash.map(|b|hex(&b)),"crc":h.crc,"directory":rows})
        }
        File::Web(w) => {
            json!({"signature":w.signature().to_str().unwrap(),"packer":match w.packer(){WebPacker::None=>"none",WebPacker::Gzip=>"gzip",WebPacker::Brotli=>"brotli"},"directory":rows})
        }
        _ => J::Null,
    };
    let kind = match f {
        File::Bundle(_) => "bundle",
        File::Web(_) => "web",
        File::Serialized(_) => "serialized",
        File::Raw(_) => "raw",
    };
    let mut children = vec![];
    for slot in f.container_files() {
        children.push(json!([
            slot.name.to_str().unwrap(),
            tree(&f.container_entry(slot.dir_index)?)?
        ]));
    }
    let bytes = if let File::Raw(r) = f {
        Some(hex(&r.data))
    } else {
        None
    };
    Ok(
        json!({"kind":kind,"type":format!("{:?}",f.file_type()),"l1":l1,"children":children,"bytes":bytes}),
    )
}
pub fn summary(data: Vec<u8>, stream: bool) -> J {
    let env = Environment::new(EnvOptions::unitypy());
    let got = if stream {
        let scratch = unityrs_testkit::scratch::Scratch::new("web-oracle");
        let path = scratch.path().join("synthetic");
        std::fs::write(&path, &data).unwrap();
        env.load_file(Source::Path(path))
    } else {
        env.load_file(Source::Bytes {
            name: RawStr::from("synthetic"),
            data: data.into(),
        })
    };
    let result = (|| {
        let f = got?.ok_or_else(|| Error::Malformed {
            what: "oracle",
            detail: "missing file".into(),
        })?;
        let file = tree(&f)?;
        let objects = env
            .objects()
            .map(|o| {
                let o = o?;
                let p = o.read_typetree(None, true).map_err(|_| Error::Malformed {
                    what: "oracle object",
                    detail: "typetree failed".into(),
                })?;
                Ok(json!({"path_id":o.path_id(),"tree":value(&p,p.value())}))
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok::<_, Error>(
            json!({"file":file,"objects":objects,"cabs":env.registry().keys().map(|k|k.to_str().unwrap().to_owned()).collect::<Vec<_>>()}),
        )
    })();
    result.unwrap_or_else(|_| json!({"error":true}))
}
