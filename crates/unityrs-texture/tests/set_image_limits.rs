//! Memory/work refusals and atomic replacement of installed object bytes.
use std::sync::Arc;
use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::files::{SaveOptions, SerializedFile};
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::serialized::{build, one_object};
use unityrs_texture::write::{Options, Source, WriteError, image_to_texture2d, set_image};

#[test]
fn every_memory_threshold_releases_all_intermediate_charges() {
    let pixels: Vec<_> = (0..35 * 19 * 4usize).map(|i| i.to_le_bytes()[0]).collect();
    let image = Source::new(35, 19, "RGBA", &pixels, None).unwrap();
    let blob = [0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0];
    let mut successes = 0;
    for options in [
        Options {
            mipmap_count: 8,
            ..Options::new(4)
        },
        Options {
            platform: 38,
            platform_blob: Some(&blob),
            ..Options::new(4)
        },
    ] {
        for limit in (0..=32768).step_by(64) {
            let meter =
                Meter::with_decode_work(limit, 1 << 20, Arc::new(Unbounded), Reservation::Input);
            let result = image_to_texture2d(&image, options, &meter);
            if let Ok(encoded) = &result {
                successes += 1;
                assert!(meter.used() >= encoded.data.len() as u64);
            }
            drop(result);
            assert_eq!(meter.used(), 0, "limit {limit}");
        }
    }
    assert!(successes > 0);
    let meter = Meter::with_decode_work(1 << 20, 0, Arc::new(Unbounded), Reservation::Input);
    assert!(image_to_texture2d(&image, Options::new(4), &meter).is_err());
    assert_eq!(meter.used(), 0);
}

#[test]
fn zero_sides_skip_conversion_and_encoders_but_keep_named_switch_refusals() {
    for mode in ["La", "BGR;15", "L", "P"] {
        for (width, height) in [(0, 0), (0, 4), (4, 0)] {
            let image = Source::new(width, height, mode, &[], None).unwrap();
            for format in [4, 10, 25, 34, 64, 65] {
                let meter = Meter::new(0, Arc::new(Unbounded), Reservation::Input);
                let encoded = image_to_texture2d(&image, Options::new(format), &meter).unwrap();
                assert!(encoded.data.is_empty());
                assert_eq!(meter.used(), 0);
                let error = image_to_texture2d(
                    &image,
                    Options {
                        platform: 38,
                        platform_blob: Some(&[0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]),
                        ..Options::new(format)
                    },
                    &meter,
                )
                .unwrap_err();
                assert!(matches!(
                    error,
                    WriteError::Unsupported("zero-size Switch texture")
                ));
            }
        }
    }
}

#[test]
fn failures_preserve_previously_installed_object_bytes() {
    let file = SerializedFile::parse(
        RawStr::from_static(b"CAB-atomic"),
        Bytes::from(build(&one_object(
            28,
            vec![
                NodeSpec::new(0, "Texture2D", "Base").size(-1),
                NodeSpec::new(1, "int", "m_TextureFormat").size(4),
            ],
            4i32.to_le_bytes().to_vec(),
        ))),
        None,
        &Meter::unlimited(),
        &Arc::new(FileContext::unitypy()),
    )
    .unwrap();
    let object = file.object(1).unwrap();
    let image = Source::new(1, 1, "RGBA", &[1, 2, 3, 4], None).unwrap();
    let result = set_image(&object, &image, Some(10), 1);
    if cfg!(feature = "encode") {
        // Encoding succeeds, then the incomplete object schema refuses the
        // field edit. The earlier object bytes still cannot be installed.
        assert!(matches!(result, Err(WriteError::Invalid(_))));
    } else {
        assert!(matches!(result, Err(WriteError::EncoderUnavailable(10))));
    }
    assert!(!file.is_changed());
    object.set_raw_data(b"previous edit").unwrap();
    // This fails only after encoding, while editing a missing required field.
    assert!(matches!(
        set_image(&object, &image, None, 1),
        Err(WriteError::Invalid(_))
    ));
    let saved = file.save(SaveOptions::default()).unwrap();
    let back = SerializedFile::parse(
        RawStr::from_static(b"CAB-atomic"),
        saved,
        None,
        &Meter::unlimited(),
        &Arc::new(FileContext::unitypy()),
    )
    .unwrap();
    assert_eq!(
        back.object(1).unwrap().raw_data().unwrap().as_ref(),
        b"previous edit"
    );
}

#[test]
fn malformed_inputs_and_layout_exponents_return_errors() {
    for (width, height, mode, bytes) in [
        (u32::MAX, 0, "RGBA", &[][..]),
        (u32::MAX, u32::MAX, "RGBA", &[][..]),
        (1, 1, "RGBA", &[1, 2, 3][..]),
        (1, 1, "unknown", &[1][..]),
    ] {
        assert!(Source::new(width, height, mode, bytes, None).is_err());
    }
    let image = Source::new(1, 1, "RGBA", &[1, 2, 3, 4], None).unwrap();
    let meter = Meter::unlimited();
    for exponent in [31u32, 32, u32::MAX] {
        let mut blob = [0u8; 12];
        blob[8..].copy_from_slice(&exponent.to_le_bytes());
        assert!(
            image_to_texture2d(
                &image,
                Options {
                    platform: 38,
                    platform_blob: Some(&blob),
                    ..Options::new(4)
                },
                &meter
            )
            .is_err()
        );
        assert_eq!(meter.used(), 0);
    }
    assert!(image_to_texture2d(&image, Options::new(i32::MAX), &meter).is_err());
    let blob = [0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0];
    for format in [34, 45, 46, 47, 60, 64, 65] {
        assert!(matches!(
            image_to_texture2d(
                &image,
                Options {
                    platform: 38,
                    platform_blob: Some(&blob),
                    ..Options::new(format)
                },
                &meter
            ),
            Err(WriteError::Unsupported("Switch texel block"))
        ));
    }
    let image = Source::new(1, 1, "La", &[0, 0], None).unwrap();
    for format in [4, 10, 25] {
        assert!(matches!(
            image_to_texture2d(&image, Options::new(format), &meter),
            Err(WriteError::Unsupported(
                "Pillow cannot convert image mode to RGBA"
            ))
        ));
    }
}
