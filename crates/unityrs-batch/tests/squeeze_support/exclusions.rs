//! The scenes X13 keeps out until CR7 (`docs/plans/batch.md` section 6;
//! `docs/BATCH_SPEC.md` 4.3's gaps), exactly: each with its scene, its
//! input, the pass it leaves and the structure that puts it there. Below
//! one level a non-transient `Limit` or a parse failure leaves a container
//! unreached without naming its reason, so a squeezed `M` can change a
//! record no reason names (the depth-two gap: the `M` pass); and at a
//! dropped container row the core's settle swallows such a failure and
//! retries a refusal at the next lookup, its evictions counted (both memory
//! passes). [`inventory`] is checked against [`detect`] over the whole
//! catalogue, so no scene is left out by name alone, and none silently.

use std::collections::BTreeSet;
use std::sync::Arc;

use unityrs_core::budget::Reservation;
use unityrs_core::detect::{FileType, check_file_type};
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::pystr::simplify_name;
use unityrs_core::{BundleFile, Bytes, FileContext, RawStr};
use unityrs_testkit::ngui::AtlasScene;

/// One of 4.3's gaps that CR7 closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Gap {
    /// A container two levels down in a bundle of the view: the `M` pass.
    DepthTwo,
    /// A top-level container row whose registry key a later row of its
    /// bundle takes: the `M` and `D` passes.
    DroppedContainer,
}

/// One exclusion: active while the core has no CR7 hint, and only for
/// `input`'s view of `scene`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Exclusion {
    pub scene: &'static str,
    pub input: &'static str,
    pub gap: Gap,
}

impl Exclusion {
    /// Whether it keeps the `M` pass out: both gaps do.
    #[must_use]
    pub fn m(self) -> bool {
        matches!(self.gap, Gap::DepthTwo | Gap::DroppedContainer)
    }

    /// Whether it keeps the `D` pass out.
    #[must_use]
    pub fn d(self) -> bool {
        self.gap == Gap::DroppedContainer
    }
}

/// The activation condition every exclusion shares: it holds until CR7
/// lands, which marks such rows from the engine's peek and reaches them
/// below depth 1. Then this inventory empties and X13 runs them all.
pub const UNTIL: &str = "until CR7 (docs/plans/batch.md section 6) lands";

/// The exclusions, by scene and input, each with its gap.
#[must_use]
pub fn inventory() -> BTreeSet<Exclusion> {
    [("fallback_rows", "x.bundle", Gap::DepthTwo)]
        .into_iter()
        .map(|(scene, input, gap)| Exclusion { scene, input, gap })
        .collect()
}

fn meter() -> (Arc<FileContext>, Arc<Meter>) {
    let ctx = Arc::new(FileContext::unitypy());
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    (ctx, meter)
}

fn open(name: &str, data: Bytes) -> Option<Arc<BundleFile>> {
    let (ctx, meter) = meter();
    BundleFile::open(RawStr::from(name), data, None, &meter, &ctx).ok()
}

/// Each top-level row of `b` that is a container, by index, with its bytes.
fn containers(b: &BundleFile) -> Vec<(usize, Bytes)> {
    (0..b.directory().len())
        .filter_map(|i| {
            let bytes = b.entry_bytes(i).ok()?;
            matches!(
                check_file_type(&bytes, &Limits::default()),
                Ok(FileType::BundleFile)
            )
            .then_some((i, bytes))
        })
        .collect()
}

/// The gaps of one bundle's bytes. A row's registry key is the core's own
/// (`simplify_name`: a Windows-style basename, lower-cased as Python does),
/// so a container dropped by a later row the core keys alike is found.
#[must_use]
pub fn gaps(data: &[u8]) -> BTreeSet<Gap> {
    let mut found = BTreeSet::new();
    let Some(b) = open("top", Bytes::copy_from_slice(data)) else {
        return found;
    };
    let (_ctx, meter) = meter();
    let key = |name: &RawStr| simplify_name(name, &meter).ok();
    let dir = b.directory();
    for (i, bytes) in containers(&b) {
        let k = key(&dir[i].path);
        if k.is_some() && dir[i + 1..].iter().any(|later| key(&later.path) == k) {
            found.insert(Gap::DroppedContainer);
        }
        if let Some(inner) = open("inner", bytes)
            && !containers(&inner).is_empty()
        {
            found.insert(Gap::DepthTwo);
        }
    }
    found
}

/// Every exclusion the catalogue's structure calls for: each input's view
/// (the input and its `deps`, transitively) and each gap any of its
/// bundles has.
#[must_use]
pub fn detect(scenes: &[(&'static str, AtlasScene)]) -> BTreeSet<Exclusion> {
    let mut out = BTreeSet::new();
    for (name, scene) in scenes {
        for input in &scene.inputs {
            let mut view = BTreeSet::new();
            let mut pending = vec![input.clone()];
            while let Some(id) = pending.pop() {
                if view.insert(id.clone()) {
                    let b = scene.bundles.iter().find(|b| b.id == id).unwrap();
                    pending.extend(b.deps.iter().cloned());
                }
            }
            let input: &'static str = Box::leak(input.clone().into_boxed_str());
            for b in scene.bundles.iter().filter(|b| view.contains(&b.id)) {
                for gap in gaps(&b.data) {
                    out.insert(Exclusion {
                        scene: name,
                        input,
                        gap,
                    });
                }
            }
        }
    }
    out
}
