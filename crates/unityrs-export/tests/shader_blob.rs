use unityrs_core::limits::Meter;
use unityrs_export::shader::{Program, validate_blob};

fn blob(version: i32, kind: i32, code: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    for n in [1i32, 16, 0, 0, version, kind, 0, 0, 0] {
        b.extend(n.to_le_bytes());
    }
    if version >= 201_608_170 {
        b.extend(0i32.to_le_bytes());
    }
    b.extend(0i32.to_le_bytes());
    if (201_806_140..202_012_090).contains(&version) {
        b.extend(0i32.to_le_bytes());
    }
    b.extend(i32::try_from(code.len()).unwrap().to_le_bytes());
    b.extend(code);
    b
}
#[test]
fn blob_versions_and_program_text() {
    for version in [
        201_509_030,
        201_510_240,
        201_608_170,
        201_609_010,
        201_708_220,
        201_802_150,
        201_806_140,
        202_012_090,
    ] {
        let data = blob(version, 1, b"gl code");
        let meter = Meter::unlimited();
        let p = Program::parse(&data, (2021, 3), &meter).unwrap();
        assert_eq!(p.export(0, &meter).unwrap().as_slice(), b"\"gl code\"");
    }
}
#[test]
fn hostile_tables_are_errors_without_declared_allocations() {
    let m = Meter::unlimited();
    assert!(validate_blob(&i32::MAX.to_le_bytes(), (2021, 3), &m).is_err());
    assert!(validate_blob(&(-1i32).to_le_bytes(), (2021, 3), &m).is_ok());
    for size in 0..48 {
        assert_eq!(
            validate_blob(&vec![0xff; size], (2021, 3), &m).is_ok(),
            size >= 4
        );
    }
    let data = blob(202_012_090, 32, b"");
    let e = Program::parse(&data, (2021, 3), &m).unwrap_err();
    assert_eq!(e.exception(), Some("ValueError"));
    assert_eq!(e.to_string(), "32 is not a valid ShaderGpuProgramType");
}
#[test]
fn code_families_keep_reference_comments_and_utf8_errors() {
    let m = Meter::unlimited();
    for (kind, code, want) in [
        (
            9,
            b"x".as_slice(),
            "\"// shader disassembly not supported on DXBC\"",
        ),
        (25, b"x", "\"\""),
        (31, b"x", "\"//shader disassembly not supported on 31\""),
        (23, b"ABCDentry\0metal", "\"metal\""),
    ] {
        let b = blob(202_012_090, kind, code);
        assert_eq!(
            Program::parse(&b, (2021, 3), &m)
                .unwrap()
                .export(0, &m)
                .unwrap()
                .as_slice(),
            want.as_bytes()
        );
    }
    let b = blob(202_012_090, 1, b"\xff");
    assert_eq!(
        Program::parse(&b, (2021, 3), &m)
            .unwrap()
            .export(0, &m)
            .unwrap_err()
            .exception(),
        Some("UnicodeDecodeError")
    );
}

#[test]
fn invalid_capacity_keeps_first_subprogram_exception() {
    let mut blob = Vec::new();
    for n in [i32::MAX, 8, 201_806_140, 99] {
        blob.extend(n.to_le_bytes());
    }
    let error = Program::parse(&blob, (2021, 3), &Meter::unlimited()).unwrap_err();
    assert_eq!(error.exception(), Some("ValueError"));
    assert_eq!(error.to_string(), "99 is not a valid ShaderGpuProgramType");
}

#[test]
fn legacy_blob_refusal_precedes_script_and_output_allocation() {
    use unityrs_core::{
        Bytes, File, RawStr,
        env::{EnvOptions, Environment, Source},
    };
    use unityrs_testkit::{
        ngui::{Object, V, st},
        scene3d::{Layout, file},
    };
    let data = file(
        Layout::Unity2022,
        &[],
        &[Object::new(
            1,
            48,
            st(
                "Shader",
                vec![
                    ("m_SubProgramBlob", V::Bytes(vec![1])),
                    ("m_Script", V::Int(99)),
                ],
            ),
        )],
    )
    .unwrap();
    let env = Environment::new(EnvOptions::unitypy());
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from("synthetic-shader.assets"),
            data: Bytes::from(data),
        })
        .unwrap()
    else {
        panic!("file")
    };
    let meter = Meter::unlimited();
    let error = unityrs_export::shader::export(&file.object(1).unwrap(), &env, &meter).unwrap_err();
    assert!(matches!(
        error,
        unityrs_export::shader::Error::Unsupported(_)
    ));
    assert_eq!(meter.used(), 0);
    assert!(meter.decoded() > 0); // Field-name searches are charged before the refusal.
}

#[test]
fn rendering_charges_before_utf8_and_metal_failure_paths() {
    use std::sync::Arc;
    use unityrs_core::{
        budget::{Reservation, Unbounded},
        limits::ChargeError,
    };
    for (kind, code, scan) in [
        (1, b"aa\xff".as_slice(), 3),
        (23, b"ABCDunterminated".as_slice(), 32),
    ] {
        let bytes = blob(202_012_090, kind, code);
        let program = Program::parse(&bytes, (2021, 3), &Meter::unlimited()).unwrap();
        let before = 1; // Opening quote, then the scan's upper bound.
        for work in [before, before + scan - 1] {
            let meter =
                Meter::with_decode_work(u64::MAX, work, Arc::new(Unbounded), Reservation::Input);
            let error = program.export(0, &meter).unwrap_err();
            assert!(
                matches!(error, unityrs_export::shader::Error::Charge(ChargeError::Limit(ref e)) if e.kind() == unityrs_core::error::LimitKind::DecodeWork),
                "{error}"
            );
            assert_eq!(meter.used(), 0);
        }
        // Invalid UTF-8 errors retain a metered copy of the decoded source
        // for Python's UnicodeDecodeError.object and exact error span.
        let error_copy = if kind == 1 { code.len() as u64 } else { 0 };
        let meter = Meter::with_decode_work(
            u64::MAX,
            before + scan + error_copy,
            Arc::new(Unbounded),
            Reservation::Input,
        );
        let error = program.export(0, &meter).unwrap_err();
        assert_eq!(
            error.exception(),
            Some(if kind == 1 {
                "UnicodeDecodeError"
            } else {
                "Exception"
            })
        );
    }
}
