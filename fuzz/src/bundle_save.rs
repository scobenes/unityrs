//! Bundle load/save/load and canonical-save idempotence on existing seeds.
use unityrs_core::files::{
    BundleFile, BundlePacker, BundleSaveOptions, File, SaveError, SaveOptions, WebSaveOptions,
};
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
        Error::Entry { source, .. } => resource_refusal(source),
        _ => false,
    }
}

/// Every accepted save reloads with the same files, directory flags and
/// saved child content, then saves identically. Each open has a fresh
/// work allowance. Invalid input and resource refusals are allowed.
pub fn check(data: &[u8]) {
    check_packer(data, BundlePacker::None);
    // Payload mutations select all five packers without changing the header;
    // format 9 and named HC reach HEAD's writer and cumulative chunk rule.
    let packer = match data.get(data.len() / 2).copied().unwrap_or(0) % 5 {
        0 => BundlePacker::Lz4,
        1 => BundlePacker::Lzma,
        2 => BundlePacker::Flags {
            data: 195,
            blocks: 3,
        },
        3 => BundlePacker::Lz4Hc,
        _ => BundlePacker::Original,
    };
    check_packer(data, packer);
}

fn check_packer(data: &[u8], packer: BundlePacker) {
    let ctx = crate::context();
    let bytes = Bytes::copy_from_slice(data);
    for kind in [ReaderKind::Memory, ReaderKind::Stream] {
        let Ok(bundle) = BundleFile::open_with_reader(
            RawStr::from_static(b"fuzz.bundle"),
            bytes.clone(),
            None,
            &crate::meter(&ctx),
            &ctx,
            kind,
        ) else {
            continue;
        };
        let options = BundleSaveOptions {
            packer,
            ..BundleSaveOptions::default()
        };
        let saved = match bundle.save(options) {
            Ok(saved) => saved,
            // Malformed reader inputs can lack writeable fields; future
            // HEAD formats remain outside this writer's scope.
            Err(
                SaveError::Charge(_)
                | SaveError::Read(_)
                | SaveError::Range(_)
                | SaveError::Missing(_)
                | SaveError::Unsupported(_)
                | SaveError::UnsupportedCodec(_)
                | SaveError::HeadTierWritesNotImplemented,
            ) => continue,
            Err(error) => panic!("unexpected encoder failure: {error}"),
        };
        let back = match BundleFile::open_with_reader(
            RawStr::from_static(b"saved.bundle"),
            saved.clone(),
            None,
            &crate::meter(&ctx),
            &ctx,
            kind,
        ) {
            Ok(back) => back,
            Err(error) if resource_refusal(&error) => continue,
            Err(error) => panic!("saved bundle does not reload: {error}"),
        };
        assert_eq!(bundle.files().len(), back.files().len());
        for (original, restored) in bundle.files().iter().zip(back.files()) {
            assert_eq!(original.name, restored.name);
            assert_eq!(
                bundle.directory()[original.dir_index].flags,
                back.directory()[restored.dir_index].flags,
            );
            let expected = match bundle.entry(original.dir_index) {
                Ok(File::Raw(raw)) => raw.data.clone(),
                Ok(File::Serialized(sf)) => match sf.save(SaveOptions::default()) {
                    Ok(saved) => saved,
                    Err(SaveError::Charge(_)) => continue,
                    Err(error) => panic!("previously saved child refuses: {error}"),
                },
                // Nested containers' own saves are pinned by idempotence.
                Ok(File::Bundle(_)) => continue,
                Ok(File::Web(web)) => match web.save(WebSaveOptions::default()) {
                    Ok(saved) => saved,
                    Err(SaveError::Charge(_)) => continue,
                    Err(SaveError::Read(error)) if resource_refusal(&error) => continue,
                    Err(error) => panic!("previously saved WebFile refuses: {error}"),
                },
                Err(error) if resource_refusal(&error) => continue,
                Err(error) => panic!("previously saved child does not parse: {error}"),
            };
            match back.entry_bytes(restored.dir_index) {
                Ok(actual) => assert_eq!(expected, actual),
                Err(error) if resource_refusal(&error) => continue,
                Err(error) => panic!("saved child range is invalid: {error}"),
            }
        }
        // Original adopts the first saved block's codec, which may have
        // become stored. Its stable acceptance rule is canonical none.
        let (expected, next_options) = if packer == BundlePacker::Original {
            let canonical = BundleSaveOptions::default();
            match back.save(canonical) {
                Ok(bytes) => (bytes, canonical),
                Err(SaveError::Charge(_)) => continue,
                Err(SaveError::Read(error)) if resource_refusal(&error) => continue,
                Err(error) => panic!("canonical save refused: {error}"),
            }
        } else {
            (saved, options)
        };
        let stable = if packer == BundlePacker::Original {
            match BundleFile::open_with_reader(
                RawStr::from_static(b"canonical.bundle"),
                expected.clone(),
                None,
                &crate::meter(&ctx),
                &ctx,
                kind,
            ) {
                Ok(bundle) => bundle,
                Err(error) if resource_refusal(&error) => continue,
                Err(error) => panic!("canonical bundle does not reload: {error}"),
            }
        } else {
            back
        };
        match stable.save(next_options) {
            Ok(again) => assert_eq!(expected, again),
            Err(SaveError::Charge(_)) => {}
            Err(SaveError::Read(error)) if resource_refusal(&error) => {}
            Err(error) => panic!("second save refused a saved bundle: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    fn bytes(fixture: &serde_json::Value, at: &serde_json::Value) -> Vec<u8> {
        fixture["bytes"][usize::try_from(at.as_u64().unwrap()).unwrap()]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|piece| {
                let hex = fixture["data"][usize::try_from(piece.as_u64().unwrap()).unwrap()]
                    .as_str()
                    .unwrap();
                hex.as_bytes()
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    #[test]
    fn head_bundle_sources_and_saved_offsets_reach_the_roundtrip_harness() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../crates/unityrs-core/tests/fixtures/head_save_bundle.json"
        ))
        .unwrap();
        let cases = fixture["cases"].as_array().unwrap();
        assert_eq!(cases.len(), 108);
        let mut seen = std::collections::BTreeSet::new();
        for case in cases {
            for key in ["input", "output"] {
                if seen.insert(case[key].as_u64().unwrap()) {
                    super::check(&bytes(&fixture, &case[key]));
                }
            }
        }
        // Select Original and named HC explicitly on all format-9 source
        // shapes, independently of the payload-driven fuzz selector.
        let mut seen = std::collections::BTreeSet::new();
        for case in cases
            .iter()
            .filter(|case| case["name"].as_str().unwrap().starts_with("v9"))
        {
            if seen.insert(case["input"].as_u64().unwrap()) {
                let input = bytes(&fixture, &case["input"]);
                super::check_packer(&input, super::BundlePacker::Original);
                super::check_packer(&input, super::BundlePacker::Lz4Hc);
            }
        }
    }

    #[test]
    fn existing_bundle_builder_seeds_roundtrip() {
        for (_, data) in unityrs_testkit::mutate::builder_outputs() {
            super::check(&data);
        }
        for format in 1..=8 {
            if format == 6 {
                continue;
            }
            for signature in [b"UnityRaw".as_slice(), b"UnityWeb"] {
                super::check(&unityrs_testkit::web::legacy(
                    signature,
                    format,
                    &[("raw.resS", b"payload")],
                ));
            }
        }
    }
}
