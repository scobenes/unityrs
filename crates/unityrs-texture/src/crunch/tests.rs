use super::symbol::{Bits, Model};
use unityrs_core::limits::Meter;

#[test]
fn builder_streams_unpack_to_their_known_blocks_at_every_level() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/crunch_native.json")).unwrap();
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .chain(fixture["truncated_cases"].as_array().unwrap())
        .chain(fixture["data_size_cases"].as_array().unwrap())
        .chain(fixture["edge_block_cases"].as_array().unwrap())
        .chain(fixture["descriptor_cases"].as_array().into_iter().flatten())
    {
        let bytes = unhex(case["data"]["hex"].as_str().unwrap());
        let header = super::header::Header::parse(&bytes).unwrap();
        let meter = Meter::unlimited();
        for (level, mip) in case["levels"].as_array().unwrap().iter().enumerate() {
            let w = usize::try_from(mip["width"].as_u64().unwrap()).unwrap();
            let h = usize::try_from(mip["height"].as_u64().unwrap()).unwrap();
            let blocks = super::unpack(&bytes, header.dxt5, (w, h), level, &meter).unwrap();
            assert_eq!(
                blocks.as_slice(),
                unhex(mip["blocks"].as_str().unwrap()),
                "{} level {level}",
                case["id"]
            );
            drop(blocks);
            assert_eq!(meter.used(), 0);
        }
    }
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&s[at..at + 2], 16).unwrap())
        .collect()
}

#[test]
fn skipped_blocks_advance_selectors_without_fetching_palette_entries() {
    let doc: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/crunch_native.json")).unwrap();
    let cases = doc["edge_block_cases"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    for case in cases {
        let data = unhex(case["data"]["hex"].as_str().unwrap());
        let dxt5 = case["format"] == 29;
        let meter = Meter::unlimited();
        let blocks = super::unpack(&data, dxt5, (1, 1), 0, &meter)
            .unwrap_or_else(|error| panic!("{}: {error}", case["id"]));
        assert_eq!(
            blocks.as_slice(),
            unhex(if dxt5 {
                "ff014a04094a040920f8e10f96289628"
            } else {
                "20f8e10f96289628"
            }),
            "{}",
            case["id"]
        );
        drop(blocks);
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn palette_entries_are_validated_where_the_reference_fetches_them() {
    let doc: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/crunch_native.json")).unwrap();
    for case in doc["edge_block_cases"].as_array().unwrap() {
        let data = unhex(case["data"]["hex"].as_str().unwrap());
        let dxt5 = case["format"] == 29;
        let start = usize::try_from(u32::from_be_bytes(data[70..74].try_into().unwrap())).unwrap();
        // Layout 7 has four endpoints per channel. All delta codes in
        // these fixtures have length 2, after the 9-bit layout symbol.
        let channels = if dxt5 { 2 } else { 1 };
        let selectors = 9 + channels * 4 * 2;
        let endpoint_bits = (0..channels * 4).map(|i| 9 + i * 2);
        let visible_selector_bits = (0..channels).map(|i| selectors + i * 2);
        for bit in endpoint_bits.chain(visible_selector_bits) {
            let mut bad = data.clone();
            // Change delta 0 (00) to 2 (10): subtracting one still
            // leaves index 1 outside its singleton palette.
            bad[start + bit / 8] |= 1 << (7 - bit % 8);
            let meter = Meter::unlimited();
            assert!(
                matches!(
                    super::unpack(&bad, dxt5, (1, 1), 0, &meter),
                    Err(super::CrunchError::Palette)
                ),
                "{} bit {bit}",
                case["id"]
            );
            assert_eq!(meter.used(), 0);
        }
        for channel in 0..channels {
            let mut bad = data.clone();
            bad[14..16].copy_from_slice(&5u16.to_be_bytes());
            // With the bottom-left block visible, a skipped right block's
            // delta 3 must carry forward: indices 0, 2, 1, ... . Merely
            // skipping that delta would incorrectly accept the next block.
            let bit = selectors + channels * 2 + channel * 2;
            for offset in 0..2 {
                bad[start + (bit + offset) / 8] |= 1 << (7 - (bit + offset) % 8);
            }
            let meter = Meter::unlimited();
            assert!(matches!(
                super::unpack(&bad, dxt5, (1, 5), 0, &meter),
                Err(super::CrunchError::Palette)
            ));
            assert_eq!(meter.used(), 0);
        }
    }
}

#[test]
fn crunch_selection_uses_the_version_and_keeps_platform_gates() {
    use crate::{Format, Selection, Skip, SkippedPlatform, TextureFields};
    let mut fields = TextureFields {
        texture_format: 28,
        width: 4,
        height: 4,
        platform: 19,
        platform_blob: None,
    };
    assert_eq!(
        crate::select_crunch(&fields, (2017, 2)),
        Selection::Export(Format::Dxt1Crunched)
    );
    for version in [(2017, 3), (2020, 1)] {
        assert_eq!(
            crate::select_crunch(&fields, version),
            Selection::Export(Format::UnityDxt1Crunched)
        );
    }
    fields.texture_format = 29;
    assert_eq!(
        crate::select_crunch(&fields, (5, 6)),
        Selection::Export(Format::Dxt5Crunched)
    );
    fields.platform = crate::XBOX360;
    assert_eq!(
        crate::select_crunch(&fields, (5, 6)),
        Selection::Skip(Skip::UnsupportedPlatform(SkippedPlatform::Xbox360))
    );
    fields.platform = crate::SWITCH;
    fields.platform_blob = Some(&[0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]);
    assert_eq!(
        crate::select_crunch(&fields, (5, 6)),
        Selection::Skip(Skip::UnsupportedPlatform(SkippedPlatform::Switch))
    );
    fields.platform = 19;
    fields.platform_blob = None;
    fields.texture_format = 29;
    assert_eq!(
        crate::select_crunch(&fields, (2017, 3)),
        Selection::Export(Format::UnityDxt5Crunched)
    );
    for (value, format) in [
        (64, Format::EtcRgb4Crunched),
        (65, Format::Etc2Rgba8Crunched),
    ] {
        fields.texture_format = value;
        for version in [(5, 6), (2017, 3), (2020, 1)] {
            assert_eq!(
                crate::select_crunch(&fields, version),
                Selection::Export(format)
            );
        }
        fields.platform = crate::XBOX360;
        assert_eq!(
            crate::select_crunch(&fields, (5, 6)),
            Selection::Export(format)
        );
        fields.platform = 19;
    }
    fields.width = 0;
    assert_eq!(
        crate::select_crunch(&fields, (2020, 1)),
        Selection::Skip(Skip::EmptyImage)
    );
}

#[test]
fn allow_list_selection_checks_crunch_version_before_platform() {
    use crate::{Format, Formats, Selection, Skip, SkippedPlatform, TextureFields, select_in};
    let mut fields = TextureFields {
        texture_format: 28,
        width: 1,
        height: 1,
        platform: crate::XBOX360,
        platform_blob: None,
    };
    for (legacy, modern) in [
        (Format::Dxt1Crunched, Format::UnityDxt1Crunched),
        (Format::Dxt5Crunched, Format::UnityDxt5Crunched),
        (Format::EtcRgb4Crunched, Format::EtcRgb4Crunched),
        (Format::Etc2Rgba8Crunched, Format::Etc2Rgba8Crunched),
    ] {
        fields.texture_format = legacy.texture_format();
        let allowed = Formats::NONE.with(legacy);
        for version in [
            (5, 6),
            (2017, 2),
            (2017, 3),
            (2022, 3),
            (u16::MAX, u16::MAX),
        ] {
            fields.platform = 19;
            let want = Selection::Export(if version < (2017, 3) { legacy } else { modern });
            assert_eq!(select_in(&fields, allowed, version), want);
            assert_eq!(select_in(&fields, Formats::DECODED, version), want);
            assert_eq!(
                select_in(&fields, Formats::NONE.with(modern), version),
                want
            );
            assert_eq!(
                select_in(&fields, Formats::NONE, version),
                Selection::Skip(Skip::UnsupportedFormat(fields.texture_format))
            );
            assert_eq!(
                crate::select(&fields),
                Selection::Skip(Skip::UnsupportedFormat(fields.texture_format))
            );
            fields.platform = crate::XBOX360;
            assert_eq!(
                select_in(&fields, allowed, version),
                if fields.texture_format < 30 {
                    Selection::Skip(Skip::UnsupportedPlatform(SkippedPlatform::Xbox360))
                } else {
                    want
                }
            );
            fields.platform = crate::SWITCH;
            fields.platform_blob = Some(&[0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]);
            assert_eq!(
                select_in(&fields, allowed, version),
                Selection::Skip(Skip::UnsupportedPlatform(SkippedPlatform::Switch))
            );
            assert_eq!(
                select_in(&fields, Formats::NONE, version),
                Selection::Skip(Skip::UnsupportedFormat(fields.texture_format))
            );
            fields.width = 0;
            assert_eq!(
                select_in(&fields, Formats::NONE, version),
                Selection::Skip(Skip::EmptyImage)
            );
            fields.width = 1;
            fields.platform_blob = None;
        }
    }
}

#[test]
fn canonical_codes_and_incomplete_trees() {
    let meter = Meter::unlimited();
    let model = Model::new(&[1, 3, 3, 3, 3], &meter).unwrap();
    let mut bits = Bits::new(&[0b0100_1011, 0b1011_1000]).unwrap();
    for expected in [0, 1, 2, 3, 4, 0] {
        assert_eq!(model.decode(&mut bits).unwrap(), expected);
    }
    assert!(Model::new(&[1, 1, 1], &meter).is_err());
    assert!(Model::new(&[0, 0], &meter).is_err());
    assert!(Model::new(&[17], &meter).is_err());
    let model = Model::new(&[2, 2], &meter).unwrap();
    let mut bits = Bits::new(&[0xff]).unwrap();
    assert_eq!(model.decode(&mut bits).unwrap(), 0);
    assert_eq!(
        bits.read(8).unwrap(),
        0xff,
        "unused codeword consumes no bits"
    );
}

#[test]
fn bit_reads_are_msb_first_and_zero_extend_truncation() {
    let mut bits = Bits::new(&[0x96, 0x35, 0xab]).unwrap();
    assert_eq!(bits.read(4).unwrap(), 9);
    assert_eq!(bits.read(12).unwrap(), 0x635);
    assert_eq!(bits.read(8).unwrap(), 0xab);
    assert_eq!(bits.read(32).unwrap(), 0);
    assert!(Bits::new(&[]).is_err());
}

fn first_stream() -> Vec<u8> {
    let doc: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/crunch_native.json")).unwrap();
    unhex(doc["cases"][0]["data"]["hex"].as_str().unwrap())
}

#[test]
fn header_validation_rejects_bad_ranges_counts_faces_and_truncation() {
    use super::{CrunchError as E, header::Header};
    let valid = first_stream();
    assert!(Header::parse(&valid).is_ok());
    for end in 0..valid.len() {
        assert!(Header::parse(&valid[..end]).is_err(), "prefix {end}");
    }
    for (at, bytes) in [
        (0, vec![0, 0]),
        (2, vec![0, 73]),
        (2, vec![255, 255]),
        (6, vec![255; 4]),
        (6, vec![0; 4]),
        (16, vec![0]),
        (16, vec![17]),
        (18, vec![255]),
        (33, vec![255; 3]),
        (36, vec![255; 3]),
        (39, vec![0, 0]),
        (65, vec![255, 255]),
        (67, vec![255; 3]),
        (70, vec![255; 4]),
        (70, vec![0; 4]),
    ] {
        let mut data = valid.clone();
        data[at..at + bytes.len()].copy_from_slice(&bytes);
        assert!(Header::parse(&data).is_err(), "at {at}");
    }
    let mut data = valid.clone();
    data[17] = 6;
    assert!(matches!(Header::parse(&data), Err(E::Faces(6))));
    data[17] = 1;
    data[20] = 1;
    assert!(matches!(Header::parse(&data), Err(E::Segmented)));
    // Wrapper uses get_header, not validate_file, so CRC is not enforced.
    let mut data = valid;
    data[4..6].fill(0);
    data[10..12].fill(0);
    assert!(Header::parse(&data).is_ok());
}

fn put(bits: &mut Vec<u8>, value: u32, count: u8) {
    for shift in (0..count).rev() {
        bits.push(u8::try_from((value >> shift) & 1).unwrap());
    }
}

fn pack(bits: &[u8]) -> Vec<u8> {
    bits.chunks(8)
        .map(|chunk| {
            chunk
                .iter()
                .enumerate()
                .fold(0, |n, (i, b)| n | (b << (7 - i)))
        })
        .collect()
}

#[test]
fn received_models_cover_run_boundaries_and_hostile_alphabets() {
    let meter = Meter::unlimited();
    // Secondary alphabet has 21 five-bit codes. Spell the target lengths
    // [8;80], [0;148], [8;11], exercising every run's maximum extra bits
    // and the minimum repeat lengths (3 and 7).
    let mut bits = Vec::new();
    let total = 80 + 148 + 11;
    put(&mut bits, total, 14);
    put(&mut bits, 21, 5);
    for _ in 0..21 {
        put(&mut bits, 5, 3);
    }
    for (symbol, extra, width) in [
        (8, 0, 0),
        (20, 63, 6),
        (19, 3, 2),
        (8, 0, 0),
        (8, 0, 0),
        (8, 0, 0),
        (18, 127, 7),
        (17, 7, 3),
        (8, 0, 0),
        (19, 0, 2),
        (20, 0, 6),
    ] {
        put(&mut bits, symbol, 5);
        put(&mut bits, extra, width);
    }
    let bytes = pack(&bits);
    let received = Model::receive(&mut Bits::new(&bytes).unwrap(), &meter).unwrap();
    let lengths: Vec<u8> = [vec![8; 80], vec![0; 148], vec![8; 11]].concat();
    let expected = Model::new(&lengths, &meter).unwrap();
    for code in 0u8..91 {
        let bytes = [code];
        assert_eq!(
            received.decode(&mut Bits::new(&bytes).unwrap()),
            expected.decode(&mut Bits::new(&bytes).unwrap())
        );
    }
    for count in [0, 8193, 16383] {
        let mut bits = Vec::new();
        put(&mut bits, count, 14);
        assert!(Model::receive(&mut Bits::new(&pack(&bits)).unwrap(), &meter).is_err());
    }
    let maximum = Model::new(&[13; 8192], &meter).unwrap();
    assert_eq!(
        maximum
            .decode(&mut Bits::new(&[0xff, 0xf8]).unwrap())
            .unwrap(),
        8191
    );
    assert!(Model::new(&[13; 8193], &meter).is_err());
    let long = Model::new(&[16], &meter).unwrap();
    assert_eq!(long.decode(&mut Bits::new(&[0, 0]).unwrap()).unwrap(), 0);
    assert_eq!(long.decode(&mut Bits::new(&[0]).unwrap()).unwrap(), 0);
}

#[test]
fn streams_never_panic_and_every_meter_refusal_releases_scratch() {
    use crate::{DecodeError, Format, decode, plan};
    use std::sync::Arc;
    use unityrs_core::budget::{Reservation, Unbounded};
    let data = first_stream();
    let p = plan(Format::Dxt1Crunched, data.len(), 1, 1).unwrap();
    assert!(p.scratch > unityrs_core::limits::heap_cost(8));
    // A fine-grained sweep trips the output, model, nested length tables,
    // palette and unpacked block allocations in turn.
    let mut successes = 0;
    for limit in (0..4096).step_by(16).chain([p.peak_charge()]) {
        let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
        match decode(Format::Dxt1Crunched, &data, 1, 1, &meter) {
            Ok(image) => {
                successes += 1;
                assert_eq!(meter.used(), p.charge());
                drop(image);
            }
            Err(DecodeError::Charge(_)) => {}
            other => panic!("{limit}: {other:?}"),
        }
        assert_eq!(meter.used(), 0, "{limit}");
    }
    assert!(successes > 0);
    // Updating the declared length reaches the bit parser, not just the
    // outer header's data-size check, for every truncated stream.
    for len in 75..data.len() - 1 {
        let mut cut = data[..len].to_vec();
        cut[6..10].copy_from_slice(&u32::try_from(len).unwrap().to_be_bytes());
        let meter = Meter::unlimited();
        assert!(
            matches!(
                decode(Format::Dxt1Crunched, &cut, 1, 1, &meter),
                Err(DecodeError::Crunch(_))
            ),
            "prefix {len}"
        );
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn empty_level_and_bad_header_are_rejected_before_output_allocation() {
    use crate::{DecodeError, Format, decode};
    use std::sync::Arc;
    use unityrs_core::budget::{Reservation, Unbounded};
    for empty_level in [false, true] {
        let mut data = first_stream();
        if empty_level {
            let end = data.len().to_be_bytes();
            data[70..74].copy_from_slice(&end[end.len() - 4..]);
        } else {
            data[0] = 0;
        }
        let meter = Meter::new(0, Arc::new(Unbounded), Reservation::Input);
        assert!(matches!(
            decode(Format::Dxt1Crunched, &data, 1, 1, &meter),
            Err(DecodeError::Crunch(_))
        ));
        assert_eq!(meter.used(), 0);
    }
}

#[test]
fn review_streams_match_the_reference_blocks() {
    let doc: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/crunch_review.json")).unwrap();
    for case in doc["cases"].as_array().unwrap() {
        let data = unhex(case["data"].as_str().unwrap());
        let blocks = super::unpack(
            &data,
            case["dxt5"].as_bool().unwrap(),
            (8, 8),
            0,
            &Meter::unlimited(),
        )
        .unwrap();
        assert_eq!(
            blocks.as_slice(),
            unhex(case["blocks"].as_str().unwrap()),
            "{}",
            case["id"]
        );
    }
}

#[test]
fn a_tiny_zero_extended_level_has_no_dimension_dependent_input_bound() {
    use crate::{DecodeError, Format, decode, plan};
    use std::sync::Arc;
    use unityrs_core::budget::{Reservation, Unbounded};
    let doc: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/crunch_review.json")).unwrap();
    let mut data = unhex(doc["cases"][0]["data"].as_str().unwrap());
    data[12..16].copy_from_slice(&[0x40, 0, 0x40, 0]);
    assert_eq!(data.len(), 187);
    let p = plan(Format::Dxt5Crunched, data.len(), 16384, 16384).unwrap();
    assert_eq!(p.input_bytes, 75);
    let meter = Meter::new(4096, Arc::new(Unbounded), Reservation::Input);
    assert!(matches!(
        decode(Format::Dxt5Crunched, &data, 16384, 16384, &meter),
        Err(DecodeError::Charge(_))
    ));
    assert_eq!(meter.used(), 0);
    // Removing the sole level byte must fail before even the RGBA charge,
    // rather than report the much larger output's memory limit first.
    data.pop();
    let len = u32::try_from(data.len()).unwrap().to_be_bytes();
    data[6..10].copy_from_slice(&len);
    assert!(matches!(
        decode(Format::Dxt5Crunched, &data, 16384, 16384, &meter),
        Err(DecodeError::Crunch(_))
    ));
    assert_eq!(meter.used(), 0);
}

#[test]
fn zero_header_dimensions_are_clamped_as_in_the_reference() {
    let mut data = first_stream();
    for range in [12..14, 14..16, 12..16] {
        data[range].fill(0);
        let blocks = super::unpack(&data, false, (1, 1), 0, &Meter::unlimited()).unwrap();
        // Legacy reference output for the 1x1 fixture, also with zero sides.
        assert_eq!(blocks.as_slice(), unhex("20f8e10f96289628"));
    }
}

#[test]
fn remaining_header_divergences_are_explicit_refusals() {
    use super::{CrunchError as E, header::Header};
    // Each mutation returns level-0 blocks in the legacy 1.0.5 reference:
    // unused offsets and declared header/data sizes do not affect its reads.
    let doc: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/crunch_native.json")).unwrap();
    let multi = unhex(doc["cases"][5]["data"]["hex"].as_str().unwrap());
    for (at, value, reason) in [
        (16, vec![17], "level count"),
        (2, vec![0, 74], "level count"),
        (78, multi[70..74].to_vec(), "level offset/order"),
    ] {
        let mut data = multi.clone();
        data[at..at + value.len()].copy_from_slice(&value);
        assert!(matches!(Header::parse(&data), Err(E::Header(why)) if why == reason));
    }
    for (at, value) in [
        (2, vec![255; 2]),
        (6, vec![0; 4]),
        (
            6,
            u32::try_from(first_stream().len() - 2)
                .unwrap()
                .to_be_bytes()
                .to_vec(),
        ),
    ] {
        let mut data = first_stream();
        data[at..at + value.len()].copy_from_slice(&value);
        assert!(Header::parse(&data).is_err());
    }
}

#[test]
fn last_level_reads_to_the_supplied_buffer_end() {
    let doc: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/crunch_native.json")).unwrap();
    let case = &doc["cases"][3];
    assert_eq!(case["id"], "DXT1Crunched-7x9-p3-m1");
    let mut data = unhex(case["data"]["hex"].as_str().unwrap());
    assert_eq!(data.len(), 188);
    data[6..10].copy_from_slice(&185u32.to_be_bytes());
    let blocks = super::unpack(&data, false, (7, 9), 0, &Meter::unlimited()).unwrap();
    assert_eq!(&blocks.as_slice()[..8], unhex("20f8e10fc823c823"));
    assert_eq!(
        blocks.as_slice(),
        unhex(case["levels"][0]["blocks"].as_str().unwrap())
    );
}

#[test]
fn unassigned_palette_codewords_finish_at_the_declared_count() {
    // Both endpoint models assign only 00. All endpoint reads see 11,
    // return zero without advancing, and must still finish at the count.
    let mut encoded = Vec::new();
    for _ in 0..2 {
        put(&mut encoded, 1, 14);
        put(&mut encoded, 17, 5);
        for slot in 0..17 {
            // ORDER[16] = 2: a singleton secondary code for length 2.
            put(&mut encoded, u32::from(slot == 16), 3);
        }
        put(&mut encoded, 0, 1);
    }
    put(&mut encoded, 0xffff, 16);
    let data = pack(&encoded);
    let mut header = super::header::Header::parse(&first_stream()).unwrap();
    header.palettes.fill(super::header::Region::default());
    header.palettes[0] = super::header::Region {
        start: 0,
        len: data.len(),
        count: 37,
    };
    let meter = Meter::unlimited();
    let palettes = super::palettes::Palettes::new(&header, &data, &meter).unwrap();
    assert_eq!(palettes.colors.as_slice(), &[0; 37]);
    drop(palettes);
    assert_eq!(meter.used(), 0);
}

#[test]
fn header_palette_work_is_refused_for_a_tiny_output() {
    use crate::{DecodeError, Format, decode, plan};
    use std::sync::Arc;
    use unityrs_core::{
        budget::{Reservation, Unbounded},
        error::LimitKind,
        limits::ChargeError,
    };
    let doc: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/crunch_review.json")).unwrap();
    let mut data = unhex(doc["cases"][0]["data"].as_str().unwrap());
    assert_eq!(data.len(), 187);
    data[12..16].copy_from_slice(&[0, 1, 0, 1]);
    for at in [39, 47, 55, 63] {
        data[at..at + 2].fill(255);
    }
    assert_eq!(
        plan(Format::Dxt5Crunched, data.len(), 1, 1)
            .unwrap()
            .output_bytes,
        4
    );
    let meter = Meter::with_decode_work(u64::MAX, 100_000, Arc::new(Unbounded), Reservation::Input);
    assert!(matches!(decode(Format::Dxt5Crunched, &data, 1, 1, &meter),
        Err(DecodeError::Charge(ChargeError::Limit(l))) if l.kind() == LimitKind::DecodeWork));
    assert_eq!(meter.used(), 0);
}

#[test]
fn model_alphabet_work_is_refused_before_expanding_lengths() {
    use std::sync::Arc;
    use unityrs_core::{
        budget::{Reservation, Unbounded},
        error::LimitKind,
        limits::ChargeError,
    };
    let mut encoded = Vec::new();
    put(&mut encoded, 16383, 14);
    let data = pack(&encoded);
    let meter = Meter::with_decode_work(u64::MAX, 100, Arc::new(Unbounded), Reservation::Input);
    assert!(
        matches!(Model::receive(&mut Bits::new(&data).unwrap(), &meter),
        Err(super::CrunchError::Charge(ChargeError::Limit(l))) if l.kind() == LimitKind::DecodeWork)
    );
    assert_eq!(meter.used(), 0);
}

#[test]
fn legacy_unused_alpha_selectors_are_neither_decoded_nor_charged() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/crunch_native.json")).unwrap();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["format"] == 28)
        .unwrap();
    let mut data = unhex(case["data"]["hex"].as_str().unwrap());
    let size = (
        usize::try_from(case["width"].as_u64().unwrap()).unwrap(),
        usize::try_from(case["height"].as_u64().unwrap()).unwrap(),
    );
    let baseline = Meter::unlimited();
    let expected = super::unpack(&data, false, size, 0, &baseline).unwrap();
    // Both ignored descriptors may name no bytes in the input. The selector
    // count is nonzero; only the alpha endpoint count gates its use.
    data[49..57].copy_from_slice(&[255, 255, 255, 0, 0, 0, 0, 0]);
    data[57..65].copy_from_slice(&[255, 255, 255, 0, 0, 0, 0, 1]);
    let meter = Meter::unlimited();
    let got = super::unpack(&data, false, size, 0, &meter).unwrap();
    assert_eq!(got.as_slice(), expected.as_slice());
    assert_eq!(meter.decoded(), baseline.decoded());
}
