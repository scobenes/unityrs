//! SerializedFile load/save/load invariants, shared with the seed replay test.
use unityrs_core::files::{SaveError, SaveOptions, SerializedFile};
use unityrs_core::stream::ReaderKind;
use unityrs_core::{Bytes, Error, RawStr};

fn resource_refusal(error: &Error) -> bool {
    use unityrs_core::typetree::TreeError;
    use unityrs_core::walk::{ParseError, ReadDetail};
    match error {
        Error::Limit(_) | Error::Budget(_) | Error::Allocator(_) => true,
        Error::Tree(TreeError::Charge(_) | TreeError::Limit(_)) => true,
        Error::AssetBundle(error) => match error.as_ref() {
            ParseError::Read(read) => matches!(
                &read.detail,
                ReadDetail::Charge(_)
                    | ReadDetail::Limit(_)
                    | ReadDetail::Tree(TreeError::Charge(_) | TreeError::Limit(_))
            ),
            ParseError::Tree(TreeError::Charge(_) | TreeError::Limit(_)) => true,
            _ => false,
        },
        _ => false,
    }
}

/// An accepted save must reload with the same live objects and raw bytes,
/// then save identically. Every operation uses its own work allowance;
/// a memory/budget refusal is an allowed outcome at every stage.
/// Formats 23-26 use HEAD's writer; formats above 26 refuse save.
pub fn check(data: &[u8]) {
    let ctx = crate::context();
    let bytes = Bytes::copy_from_slice(data);
    for kind in [ReaderKind::Memory, ReaderKind::Stream] {
        let meter = crate::meter(&ctx);
        let Ok(file) = SerializedFile::parse_with_reader(
            RawStr::from_static(b"CAB-fuzz"),
            bytes.clone(),
            None,
            &meter,
            &ctx,
            kind,
        ) else {
            continue;
        };
        let saved = match file.save(SaveOptions::default()) {
            Ok(bytes) => bytes,
            Err(
                SaveError::Charge(_)
                | SaveError::HeadTierWritesNotImplemented
                | SaveError::Range(_)
                | SaveError::Missing(_)
                | SaveError::Read(_),
            ) => continue,
            Err(error) => panic!("unexpected save error: {error}"),
        };
        let meter = crate::meter(&ctx);
        let back = match SerializedFile::parse_with_reader(
            RawStr::from_static(b"CAB-back"),
            saved.clone(),
            None,
            &meter,
            &ctx,
            kind,
        ) {
            Ok(back) => back,
            Err(error) if resource_refusal(&error) => continue,
            Err(error) => panic!("saved file does not reload: {error}"),
        };
        assert_eq!(file.object_order().len(), back.object_order().len());
        assert_eq!(file.unity_version(), back.unity_version());
        assert_eq!(file.externals(), back.externals());
        assert_eq!(file.script_types(), back.script_types());
        assert_eq!(file.user_information(), back.user_information());
        for (original, restored) in file.types().iter().zip(back.types()) {
            assert_eq!(original.class_id, restored.class_id);
            assert_eq!(original.nodes, restored.nodes);
            assert_eq!(original.type_dependencies, restored.type_dependencies);
        }

        for original in file.objects() {
            let restored = back.object(original.path_id()).expect("saved path id");
            assert_eq!(restored.class_id(), original.class_id());
            assert_eq!(restored.info().type_id, original.info().type_id);
            assert_eq!(
                restored.raw_data().expect("saved byte range"),
                original.raw_data().expect("original byte range")
            );
        }
        match back.save(SaveOptions::default()) {
            Ok(again) => assert_eq!(again, saved),
            Err(SaveError::Charge(_)) => {}
            Err(error) => panic!("second save refused a written file: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn head_format_26_save_roundtrip_regression() {
        // The HEAD seed that previously saved v22 metadata under a v26 header.
        let hex = concat!(
            "00000000000000000000001a000000000000000000000049000000000000",
            "008000000000000000800000000000000000363030302e342e3066310000",
            "00000000010000000100000000ffff000000000000000000000000000000",
            "00000000000000000000000000000000000000000073796e746865746963",
            "0000000000000000"
        );
        let data: Vec<u8> = hex
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect();
        let ctx = crate::context();
        let file = unityrs_core::files::SerializedFile::parse(
            unityrs_core::RawStr::from_static(b"CAB-regression"),
            unityrs_core::Bytes::copy_from_slice(&data),
            None,
            &crate::meter(&ctx),
            &ctx,
        )
        .unwrap();
        assert_eq!(file.header().version, 26);
        assert!(
            file.save(unityrs_core::files::SaveOptions::default())
                .is_ok()
        );
        super::check(&data);
    }

    #[test]
    fn existing_serialized_builder_seeds_roundtrip() {
        for (_, data) in unityrs_testkit::mutate::builder_outputs() {
            super::check(&data);
        }
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../crates/unityrs-core/tests/fixtures/serialized_save.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let at = usize::try_from(case["input"].as_u64().unwrap()).unwrap();
            let data: Vec<u8> = fixture["data"][at]
                .as_str()
                .unwrap()
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
                .collect();
            super::check(&data);
        }
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../crates/unityrs-core/tests/fixtures/head_save_serialized.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            for field in ["input", "output"] {
                let Some(hex) = case[field].as_str() else {
                    continue;
                };
                let data: Vec<u8> = hex
                    .as_bytes()
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
                    .collect();
                super::check(&data);
            }
        }
    }
}
