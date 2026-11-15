//! Legacy Crunch DXT1 and DXT5 decoding from a `TextureCase`. Dimensions
//! are bounded to keep a valid stream's pixel work small. The meter is the
//! shared memory bound or the plan's conservative peak charge and its
//! neighbours; malformed streams may fail after the length-only plan.

use std::sync::Arc;

use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::limits::{ChargeError, Meter};
use unityrs_testkit::fuzz::TextureCase;
use unityrs_texture::{DecodeError, Format, Plan, decode, plan};

use crate::MEMORY;

/// Runs one input, panicking at an inconsistent answer or leaked charge.
pub fn check(input: &[u8]) {
    let case = TextureCase::decode(input);
    let format = if case.format.rem_euclid(2) == 0 {
        Format::Dxt1Crunched
    } else {
        Format::Dxt5Crunched
    };
    let (width, height) = (case.width.clamp(-4, 128), case.height.clamp(-4, 128));
    let data = &case.data[..case.data.len().min(64 << 10)];
    let planned = plan(format, data.len(), width, height);
    let peak = planned.as_ref().map_or(0, Plan::peak_charge);
    let limit = match case.meter & 3 {
        0 => MEMORY,
        m => (peak + u64::from(m)).saturating_sub(2),
    };
    let meter = Meter::new(limit, Arc::new(Unbounded), Reservation::Input);
    match (decode(format, data, width, height, &meter), planned) {
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
            assert!(limit < p.peak_charge(), "refused within the planned peak");
        }
        (Err(error @ DecodeError::Crunch(_)), Ok(_)) => {
            assert_eq!(meter.used(), 0, "failed decode retained scratch");
            assert_eq!(
                decode(format, data, width, height, &meter).map(|_| ()),
                Err(error),
                "malformed stream's verdict changed on replay"
            );
        }
        (got, planned) => assert_eq!(
            got.map(|_| ()),
            planned.map(|_| ()),
            "the decode's answer is not its plan's"
        ),
    }
    assert_eq!(meter.used(), 0, "the meter is not back at 0");
}
