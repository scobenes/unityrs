//! The `meshdecode` target's check: a Mesh's or a Sprite's value
//! (`unityrs_testkit::fuzz::MeshCase`), read by UnityPyBoost's reader
//! (`walk::boost::read`), through `unityrs-meshdecode`'s
//! `MeshHandler::process` and `get_triangles`, as an extractor runs them
//! (`docs/plans/extract.md` E16, E59): under the case's limits ([`crate::limits`]'s
//! memory and work bounds), and, when the case's meter is not 0, again under
//! a meter of `meter² × 64` bytes with the same work bound.
//!
//! Besides a panic (libFuzzer's hook aborts at it), a finding is: a second
//! `get_triangles`, or a second `process` (run last, once the first's
//! outputs are dropped), that answers otherwise than the first, but for a
//! refusal of the work it adds, as each call reads its input and changes
//! nothing; a run under the smaller meter whose answer is not the first's,
//! but for a refusal of a charge past its limit; and a meter not back at 0
//! once the outputs drop.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use unityrs_core::budget::{Reservation, Unbounded};
use unityrs_core::limits::{ChargeError, Meter};
use unityrs_core::walk::{ReadCtx, boost};
use unityrs_core::{Bytes, Parsed, RawStr};
use unityrs_meshdecode::{
    MeshError, MeshHandler, Processed, Resources, Source, Triangles, Values, Version, VertexArray,
};
use unityrs_testkit::fuzz::MeshCase;

use crate::{Case, WORK};

/// What a resource read returns: the case's bytes, whatever it asks.
struct Resource(Bytes);

impl Resources for Resource {
    fn resource(
        &self,
        _path: &RawStr,
        _offset: u64,
        _size: u64,
    ) -> Result<Bytes, unityrs_core::Error> {
        Ok(self.0.clone())
    }
}

/// One call's answer, small enough to compare.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// A charge past the meter's limit, or work past its bound.
    Refused,
    /// Any other error, by its `Debug` form.
    Error(String),
    /// The outputs, by a hash of every value.
    Done(u64),
}

impl Outcome {
    fn of(e: &MeshError) -> Self {
        match e {
            MeshError::Charge(ChargeError::Limit(_)) => Self::Refused,
            e => Self::Error(format!("{e:?}")),
        }
    }
}

fn hash_array(h: &mut DefaultHasher, a: Option<&VertexArray>) {
    let Some(a) = a else {
        0u8.hash(h);
        return;
    };
    (1u8, a.dimension(), a.len()).hash(h);
    match a.values() {
        Values::Float(v) => v.iter().for_each(|x| x.to_bits().hash(h)),
        Values::Int(v) => v.hash(h),
        // A form a later phase adds (`Values` is non-exhaustive).
        other => format!("{other:?}").hash(h),
    }
}

fn hash_processed(p: &Processed<'_>) -> u64 {
    let mut h = DefaultHasher::new();
    (p.vertex_count(), p.use_16bit_indices()).hash(&mut h);
    p.index_buffer().map(|b| b.as_slice()).hash(&mut h);
    for a in [
        p.vertices(),
        p.normals(),
        p.tangents(),
        p.colors(),
        p.bone_weights(),
        p.bone_indices(),
    ] {
        hash_array(&mut h, a);
    }
    for i in 0..8 {
        hash_array(&mut h, p.uv(i));
    }
    h.finish()
}

fn hash_triangles(t: &Triangles) -> u64 {
    let mut h = DefaultHasher::new();
    for s in t.submeshes() {
        (s.triangles(), s.cut(), s.empty(), s.index_count()).hash(&mut h);
    }
    h.finish()
}

/// What `process` and then `get_triangles` (twice) answer: `process`'s
/// outcome, and the first `get_triangles`' when `process` succeeded.
pub(crate) type Answer = (Outcome, Option<Outcome>);

/// What `process`, then `get_triangles` (twice), then `process` again with
/// the first's outputs dropped, answer under `meter`: the second `process`
/// comes last, so whatever it does leaves the work and memory the others
/// see as in a run without it.
fn run(
    case: &MeshCase,
    parsed: &Parsed,
    resources: Option<&dyn Resources>,
    meter: &Arc<Meter>,
) -> Answer {
    let [major, minor, build] = case.version;
    let version = Version::new(major, minor, build, case.version_type, case.version_number);
    let source = if case.has(MeshCase::SPRITE) {
        Source::sprite(parsed)
    } else {
        Source::mesh(parsed)
    };
    let handler = MeshHandler::new(source, version).little_endian(!case.has(MeshCase::BIG_ENDIAN));
    let p = match handler.process(resources, meter) {
        Ok(p) => p,
        Err(e) => return (Outcome::of(&e), None),
    };
    let done = Outcome::Done(hash_processed(&p));
    let triangles = || match p.get_triangles(meter) {
        Ok(t) => Outcome::Done(hash_triangles(&t)),
        Err(e) => Outcome::of(&e),
    };
    let first = triangles();
    let second = triangles();
    assert!(
        second == first || second == Outcome::Refused,
        "a second get_triangles answered {second:?}, the first {first:?}"
    );
    drop(p);
    let again = match handler.process(resources, meter) {
        Ok(q) => Outcome::Done(hash_processed(&q)),
        Err(e) => Outcome::of(&e),
    };
    assert!(
        again == done || again == Outcome::Refused,
        "a second process answered {again:?}, the first {done:?}"
    );
    (done, Some(first))
}

/// Runs one input, panicking at a finding.
pub fn check(input: &[u8]) {
    let _ = answers(input);
}

/// Runs one input, panicking at a finding: the answer under the case's
/// limits and, when the case's meter is not 0, the answer under the smaller
/// meter; `None` when the value is not read.
pub(crate) fn answers(input: &[u8]) -> Option<(Answer, Option<Answer>)> {
    let case = MeshCase::decode(input);
    let value = Case::new(&case.value.encode())?;
    crate::with_stack(|| {
        let read = ReadCtx {
            endian: value.endian(),
            ref_types: value.ref_types(),
            options: &value.options,
            meter: &value.meter,
        };
        let parsed = boost::read(&value.data, &value.tree, &read).ok()?;
        let resource = Resource(Bytes::from(case.resource.clone()));
        let resources: Option<&dyn Resources> = if case.has(MeshCase::RESOURCES) {
            Some(&resource)
        } else {
            None
        };
        let meter = value.new_meter();
        let full = run(&case, &parsed, resources, &meter);
        assert_eq!(meter.used(), 0, "the meter is not back at 0");
        if case.meter == 0 {
            return Some((full, None));
        }
        let limit = u64::from(case.meter).pow(2) * 64;
        let small = Meter::with_decode_work(limit, WORK, Arc::new(Unbounded), Reservation::Input);
        let under = run(&case, &parsed, resources, &small);
        assert_eq!(small.used(), 0, "the smaller meter is not back at 0");
        let same = under.0 == full.0 && (under.1 == full.1 || under.1 == Some(Outcome::Refused));
        assert!(
            under.0 == Outcome::Refused || same,
            "under {limit} bytes: {under:?}; under the case's limits: {full:?}"
        );
        Some((full, Some(under)))
    })
}
