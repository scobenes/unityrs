//! Synthetic B2a inputs built by the shared bundle and SerializedFile builders.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use serde_json::{Value, json};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::bundle::{BundleSpec, EntrySpec, TestCodec, build, build_with_signature};

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, b| {
        write!(out, "{b:02x}").unwrap();
        out
    })
}

fn entry(name: &[u8], data: &[u8]) -> EntrySpec {
    EntrySpec {
        name: name.to_vec(),
        data: data.to_vec(),
        flags: 0,
    }
}

fn raw() -> BundleSpec {
    BundleSpec {
        entries: vec![
            entry(b"a.resS", b"alpha"),
            entry(b"empty.resS", b""),
            entry(b"a.resS", b"last-alpha"),
            entry(b"b-\xff.resS", b"beta\0\xff"),
        ],
        ..BundleSpec::default()
    }
}

fn add(out: &mut Vec<Value>, name: &str, input: &[u8], mutation: &str, packers: &Value) {
    out.push(json!({"name":name,"input":hex(input),"mutation":mutation,"packers":packers}));
}

fn packers() -> Value {
    json!(["none", "original", "lz4", "lzma", [195, 3]])
}

fn noise(len: usize) -> Vec<u8> {
    let mut state = 0x91e1_0da5u32;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state.to_le_bytes()[0]
        })
        .collect()
}

fn formats(out: &mut Vec<Value>) {
    for format in 1..=8 {
        // The FS reader accepts earlier format numbers too; alignment changes at 7.
        let spec = BundleSpec { format, ..raw() };
        add(
            out,
            &format!("fs-v{format}"),
            &build(&spec),
            "none",
            &packers(),
        );
    }
    for engine in [
        "2018.4.36f1",
        "2019.4.14f1",
        "2019.4.15f1",
        "2020.3.33f1",
        "2020.3.34f1",
        "2021.3.1f1",
        "2021.3.2f1",
        "2022.1.0f1",
        "2022.1.1f1",
    ] {
        let spec = BundleSpec {
            format: 6,
            version_engine: engine.as_bytes().to_vec(),
            ..raw()
        };
        add(
            out,
            &format!("engine-{engine}"),
            &build(&spec),
            "none",
            &packers(),
        );
    }
}

fn packing(out: &mut Vec<Value>) {
    for at_end in [false, true] {
        let spec = BundleSpec {
            info_at_end: at_end,
            extra_flags: 0x200,
            ..raw()
        };
        add(
            out,
            &format!("padding-end{at_end}"),
            &build(&spec),
            "none",
            &json!(["original", [0x240, 0x140], [0x2c3, 3]]),
        );
    }
    for codec in [TestCodec::None, TestCodec::Lzma, TestCodec::Lz4Hc] {
        let spec = BundleSpec {
            data_codec: codec,
            info_codec: codec,
            ..raw()
        };
        add(
            out,
            &format!("original-{codec:?}"),
            &build(&spec),
            "none",
            &json!(["original"]),
        );
    }
    for entries in [vec![], vec![entry(b"empty.resS", b"")]] {
        let spec = BundleSpec {
            entries,
            ..BundleSpec::default()
        };
        add(
            out,
            &format!("empty-{}", spec.entries.len()),
            &build(&spec),
            "none",
            &packers(),
        );
    }
}

fn chunks(out: &mut Vec<Value>) {
    for (label, data) in [
        ("random", noise(0x40001)),
        ("zeros", vec![0; 0x40001]),
        (
            "mixed",
            [
                noise(0x10000),
                vec![0; 0x10000],
                noise(0x10000),
                vec![0; 0x10000],
                noise(11),
            ]
            .concat(),
        ),
    ] {
        let spec = BundleSpec {
            entries: vec![entry(b"data.resS", &data)],
            ..BundleSpec::default()
        };
        add(out, label, &build(&spec), "none", &packers());
    }
    let mut exact = noise(130_549);
    exact.resize(0x20000, 0);
    exact.push(b'x');
    let spec = BundleSpec {
        entries: vec![entry(b"exact.resS", &exact)],
        ..BundleSpec::default()
    };
    add(
        out,
        "equal-size-chunk",
        &build(&spec),
        "none",
        &json!(["lz4", [195, 3]]),
    );
}

fn encryption(out: &mut Vec<Value>) {
    for engine in ["2020.3.33f1", "2022.3.0f1"] {
        let spec = BundleSpec {
            version_engine: engine.as_bytes().to_vec(),
            ..raw()
        };
        let packers = if engine.starts_with("2020") {
            json!([[0x240, 0x300], [0x2c3, 0x203]])
        } else {
            json!([
                [0x440, 0x500],
                [0x4c3, 0x403],
                [0x1040, 0x1100],
                [0x10c3, 0x1003],
                [0x1440, 0x1500],
                [0x14c3, 0x1403]
            ])
        };
        add(
            out,
            &format!("clear-encryption-{engine}"),
            &build(&spec),
            "none",
            &packers,
        );
    }
}

fn serialized(out: &mut Vec<Value>) {
    for format in 1..=22 {
        let mut sf = unityrs_testkit::serialized::one_object(
            49,
            vec![
                NodeSpec::new(0, "TextAsset", "Base"),
                NodeSpec::new(1, "int", "value"),
            ],
            31i32.to_le_bytes().to_vec(),
        );
        sf.format = format;
        if format < 16 {
            sf.objects[0].type_id = 49;
        }
        // Remain above ImportHelper's 128-byte AssetsFile sniff threshold,
        // including format 3 and the save that empties the edited object.
        let mut filler = sf.objects[0].clone();
        filler.path_id = 2;
        filler.data = vec![0x5a; 128];
        sf.objects.push(filler);
        let spec = BundleSpec {
            entries: vec![
                EntrySpec {
                    name: b"CAB-file".to_vec(),
                    data: unityrs_testkit::serialized::build(&sf),
                    flags: 4,
                },
                entry(b"keep.resS", b"kept"),
            ],
            ..BundleSpec::default()
        };
        for mutation in ["raw", "empty", "typetree"] {
            add(
                out,
                &format!("held-sf{format}-{mutation}"),
                &build(&spec),
                mutation,
                &if format == 22 {
                    packers()
                } else {
                    json!(["none"])
                },
            );
        }
    }
}

fn legacy(out: &mut Vec<Value>) {
    for signature in [b"UnityWeb".as_slice(), b"UnityRaw"] {
        for format in 1..=8 {
            let name = format!("{}-{format}", std::str::from_utf8(signature).unwrap());
            let input = if format == 6 {
                build_with_signature(&BundleSpec { format, ..raw() }, signature)
            } else {
                unityrs_testkit::web::legacy(
                    signature,
                    format,
                    &[("a.resS", b"legacy"), ("empty.resS", b"")],
                )
            };
            add(out, &name, &input, "none", &packers());
        }
    }
}

fn main() {
    let mut out = vec![];
    formats(&mut out);
    packing(&mut out);
    chunks(&mut out);
    encryption(&mut out);
    serialized(&mut out);
    legacy(&mut out);
    println!("{}", serde_json::to_string(&out).unwrap());
}
