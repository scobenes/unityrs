//! Encoder buffers and partially allocated astcenc contexts obey the meter.
#![cfg(feature = "encode")]
use std::sync::Arc;
use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::limits::Meter;
use unityrs_texture::write::{Options, Source, WriteError, image_to_texture2d};

#[test]
fn partial_encoder_allocations_release_every_charge() {
    let pixels = [17, 43, 91, 183];
    let source = Source::new(1, 1, "RGBA", &pixels, None).unwrap();
    for format in [10, 12, 25, 26, 27, 34, 45, 47, 48, 59] {
        let mut successes = 0;
        let mut refusals = 0;
        for limit in [
            0,
            64,
            256,
            1024,
            4096,
            16384,
            65536,
            1 << 20,
            8 << 20,
            64 << 20,
        ] {
            let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
            let result = image_to_texture2d(&source, Options::new(format), &meter);
            match &result {
                Ok(encoded) => {
                    successes += 1;
                    assert_eq!(encoded.format, format);
                    assert!(meter.used() >= encoded.data.len() as u64);
                    assert_eq!(
                        encoded.data.len(),
                        if [10, 26, 34, 45].contains(&format) {
                            8
                        } else {
                            16
                        }
                    );
                }
                Err(WriteError::Charge(_)) => refusals += 1,
                other => panic!("format {format}, limit {limit}: {other:?}"),
            }
            drop(result);
            assert_eq!(meter.used(), 0, "format {format}, limit {limit}");
        }
        assert!(successes > 0 && refusals > 0, "format {format}");
        let meter = Meter::with_decode_work(64 << 20, 0, Arc::new(Unbounded), Reservation::Input);
        assert!(matches!(
            image_to_texture2d(&source, Options::new(format), &meter),
            Err(WriteError::Charge(_))
        ));
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn independent_encodes_are_deterministic_when_called_concurrently() {
    // BC7's one-time lookup-table setup and astcenc's per-call allocation
    // context must be safe when independent callers encode simultaneously.
    std::thread::scope(|scope| {
        let jobs: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    let pixels = [19; 4 * 4 * 4];
                    let source = Source::new(4, 4, "RGBA", &pixels, None).unwrap();
                    let meter = Meter::unlimited();
                    [25, 48].map(|format| {
                        let encoded =
                            image_to_texture2d(&source, Options::new(format), &meter).unwrap();
                        let bytes = encoded.data.to_vec();
                        drop(encoded);
                        assert_eq!(meter.used(), 0);
                        bytes
                    })
                })
            })
            .collect();
        let outputs: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
        for pair in outputs.windows(2) {
            assert_eq!(pair[0], pair[1]);
        }
    });
}

#[test]
fn concurrent_astc_footprints_keep_their_own_quality_presets() {
    // Upstream config_init used a shared mutable preset pointer. Different
    // footprints select different preset families, so exercise them together.
    let pixels: Vec<u8> = (0..13 * 11 * 4usize)
        .map(|i| (i.wrapping_mul(73) + i / 3 * 19 + 17).to_le_bytes()[0])
        .collect();
    let source = Source::new(13, 11, "RGBA", &pixels, None).unwrap();
    let expected = std::array::from_fn::<_, 6, _>(|i| {
        image_to_texture2d(
            &source,
            Options::new(48 + i32::try_from(i).unwrap()),
            &Meter::unlimited(),
        )
        .unwrap()
        .data
    });
    let barrier = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        for worker in 0..4usize {
            let expected = &expected;
            let barrier = &barrier;
            scope.spawn(move || {
                for round in 0..32 {
                    let i = (worker + round) % 6;
                    barrier.wait();
                    let encoded = image_to_texture2d(
                        &source,
                        Options::new(48 + i32::try_from(i).unwrap()),
                        &Meter::unlimited(),
                    )
                    .unwrap();
                    assert_eq!(encoded.data, expected[i]);
                }
            });
        }
    });
}

#[test]
fn large_astc_12x12_is_refused_under_work_limit_that_admits_dxt1() {
    use unityrs_core::error::LimitKind;
    use unityrs_core::limits::ChargeError;

    let pixels = vec![73; 1200 * 1200 * 4];
    let source = Source::new(1200, 1200, "RGBA", &pixels, None).unwrap();
    for format in [10, 53, 59] {
        let meter =
            Meter::with_decode_work(64 << 20, 128 << 20, Arc::new(Unbounded), Reservation::Input);
        let result = image_to_texture2d(&source, Options::new(format), &meter);
        if format == 10 {
            assert!(result.is_ok(), "{result:?}");
        } else {
            assert!(
                matches!(
                    result,
                    Err(WriteError::Charge(ChargeError::Limit(ref error)))
                        if error.kind() == LimitKind::DecodeWork
                ),
                "format {format} should refuse decode work"
            );
        }
        drop(result);
        assert_eq!(meter.used(), 0);
    }
}
