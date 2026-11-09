use std::sync::Arc;

use super::{
    Packed, ResolvedVersion, UnityVersion, VersionError, VersionPolicy, VersionSource,
    resolve_bundle, resolve_serialized,
};
use crate::budget::{Reservation, Unbounded};
use crate::error::{Error, LimitKind};
use crate::limits::{Charge, Limits, Meter};
use crate::raw::RawStr;

#[test]
fn the_chains_take_the_files_string_so_a_version_keeps_it_uncopied() {
    // Round 4: they took `&[u8]`, so the `RawStr` a version (or a
    // `NoMatch`) keeps could only be a copy, uncharged, of a string up to
    // 32,767 bytes, made again by every parse of an entry. Package C
    // implements the bodies; these signatures are what lets them share.
    // Round 7: the chains take the handle's charge, whose meter counts
    // their work and which pays for their heap.
    let _: fn(&RawStr) -> Result<UnityVersion, VersionError> = UnityVersion::parse;
    let _: fn(&RawStr, &VersionPolicy, &mut Charge) -> Result<ResolvedVersion, Error> =
        resolve_bundle;
    let _: fn(
        &RawStr,
        Option<&RawStr>,
        &VersionPolicy,
        &mut Charge,
    ) -> Result<ResolvedVersion, Error> = resolve_serialized;
}

#[test]
fn a_versions_heap_is_its_big_packed_integer_only() {
    let file = bytes::Bytes::from(vec![b'7'; 32_767]);
    let shared = UnityVersion {
        raw: RawStr::from_shared(file.clone()),
        packed: Packed::from_u128(1),
    };
    assert_eq!(shared.heap_bytes(), 0, "the raw string is the file's");
    assert_eq!(shared.raw().as_bytes().as_ptr(), file.as_ptr());
    let big = UnityVersion {
        raw: RawStr::default(),
        packed: Packed::from_limbs(&[1, 2, 3]),
    };
    assert_eq!(big.heap_bytes(), 24);
    assert_eq!(big.packed_u64(), None);
}

#[test]
fn a_tpk_version_round_trips_and_orders_as_python_ints() {
    // 2022.3.0f1 packed as UnityPy packs it.
    let v2022 = (2022u64 << 48) | (3 << 32) | (3 << 8) | 1;
    let v2019 = (2019u64 << 48) | (4 << 32) | (15 << 16) | (3 << 8) | 1;
    let a = UnityVersion::from_packed(v2022);
    let b = UnityVersion::from_packed(v2019);
    assert_eq!(a.packed_u64(), Some(v2022));
    assert!(b < a);
    assert_eq!(a, UnityVersion::from_packed(v2022));
    assert!(a.raw().is_empty());
    let mut v = vec![a.clone(), b.clone()];
    v.sort();
    assert_eq!(v, [b, a]);
}

/// A charge of nothing on a meter of `work` units of work, with no memory
/// bound.
fn charge_on(work: u64) -> Charge {
    let limits = Limits {
        max_decode_work: work,
        ..Limits::default()
    };
    Charge::zero(&Meter::for_input(
        &limits,
        Arc::new(Unbounded),
        Reservation::Input,
    ))
}

/// The work `s`'s parse counts, and its result.
fn counted(s: &str) -> (u64, Result<Result<UnityVersion, VersionError>, Error>) {
    let mut charge = charge_on(u64::MAX);
    let got = UnityVersion::parse_charged(&RawStr::from(s), &mut charge);
    (charge.meter().decoded(), got)
}

/// "0" * 4,299 + "1.2.3f4": a 4,300-digit major whose value is 1.
fn zeros_major() -> String {
    format!("{}1.2.3f4", "0".repeat(4299))
}

/// "9" * 4,300 + ".0.0f1": a 4,300-digit major of 4,300 significant digits.
fn nines_major() -> String {
    format!("{}.0.0f1", "9".repeat(4300))
}

/// Round 6 (group e's sweep, queued for package C): every 19 digits of a
/// group stepped through all 226 limbs of a fixed array, 51,302 limb steps
/// for a 4,300-digit group whatever its value, and so did each merge of a
/// group (`2022.3.0f1`: ten steps of 226, 2,260 limbs, which a mutant
/// restoring the array counts as 10 + 2,260 x 8 = 18,090), and nothing
/// counted them, on every parse of an entry. A step now goes over
/// the limbs the value holds, and the chains count each one as the input's
/// work before it runs: the bytes scanned since the last count, and 8 a
/// limb (`LIMB_WORK`).
///
/// The expected counts are derived by hand (`2022.3.0f1`: its 10 bytes;
/// major, minor and number one limb to multiply and one to merge, the
/// build's zero one to multiply and none to merge, the type one of each:
/// 9 limbs) and, for the long strings, by a model of the same rule in
/// Python ints (each chunk's step is `(v.bit_length() + 63) // 64 + 1`
/// limbs of the value `v` before it, each merge the group's limbs).
#[test]
fn a_versions_packing_counts_the_limbs_it_holds_as_the_inputs_work() {
    let (work, got) = counted("2022.3.0f1");
    assert_eq!(work, 10 + 9 * 8);
    let v = got.unwrap().unwrap();
    assert_eq!(
        v.packed_u64(),
        Some((2022 << 48) | (3 << 32) | (3 << 8) | 1)
    );

    // Linear in the string while the value is small: 227 chunks of a
    // zero-valued group step through one limb each (the fixed array: 226).
    let (work, got) = counted(&zeros_major());
    assert_eq!(work, 6_194);
    assert_eq!(
        got.unwrap().unwrap().packed_u64(),
        Some((1 << 48) | (2 << 32) | (3 << 16) | (3 << 8) | 4)
    );
    assert!(
        work < 2 * 4_306,
        "{work}: a string of 4,306 bytes whose value is small"
    );

    // Significant digits: the limbs the value holds, 25,633 for the group
    // (the fixed array stepped through 51,302).
    let (work, got) = counted(&nines_major());
    assert_eq!(work, 211_210);
    assert!(
        work < 227 * 226 * 8,
        "{work}: no fewer limbs than the fixed array's"
    );
    assert!(work < 50 * 4_306, "{work}: under 50 a byte");
    assert_eq!(got.unwrap().unwrap().heap_bytes(), 224 * 8);
}

/// A refused step stops the parse: what it scanned counts, its limbs do
/// not, and nothing after it runs.
///
/// The counts are derived by hand, and by the Python-int model of
/// `a_versions_packing_counts_the_limbs_it_holds_as_the_inputs_work`.
/// The major's group is 4,300 digits whose value stays 0 until the last
/// one, so each of its 19-digit chunks counts its 19 bytes and one limb, 27
/// a step: 37 steps count 999, and the 38th asks for 999 + 27 = 1,026, past
/// an allowance of 1,000. Refused, its 19 scanned bytes count all the same:
/// 1,018, past the allowance, so the meter's work is used up. The parse's
/// last step (the type's merge, one limb) scans nothing: refused at an
/// allowance one short of the whole parse, it leaves 6,194 - 8 counted,
/// within the allowance. Round 8: the test took any count from 973 to
/// 1,019, which a parse that dropped the refused step's bytes (999, still
/// within the allowance) also gave.
#[test]
fn a_refused_step_stops_the_packing() {
    let s = RawStr::from(zeros_major().as_str());
    // Exactly its work fits; one less is refused at its last step.
    let mut charge = charge_on(6_194);
    assert!(matches!(
        UnityVersion::parse_charged(&s, &mut charge),
        Ok(Ok(_))
    ));
    let mut charge = charge_on(6_193);
    let got = UnityVersion::parse_charged(&s, &mut charge);
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::DecodeWork),
        "{got:?}"
    );
    assert_eq!(
        charge.meter().decoded(),
        6_194 - 8,
        "a refused step that scanned nothing counts nothing"
    );
    assert!(charge.meter().work_used_up().is_none());
    // A small allowance stops it long before the end of the string: the
    // 37 steps that fit, then the refused step's 19 scanned bytes.
    let mut charge = charge_on(1_000);
    let got = UnityVersion::parse_charged(&s, &mut charge);
    assert!(
        matches!(&got, Err(Error::Limit(l)) if l.kind == LimitKind::DecodeWork),
        "{got:?}"
    );
    let work = charge.meter().decoded();
    assert_eq!(work, 37 * (19 + 8) + 19, "the refused step's bytes count");
    assert!(
        charge.meter().work_used_up().is_some(),
        "{work}: the refused step's scan takes the work past the allowance"
    );
    // Uncounted, a caller's own string parses whatever its size.
    assert!(UnityVersion::parse(&s).is_ok());
}

/// "1" + "0" * 40 + ".0.0f1": a major of 10^40, which packs past 2^128
/// into three limbs (10^40 << 48 < 2^181).
fn big_major() -> RawStr {
    RawStr::from(format!("1{}.0.0f1", "0".repeat(40)).as_str())
}

/// A big packed integer's limbs (24 bytes, a 32-byte glibc chunk) are
/// charged to the handle's charge before they are allocated; a big
/// version the bundle chain drops (its 16-bit major 0) gives its charge
/// back, and a forced one is charged before its copy. (The callers
/// charged `heap_bytes()` after the chain had allocated it, without the
/// chunk's rounding.)
#[test]
fn the_chains_charge_a_big_versions_limbs_to_the_handle() {
    let policy = VersionPolicy::default();
    let mut charge = charge_on(u64::MAX);
    let got = resolve_serialized(&big_major(), None, &policy, &mut charge).unwrap();
    assert_eq!(got.version.heap_bytes(), 24);
    assert_eq!(charge.bytes(), 32);
    assert_eq!(charge.meter().used(), 32);

    // 2^136 << 48 = 2^184: three limbs, its bits 48 to 63 zero.
    let dropped = RawStr::from("87112285931760246646623899502532662132736.0.0f1");
    let policy = VersionPolicy {
        fallback: Some(RawStr::from("2020.1.0f1")),
        ..VersionPolicy::default()
    };
    let mut charge = charge_on(u64::MAX);
    let got = resolve_bundle(&dropped, &policy, &mut charge).unwrap();
    assert_eq!(got.source, VersionSource::Fallback);
    assert_eq!(charge.bytes(), 0, "the dropped version's limbs");
    assert!(charge.meter().decoded() > 0);

    let forced = UnityVersion::parse(&big_major()).unwrap();
    let policy = VersionPolicy {
        force_bundle: Some(forced),
        ..VersionPolicy::default()
    };
    let mut charge = charge_on(u64::MAX);
    let got = resolve_bundle(&RawStr::from("2020.1.0f1"), &policy, &mut charge).unwrap();
    assert_eq!(got.source, VersionSource::Forced);
    assert_eq!(charge.bytes(), 32);
}
