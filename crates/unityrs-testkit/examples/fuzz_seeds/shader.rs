//! Synthetic shader program blobs: both entry widths, subprogram header
//! boundaries, malformed counts, offsets and truncated fields; plus
//! arbitrary typetree values reaching the converter's field guards.
use super::Seeds;
use unityrs_testkit::ByteWriter;
use unityrs_testkit::fuzz::{Row, ShaderCase, TreeCase};
use unityrs_testkit::ngui::{self, V};

fn blob(
    version: (u32, u32),
    program_version: i32,
    kind: i32,
    code: &[u8],
    keywords: bool,
) -> Vec<u8> {
    let offset: i32 = if version >= (2019, 3) { 16 } else { 12 };
    let mut out = 1i32.to_le_bytes().to_vec();
    out.extend_from_slice(&offset.to_le_bytes());
    out.resize(usize::try_from(offset).unwrap_or(0), 0);
    for v in [program_version, kind, 0, 0, 0] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    if program_version >= 201_608_170 {
        out.extend_from_slice(&0i32.to_le_bytes());
    }
    let mut words = || {
        out.extend_from_slice(&i32::from(keywords).to_le_bytes());
        if keywords {
            out.extend_from_slice(&4i32.to_le_bytes());
            out.extend_from_slice(b"TEST");
        }
    };
    words(); // Global keywords.
    if (201_806_140..202_012_090).contains(&program_version) {
        words(); // Local keywords.
    }
    out.extend_from_slice(&i32::try_from(code.len()).unwrap_or(0).to_le_bytes());
    out.extend_from_slice(code);
    out.resize(out.len().next_multiple_of(4), 0);
    out
}

fn repeated(version: (u32, u32), blob: &[u8]) -> Vec<u8> {
    let entry = if version >= (2019, 3) { 12 } else { 8 };
    let offset: i32 = 4 + 2 * entry;
    let mut out = 2i32.to_le_bytes().to_vec();
    for _ in 0..2 {
        out.extend_from_slice(&offset.to_le_bytes());
        out.resize(out.len() + usize::try_from(entry - 4).unwrap_or(0), 0);
    }
    out.extend_from_slice(&blob[usize::try_from(4 + entry).unwrap_or(0)..]);
    out
}

pub(super) fn seeds(s: &mut Seeds) -> std::io::Result<()> {
    for version in [(5, 3), (2019, 2), (2019, 3), (2021, 1), (6000, 3)] {
        let mut cases = vec![
            ("empty".to_owned(), Vec::new()),
            ("zero".to_owned(), 0i32.to_le_bytes().to_vec()),
            ("negative-count".to_owned(), (-1i32).to_le_bytes().to_vec()),
            ("large-count".to_owned(), i32::MAX.to_le_bytes().to_vec()),
        ];
        for program_version in [201_509_030, 201_608_170, 201_806_140, 202_012_090] {
            for (tag, code, keywords) in [
                ("empty", b"".as_slice(), false),
                ("text", b"// synthetic\n".as_slice(), false),
                ("keywords", b"// synthetic\n".as_slice(), true),
            ] {
                let bytes = blob(version, program_version, 1, code, keywords);
                cases.push((format!("p{program_version}-{tag}"), bytes));
            }
        }
        for (name, kind, code) in [
            ("invalid-utf8", 1, b"\xff".as_slice()),
            ("metal", 23, b"ABCDmain\0source"),
            ("metal-fs", 24, b"ABCDmain\0source"),
            (
                "metal-offset",
                23,
                b"\xfe\xca\x0d\xf0\x08\0\0\0main\0source",
            ),
            ("metal-no-nul", 23, b"ABCDmain"),
            ("metal-invalid-utf8", 23, b"ABCDmain\0\xff"),
        ] {
            cases.push((
                name.to_owned(),
                blob(version, 202_012_090, kind, code, false),
            ));
        }
        let bytes = blob(version, 201_806_140, 1, b"synthetic", true);
        cases.push(("repeated-entry".to_owned(), repeated(version, &bytes)));
        for cut in [3, 7, bytes.len() - 1] {
            cases.push((format!("cut-{cut}"), bytes[..cut].to_vec()));
        }
        for (name, offset) in [("negative-offset", -1i32), ("large-offset", i32::MAX)] {
            let mut bytes = bytes.clone();
            bytes[4..8].copy_from_slice(&offset.to_le_bytes());
            cases.push((name.to_owned(), bytes));
        }
        for (name, bytes) in cases {
            for meter in [0, 1, 16, 64] {
                let case = ShaderCase {
                    version,
                    meter,
                    blob: bytes.clone(),
                };
                s.add(
                    "shader",
                    &format!("v{}-{}-{name}-m{meter}", version.0, version.1),
                    &case.encode(),
                )?;
            }
        }
    }
    converter_seeds(s)?;
    recorded_converter_seeds(s)
}

/// Complete compressed shaders, including rendering failures, from the
/// synthetic oracle; the converter target must reach more than field guards.
fn recorded_converter_seeds(s: &mut Seeds) -> std::io::Result<()> {
    use std::sync::Arc;
    use unityrs_core::{
        Bytes, FileContext, RawStr, budget::Reservation, files::SerializedFile, limits::Meter,
    };
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../unityrs-export/tests/fixtures/shader.json"
    ))
    .map_err(std::io::Error::other)?;
    let modern: serde_json::Value = serde_json::from_str(include_str!(
        "../../../unityrs-export/tests/fixtures/shader_modern.json"
    ))
    .map_err(std::io::Error::other)?;
    let bad = || std::io::Error::other("invalid synthetic shader fixture");
    let legacy: serde_json::Value = serde_json::from_str(include_str!(
        "../../../unityrs-export/tests/fixtures/shader_legacy.json"
    ))
    .map_err(std::io::Error::other)?;
    for name in [
        "program_kind_1",
        "program_kind_23",
        "program_kind_24",
        "metal_header_offset",
        "converter_invalid_source_utf8",
        "multiple_platforms_tiers_sorted_nested_True",
        "player_progVertex_6000_3_0",
        "player_progFragment_6000_3_0",
        "player_progGeometry_6000_3_0",
        "player_progHull_6000_3_0",
        "player_progDomain_6000_3_0",
        "parameters_first_False_6000_3",
        "parameters_first_True_6000_3",
        "player_groups_duplicate_sorted",
        "segments_6000_3_valid",
        "negative_code_index",
        "missing_parameter_index",
        "big_endian_5_3_valid",
        "big_endian_5_4_valid",
        "big_endian_5_3_index_bad_9",
        "big_endian_5_4_index_bad_9",
    ] {
        let case = fixture["cases"]
            .as_array()
            .ok_or_else(bad)?
            .iter()
            .chain(modern["cases"].as_array().ok_or_else(bad)?)
            .chain(legacy["cases"].as_array().ok_or_else(bad)?)
            .find(|case| case["case"] == name)
            .ok_or_else(bad)?;
        let hex = case["input"].as_str().ok_or_else(bad)?;
        let bytes: Vec<u8> = hex
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| {
                std::str::from_utf8(pair)
                    .ok()
                    .and_then(|p| u8::from_str_radix(p, 16).ok())
                    .ok_or_else(bad)
            })
            .collect::<Result<_, _>>()?;
        let ctx = Arc::new(FileContext::unitypy());
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        let file = SerializedFile::parse(
            RawStr::from("synthetic.assets"),
            Bytes::from(bytes),
            None,
            &meter,
            &ctx,
        )
        .map_err(std::io::Error::other)?;
        let object = file.object(1).ok_or_else(bad)?;
        let nodes = object
            .serialized_type()
            .and_then(|t| t.nodes.as_deref())
            .ok_or_else(bad)?;
        let mut case = TreeCase {
            rows: super::rows(nodes),
            data: object.raw_data().map_err(std::io::Error::other)?.to_vec(),
            ..TreeCase::default()
        };
        for (tag, flags) in [("large", 0), ("small", TreeCase::SMALL_MEMORY)] {
            case.flags = flags;
            s.add(
                "shader",
                &format!("tree-complete-{name}-{tag}"),
                &case.encode(),
            )?;
        }
    }
    Ok(())
}

fn converter_seeds(s: &mut Seeds) -> std::io::Result<()> {
    for (name, fields) in [
        (
            "script",
            vec![("m_Script", V::Bytes(b"Shader synthetic {}\n".to_vec()))],
        ),
        ("empty-script", vec![("m_Script", V::Bytes(Vec::new()))]),
        ("invalid-utf8", vec![("m_Script", V::Bytes(vec![0xff]))]),
        (
            "string-script",
            vec![("m_Script", V::Str(b"text".to_vec()))],
        ),
        ("integer-script", vec![("m_Script", V::Int(-1))]),
        ("missing-script", vec![("arbitrary", V::Int(123))]),
        ("legacy-blob", vec![("m_SubProgramBlob", V::Bytes(vec![1]))]),
        (
            "missing-platforms",
            vec![("compressedBlob", V::Bytes(vec![1]))],
        ),
        (
            "string-blob",
            vec![("compressedBlob", V::Str(b"blob".to_vec()))],
        ),
        (
            "integer-platforms",
            vec![
                ("compressedBlob", V::Bytes(vec![1])),
                ("platforms", V::Int(1)),
            ],
        ),
    ] {
        let root = V::Struct(
            "Shader".to_owned(),
            fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect(),
        );
        let rows = ngui::nodes(0, "Base", &root).map_err(std::io::Error::other)?;
        let mut bytes = ByteWriter::le();
        ngui::write(&mut bytes, &root).map_err(std::io::Error::other)?;
        let mut case = TreeCase {
            rows: rows
                .into_iter()
                .map(|r| Row {
                    level: r.level,
                    meta_flag: r.meta_flag,
                    ty: r.ty,
                    name: r.name,
                })
                .collect(),
            data: bytes.into_inner(),
            ..TreeCase::default()
        };
        for (tag, flags) in [("large", 0), ("small", TreeCase::SMALL_MEMORY)] {
            case.flags = flags;
            s.add("shader", &format!("tree-{name}-{tag}"), &case.encode())?;
        }
    }
    Ok(())
}
