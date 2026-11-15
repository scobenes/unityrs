use super::*;
use serde_json::Value;
fn fixture() -> Value {
    serde_json::from_str(include_str!("../../tests/fixtures/unitycrunch_native.json")).unwrap()
}
fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}
fn format(case: &Value) -> Format {
    match case["format"].as_u64().unwrap() {
        28 => Format::UnityDxt1Crunched,
        29 => Format::UnityDxt5Crunched,
        64 => Format::EtcRgb4Crunched,
        65 => Format::Etc2Rgba8Crunched,
        _ => panic!(),
    }
}
#[test]
fn every_fork_mip_matches_the_builder_blocks() {
    for case in fixture()["cases"].as_array().unwrap() {
        let data = unhex(case["data"]["hex"].as_str().unwrap());
        for (level, mip) in case["levels"].as_array().unwrap().iter().enumerate() {
            let size = (
                usize::try_from(mip["width"].as_u64().unwrap()).unwrap(),
                usize::try_from(mip["height"].as_u64().unwrap()).unwrap(),
            );
            let meter = Meter::unlimited();
            let got = unpack(&data, format(case), size, level, &meter)
                .unwrap_or_else(|e| panic!("{} mip {level}: {e}", case["id"]));
            assert_eq!(
                got.as_slice(),
                unhex(mip["blocks"].as_str().unwrap()),
                "{} mip {level}",
                case["id"]
            );
            drop(got);
            assert_eq!(meter.used(), 0);
        }
    }
}

#[test]
fn a_reference_to_uninitialized_row_storage_is_refused() {
    for case in fixture()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["width"] == 1 && c["height"] == 1)
    {
        let mut data = unhex(case["data"]["hex"].as_str().unwrap());
        let start = u32::from_be_bytes(data[70..74].try_into().unwrap()) as usize;
        data[start] = 0xaa; // Each two-bit reference asks for an unwritten row.
        assert!(
            unpack(&data, format(case), (1, 1), 0, &Meter::unlimited()).is_err(),
            "{}",
            case["id"]
        );
    }
}

fn edge_cases() -> Vec<Value> {
    fixture()["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["id"].as_str().unwrap().ends_with("1x1-m1-edge1"))
        .cloned()
        .collect()
}
fn set_two(data: &mut [u8], start: usize, bit: usize, value: u8) {
    for i in 0..2 {
        let at = bit + i;
        let mask = 1 << (7 - at % 8);
        data[start + at / 8] =
            (data[start + at / 8] & !mask) | (((value >> (1 - i)) & 1) << (7 - at % 8));
    }
}
#[test]
fn palette_fetches_follow_visibility_and_skipped_endpoint_state_carries() {
    for case in edge_cases() {
        let raw = unhex(case["data"]["hex"].as_str().unwrap());
        let start = u32::from_be_bytes(raw[70..74].try_into().unwrap()) as usize;
        let f = format(&case);
        let etc = case["format"].as_u64().unwrap() >= 64;
        let channels = if matches!(f, Format::UnityDxt5Crunched | Format::Etc2Rgba8Crunched) {
            2
        } else {
            1
        };
        for channel in 0..channels {
            // Visible endpoints and selectors must always be checked.
            for bit in [8 + 2 * channel, 8 + 2 * channels + 2 * channel] {
                let mut data = raw.clone();
                set_two(&mut data, start, bit, 2);
                assert!(
                    matches!(
                        unpack(&data, f, (1, 1), 0, &Meter::unlimited()),
                        Err(Error::Palette)
                    ),
                    "{} bit {bit}",
                    case["id"]
                );
            }
            // DXT skips ALL hidden fetches. ETC fetches hidden color
            // endpoints but still skips its hidden alpha endpoint fetch.
            let second = 8 + 4 * channels + if etc { 8 } else { 0 };
            let mut data = raw.clone();
            set_two(&mut data, start, second + 2 * channel, 2);
            let result = unpack(&data, f, (1, 1), 0, &Meter::unlimited());
            if etc && channel == 0 {
                assert!(matches!(result, Err(Error::Palette)));
            } else {
                assert_eq!(
                    result.unwrap().as_slice(),
                    unhex(case["levels"][0]["blocks"].as_str().unwrap())
                );
            }
            // The edge fixture already has out-of-range hidden selectors.
            // Advance of endpoint delta 3 leaves index 2 then 1 on the
            // next visible row, so skipping its state update is wrong.
            if !etc {
                let mut data = raw.clone();
                data[14..16].copy_from_slice(&5u16.to_be_bytes());
                set_two(&mut data, start, second + 2 * channel, 3);
                assert!(matches!(
                    unpack(&data, f, (1, 5), 0, &Meter::unlimited()),
                    Err(Error::Palette)
                ));
            }
        }
    }
}

#[test]
fn headers_and_formats_fail_before_allocating_the_output() {
    use crate::{DecodeError, decode};
    use unityrs_core::budget::{Reservation, Unbounded};
    for case in edge_cases() {
        let raw = unhex(case["data"]["hex"].as_str().unwrap());
        let f = format(&case);
        for (at, bytes) in [
            (0, vec![0]),
            (2, vec![0, 73]),
            (6, vec![0; 4]),
            (16, vec![0]),
            (16, vec![17]),
            (17, vec![6]),
            (18, vec![13]),
            (19, vec![0, 1]),
            (33, vec![0; 3]),
            (39, vec![0; 2]),
            (67, vec![255; 3]),
            (70, vec![255; 4]),
        ] {
            let mut data = raw.clone();
            data[at..at + bytes.len()].copy_from_slice(&bytes);
            let meter = Meter::new(0, Arc::new(Unbounded), Reservation::Input);
            assert!(
                matches!(decode(f, &data, 1, 1, &meter), Err(DecodeError::Crunch(_))),
                "{} header {at}",
                case["id"]
            );
            assert_eq!(meter.used(), 0);
        }
        assert!(matches!(
            unpack(&raw, f, (9, 1), 0, &Meter::unlimited()),
            Err(Error::Header("Texture2D/CRN block extent mismatch"))
        ));
        assert!(matches!(
            unpack(&raw, f, (1, 1), 16, &Meter::unlimited()),
            Err(Error::Header("level index"))
        ));
        let mut wrong = raw.clone();
        wrong[18] = if f == Format::UnityDxt1Crunched { 2 } else { 0 };
        assert!(matches!(
            unpack(&wrong, f, (1, 1), 0, &Meter::unlimited()),
            Err(Error::Header(_))
        ));
        let mut zero = raw;
        zero[12..16].fill(0);
        assert_eq!(
            unpack(&zero, f, (1, 1), 0, &Meter::unlimited())
                .unwrap()
                .as_slice(),
            unhex(case["levels"][0]["blocks"].as_str().unwrap())
        );
    }
}

#[test]
fn fork_allocations_release_charges_at_every_refusal_and_fit_the_plan() {
    use crate::{DecodeError, decode, plan};
    use unityrs_core::budget::{Reservation, Unbounded};
    for case in edge_cases() {
        let data = unhex(case["data"]["hex"].as_str().unwrap());
        let f = format(&case);
        let p = plan(f, data.len(), 1, 1).unwrap();
        assert_eq!(p.input_bytes, 75);
        let mut successes = 0;
        for limit in (0..8192).step_by(32).chain([p.peak_charge()]) {
            let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
            match decode(f, &data, 1, 1, &meter) {
                Ok(image) => {
                    successes += 1;
                    assert_eq!(meter.used(), p.charge());
                    drop(image);
                }
                Err(DecodeError::Charge(_)) => {}
                other => panic!("{} {limit}: {other:?}", case["id"]),
            }
            assert_eq!(meter.used(), 0);
        }
        assert!(successes > 0);
        let mut huge = data;
        for at in [39, 47, 55, 63] {
            if huge[at..at + 2] != [0, 0] {
                huge[at..at + 2].fill(255);
            }
        }
        let meter =
            Meter::with_decode_work(u64::MAX, 100_000, Arc::new(Unbounded), Reservation::Input);
        assert!(matches!(
            decode(f, &huge, 1, 1, &meter),
            Err(DecodeError::Charge(_))
        ));
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn the_initial_etc_diagonal_reference_uses_palette_zero() {
    for case in edge_cases()
        .iter()
        .filter(|c| c["format"].as_u64().unwrap() >= 64)
    {
        let mut data = unhex(case["data"]["hex"].as_str().unwrap());
        let start = u32::from_be_bytes(data[70..74].try_into().unwrap()) as usize;
        data.truncate(start + 1);
        data[start] = 3;
        let len = u32::try_from(data.len()).unwrap().to_be_bytes();
        data[6..10].copy_from_slice(&len);
        let blocks = unpack(&data, format(case), (1, 1), 0, &Meter::unlimited()).unwrap();
        assert_eq!(
            blocks.as_slice(),
            unhex(case["levels"][0]["blocks"].as_str().unwrap())
        );
    }
}

#[test]
fn zero_extended_blocks_consume_work_and_refuse_before_traversal() {
    use crate::{DecodeError, decode};
    use unityrs_core::{
        LimitKind,
        budget::{Reservation, Unbounded},
        limits::ChargeError,
    };
    for case in edge_cases() {
        let mut data = unhex(case["data"]["hex"].as_str().unwrap());
        let start = u32::from_be_bytes(data[70..74].try_into().unwrap()) as usize;
        data.truncate(start + 1);
        data[start] = 0;
        let len = u32::try_from(data.len()).unwrap().to_be_bytes();
        data[6..10].copy_from_slice(&len);
        let mut previous = 0;
        for side in [1u16, 9, 64, 1024] {
            data[12..14].copy_from_slice(&side.to_be_bytes());
            data[14..16].copy_from_slice(&side.to_be_bytes());
            let meter = Meter::unlimited();
            let image = decode(
                format(&case),
                &data,
                i32::from(side),
                i32::from(side),
                &meter,
            )
            .unwrap();
            assert!(meter.decoded() > previous, "{} side {side}", case["id"]);
            previous = meter.decoded();
            drop(image);
            assert_eq!(meter.used(), 0);
        }
        let meter =
            Meter::with_decode_work(u64::MAX, 10_000, Arc::new(Unbounded), Reservation::Input);
        assert!(matches!(decode(format(&case), &data, 1024, 1024, &meter),
            Err(DecodeError::Charge(ChargeError::Limit(l))) if l.kind() == LimitKind::DecodeWork));
        assert!(meter.decoded() <= 10_000);
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn unused_alpha_selectors_are_neither_decoded_nor_charged() {
    for case in edge_cases()
        .iter()
        .filter(|c| c["format"] == 28 || c["format"] == 64)
    {
        let mut data = unhex(case["data"]["hex"].as_str().unwrap());
        let baseline = Meter::unlimited();
        let expected = unpack(&data, format(case), (1, 1), 0, &baseline).unwrap();
        data[49..57].copy_from_slice(&[255, 255, 255, 0, 0, 0, 0, 0]);
        data[57..65].copy_from_slice(&[255, 255, 255, 0, 0, 0, 0, 1]);
        let meter = Meter::unlimited();
        let got = unpack(&data, format(case), (1, 1), 0, &meter).unwrap();
        assert_eq!(got.as_slice(), expected.as_slice());
        assert_eq!(meter.decoded(), baseline.decoded());
    }
}
