//! The `texture` target's check: a texture's fields and data
//! (`unityrs_testkit::fuzz::TextureCase`) through `unityrs-texture`'s
//! `select` and `decode_in_as`, as the batch engine runs them
//! (`docs/plans/extract.md` E15, E18, E59, E66), under the memory bound
//! ([`crate::MEMORY`]) or a meter of about the decode's charge.
//!
//! The sixty-four byte-independent formats include Pillow raw and `bcn`,
//! ETC/EAC, ASTC LDR, PVRTC/ATC and Tier B/C raw closed forms. ARGBFloat
//! and odd-width YUY2 exercise the Tier U refusals. Other unknown formats
//! map to a decoded format, and all CRN variants to plain DXT/ETC formats
//! (the `crunch` target checks CRN streams). Besides a panic (libFuzzer's
//! hook aborts at it,
//! before the decoder's containment can catch it), a finding is: a
//! selection that exports a format the texture does not have; a decode
//! whose answer is not its plan's, but for the meter's refusal of a charge
//! past its limit (the peak charge, `Plan::peak_charge`: an ASTC format's
//! tables beside the image); an image whose size, mode or charge is not the plan's;
//! and a meter not back at 0 once the image drops.

use std::sync::Arc;

use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::limits::{ChargeError, Meter};
use unityrs_testkit::fuzz::TextureCase;
use unityrs_texture::{
    DecodeError, Format, Formats, Plan, Selection, TextureFields, array, decode_in_as, plan_in_as,
    platform, select, select_in,
};

use crate::MEMORY;

/// Runs one input, panicking at a finding.
pub fn check(input: &[u8]) {
    let case = TextureCase::decode(input);
    // Exercise the entire 29-entry table even when its pixel decoder is
    // not linked yet. The synthetic seeds hold complete padded GOBs.
    if case.platform == unityrs_texture::SWITCH
        && let Some(exponent) = case
            .blob
            .as_deref()
            .and_then(|b| b.get(8..12))
            .and_then(|b| b.try_into().ok())
            .map(u32::from_le_bytes)
    {
        let format = match case.format {
            3 => 4,
            8 => 14,
            other => other,
        };
        if let Ok(layout) = platform::switch_layout(format, case.width, case.height, exponent) {
            let meter = Meter::new(MEMORY, Arc::new(Unbounded), Reservation::Input);
            let linear = platform::deswizzle(&case.data, &layout, &meter);
            if let Ok(ref linear) = linear {
                assert_eq!(linear.len(), case.data.len());
            }
            drop(linear);
            assert_eq!(meter.used(), 0, "deswizzle leaked its charge");
        }
    }
    let fields = TextureFields {
        texture_format: case.format,
        width: case.width,
        height: case.height,
        platform: case.platform,
        platform_blob: case.blob.as_deref(),
    };
    for selected in [
        select(&fields),
        select_in(&fields, Formats::DECODED, (2021, 3)),
    ] {
        if let Selection::Export(f) = selected {
            assert_eq!(
                f.texture_format(),
                case.format,
                "exported as another format"
            );
        }
    }
    let len = i32::try_from(Format::ALL.len()).unwrap_or(1);
    let any = usize::try_from(case.format.rem_euclid(len)).unwrap_or(0);
    let format = if case.format == 6 {
        Some(Format::ArgbFloat)
    } else {
        Format::from_texture_format(case.format)
    }
    .or_else(|| Format::ALL.get(any).copied())
    .unwrap_or(Format::Rgba32);
    // Crunch's length-only plan cannot validate a CRN stream; its own
    // target checks those later malformed-stream errors. Keep this
    // target's byte-independent plan equality on the plain decoders.
    let format = match format {
        Format::Dxt1Crunched | Format::UnityDxt1Crunched => Format::Dxt1,
        Format::Dxt5Crunched | Format::UnityDxt5Crunched => Format::Dxt5,
        Format::EtcRgb4Crunched => Format::EtcRgb4,
        Format::Etc2Rgba8Crunched => Format::Etc2Rgba8,
        other => other,
    };
    let fields = TextureFields {
        texture_format: format.texture_format(),
        width: case.width,
        height: case.height,
        platform: case.platform,
        platform_blob: case.blob.as_deref(),
    };
    let planned = plan_in_as(format, &fields, case.data.len());
    let charge = planned.as_ref().map_or(0, Plan::peak_charge);
    // The memory bound, or the peak charge less 1, the charge, or plus 1.
    let limit = match case.meter & 3 {
        0 => MEMORY,
        m => (charge + u64::from(m)).saturating_sub(2),
    };
    let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
    match (decode_in_as(format, &fields, &case.data, &meter), planned) {
        (Ok(image), Ok(p)) => {
            assert_eq!(
                u64::try_from(image.pixels().len()).ok(),
                Some(p.output_bytes)
            );
            assert_eq!(
                (image.width(), image.height(), image.mode()),
                (p.width, p.height, p.mode)
            );
            assert_eq!(image.charge().bytes(), p.charge());
            assert_eq!(meter.used(), p.charge());
        }
        (Err(DecodeError::Charge(ChargeError::Limit(_))), Ok(p)) => {
            assert!(limit < p.peak_charge(), "refused within the charge");
        }
        (got, planned) => assert_eq!(
            got.map(|_| ()),
            planned.map(|_| ()),
            "the decode's answer is not its plan's"
        ),
    }
    assert_eq!(meter.used(), 0, "the meter is not back at 0");
    // The same mutated dimensions/platform also reach array layout and its
    // independent platform refusals. Depth and declared data size can both
    // be hostile, independent of the available slice bytes.
    let blob = case.blob.as_deref().unwrap_or_default();
    let depth = blob
        .get(..4)
        .and_then(|b| b.try_into().ok())
        .map_or(1, i32::from_le_bytes);
    let data_size = blob
        .get(4..12)
        .and_then(|b| b.try_into().ok())
        .map_or(case.data.len() as u64, u64::from_le_bytes);
    let fields = array::ArrayFields {
        graphics_format: case.format,
        width: case.width,
        height: case.height,
        platform: case.platform,
        depth,
        data_size,
    };
    let meter = Meter::new(MEMORY, Arc::new(Unbounded), Reservation::Input);
    let result = array::decode(&fields, &case.data, None, &meter);
    drop(result);
    assert_eq!(meter.used(), 0, "array decode leaked its charge");
}
