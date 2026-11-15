//! Bundle load/save/load and canonical-save idempotence on existing seeds.
use unityrs_core::files::{
    BundleFile, BundlePacker, BundleSaveOptions, File, SaveError, SaveOptions,
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
    // The format byte distributes existing UnityFS seeds among all three
    // encoders; fuzz mutations can also change this choice independently.
    let packer = match data.get(11).copied().unwrap_or(0) % 3 {
        0 => BundlePacker::Lz4,
        1 => BundlePacker::Lzma,
        _ => BundlePacker::Flags {
            data: 195,
            blocks: 3,
        },
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
            // Malformed reader inputs can lack writeable fields; a held
            // HEAD file and WebFile save are outside this writer's scope.
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
                Ok(File::Web(_)) => panic!("WebFile save is not implemented"),
                Err(error) if resource_refusal(&error) => continue,
                Err(error) => panic!("previously saved child does not parse: {error}"),
            };
            match back.entry_bytes(restored.dir_index) {
                Ok(actual) => assert_eq!(expected, actual),
                Err(error) if resource_refusal(&error) => continue,
                Err(error) => panic!("saved child range is invalid: {error}"),
            }
        }
        match back.save(options) {
            Ok(again) => assert_eq!(saved, again),
            Err(SaveError::Charge(_)) => {}
            Err(SaveError::Read(error)) if resource_refusal(&error) => {}
            Err(error) => panic!("second save refused a saved bundle: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
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
