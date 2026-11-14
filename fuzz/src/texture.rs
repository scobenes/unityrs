//! The `texture` target's check: a texture's fields and data
//! (`unityrs_testkit::fuzz::TextureCase`) through `unityrs-texture`'s
//! `select` and `decode`, as the batch engine runs them
//! (`docs/plans/extract.md` E15, E18, E59, E66), under the memory bound
//! ([`crate::MEMORY`]) or a meter of about the decode's charge.
//!
//! A format outside the thirty-six the crate decodes (the five version 1
//! exports, the rest of Pillow's raw and `bcn` formats, the ETC and EAC
//! formats, and the ASTC LDR formats) decodes as one of them, so every
//! input reaches a decoder. Besides a panic (libFuzzer's hook aborts at it,
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
use unityrs_texture::{DecodeError, Format, Plan, Selection, TextureFields, decode, plan, select};

use crate::MEMORY;

/// Runs one input, panicking at a finding.
pub fn check(input: &[u8]) {
    let case = TextureCase::decode(input);
    let selected = select(&TextureFields {
        texture_format: case.format,
        width: case.width,
        height: case.height,
        platform: case.platform,
        platform_blob: case.blob.as_deref(),
    });
    if let Selection::Export(f) = selected {
        assert_eq!(
            Some(f),
            Format::from_texture_format(case.format),
            "exported as another format"
        );
    }
    let len = i32::try_from(Format::ALL.len()).unwrap_or(1);
    let any = usize::try_from(case.format.rem_euclid(len)).unwrap_or(0);
    let format = Format::from_texture_format(case.format)
        .or_else(|| Format::ALL.get(any).copied())
        .unwrap_or(Format::Rgba32);
    let planned = plan(format, case.data.len(), case.width, case.height);
    let charge = planned.as_ref().map_or(0, Plan::peak_charge);
    // The memory bound, or the peak charge less 1, the charge, or plus 1.
    let limit = match case.meter & 3 {
        0 => MEMORY,
        m => (charge + u64::from(m)).saturating_sub(2),
    };
    let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
    match (
        decode(format, &case.data, case.width, case.height, &meter),
        planned,
    ) {
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
}
