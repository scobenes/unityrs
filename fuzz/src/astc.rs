//! The `astc` target's check: an ASTC LDR texture's size and blocks
//! (`unityrs_testkit::fuzz::TextureCase`, its format taken as one of the
//! twelve ASTC LDR formats) through `unityrs-texture`'s `decode`, under the
//! memory bound ([`crate::MEMORY`]) or a meter of about the decode's peak
//! charge, its output and its tables (`Plan::peak_charge`).
//!
//! Besides a panic, a finding is: a decode whose answer is not its plan's,
//! but for the meter's refusal of a charge past its limit; an image whose
//! size, mode or charge is not the plan's; a meter not back at 0 once the
//! image drops, so the tables are returned; and an image whose texels are
//! not each block's decoded alone, as a one-block texture: the assembly,
//! the cut at the right and bottom edges and UnityPy's flip must place
//! every block's texels where its own decode puts them.

use std::sync::Arc;

use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::limits::{ChargeError, Meter};
use unityrs_testkit::fuzz::TextureCase;
use unityrs_texture::{DecodeError, Format, Image, Plan, decode, plan};

use crate::MEMORY;

/// The blocks of an image that are each decoded alone, at least.
const ALONE: usize = 32;

/// The twelve ASTC LDR formats, in `TextureFormat` order.
fn formats() -> impl Iterator<Item = Format> {
    Format::ALL
        .into_iter()
        .filter(|f| f.name().starts_with("ASTC_"))
}

/// Runs one input, panicking at a finding.
pub fn check(input: &[u8]) {
    let case = TextureCase::decode(input);
    let count = i32::try_from(formats().count()).unwrap_or(1);
    let any = usize::try_from(case.format.rem_euclid(count)).unwrap_or(0);
    let format = formats().nth(any).unwrap_or(Format::AstcRgba4x4);
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
            blocks_alone(format, &case.data, &image);
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

/// The blocks of `image` against each decoded alone: rows bottom first in
/// both (UnityPy's flip), the image's cut at its edges. Every block of an
/// image of at most [`ALONE`] blocks; of a larger one, the first [`ALONE`]
/// and every block of the last column and row, the ones the edges cut (each
/// block alone builds its own tables, which a large image's every block
/// would make slow).
fn blocks_alone(format: Format, data: &[u8], image: &Image) {
    let Some((bw, bh)) = format.block_size() else {
        return;
    };
    let (bw, bh) = (usize::from(bw), usize::from(bh));
    let w = usize::try_from(image.width()).unwrap_or(0);
    let h = usize::try_from(image.height()).unwrap_or(0);
    let across = w.div_ceil(bw);
    let rows = h.div_ceil(bh);
    let pixels = image.pixels();
    for (i, block) in data
        .as_chunks::<16>()
        .0
        .iter()
        .take(across * rows)
        .enumerate()
    {
        if i >= ALONE && i % across != across - 1 && i / across != rows - 1 {
            continue;
        }
        let alone = decode(
            format,
            block,
            i32::try_from(bw).unwrap_or(4),
            i32::try_from(bh).unwrap_or(4),
            &Meter::unlimited(),
        )
        .expect("a block decodes alone");
        let (x0, y0) = ((i % across) * bw, (i / across) * bh);
        let n = bw.min(w - x0);
        for y in 0..bh.min(h - y0) {
            let at = (h - 1 - (y0 + y)) * w + x0;
            let ours = pixels.get(4 * at..4 * (at + n));
            let own = (bh - 1 - y) * bw;
            let theirs = alone.pixels().get(4 * own..4 * (own + n));
            assert_eq!(ours, theirs, "block {i} row {y} is not the block's own");
        }
    }
}
