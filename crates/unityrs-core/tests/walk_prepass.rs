//! Review round 6 (claude B, HIGH), the reviewer's input scaled down: K
//! objects of one byte over one class of `N` `int` children, each read
//! failing at the first child, through a SerializedFile's objects on both
//! readers (`byte_size` 1, and 0 for the pure-Python path). A read sized
//! its dict by passes over all `N` children before it read the first, and
//! nothing counted them, so the open's work never stopped the reads (the
//! reviewer's input: 65,536 objects over 2^20 children).
//!
//! No read passes over a class's children before it reads them
//! (`walk::prepass_tests` counts its steps): its reads cost what a read of
//! one value costs, whatever `N`, so K of them are each read to their own
//! failure, as UnityPy reads them, under an allowance that holds a few
//! passes over `N` children. Fix pass 6 still passed over the children of a
//! class that repeats a name, counted as the open's work, so its reads
//! stopped at the allowance: review round 7 (claude B, LOW) found most of
//! K such reads refused with `DecodeWork` where UnityPy gave
//! `out_of_bounds` for every one.
//!
//!     scripts/dev cargo test -p unityrs-core --test walk_prepass

use std::sync::Arc;

use unityrs_core::limits::Meter;
use unityrs_core::walk::{ParseError, ReadDetail, ReadError, STEP_WORK};
use unityrs_core::{Bytes, FileContext, LimitError, LimitKind, Parsed, RawStr, SerializedFile};
use unityrs_testkit::blob::NodeSpec;
use unityrs_testkit::serialized::{ObjectSpec, build, one_object};

const K: usize = 64;
const N: usize = 1 << 16;

/// `k` objects of class 1 over one byte, each `Base` of `n` `int`s named
/// `c0`, `c1` and so on, the last named `c0` again when `dup`.
fn objects(k: usize, n: usize, dup: bool, byte_size: u32) -> Vec<u8> {
    let mut nodes = vec![NodeSpec::new(0, "Base", "Base")];
    for i in 0..n {
        let name = if dup && i + 1 == n {
            "c0".to_owned()
        } else {
            format!("c{i}")
        };
        nodes.push(NodeSpec::new(1, "int", &name));
    }
    let mut spec = one_object(1, nodes, Vec::new());
    let first = spec.objects[0].clone();
    spec.objects = (1..=k)
        .map(|i| ObjectSpec {
            path_id: i64::try_from(i).unwrap(),
            data: if i == k { vec![7] } else { Vec::new() },
            byte_size: Some(byte_size),
            ..first.clone()
        })
        .collect();
    build(&spec)
}

/// Every object of `data` read under `max_decode_work`: the work of the
/// file's load, the work of the reads, and each read's outcome.
fn read_all(data: &[u8], max_decode_work: u64) -> (u64, u64, Vec<Result<Parsed, ParseError>>) {
    let mut c = FileContext::unitypy();
    c.limits.max_decode_work = max_decode_work;
    let c = Arc::new(c);
    let m = Meter::for_input(
        &c.limits,
        c.budget.clone(),
        unityrs_core::budget::Reservation::Input,
    );
    let f = SerializedFile::parse(
        RawStr::from("CAB-p"),
        Bytes::from(data.to_vec()),
        None,
        &m,
        &c,
    )
    .unwrap();
    let load = m.decoded();
    let outcomes: Vec<_> = f.objects().map(|o| o.parse_as_dict(true)).collect();
    (load, m.decoded() - load, outcomes)
}

/// `None` for the read's own failure, where its bytes end (UnityPy's
/// `EOFError` or `struct.error`), or its `DecodeWork` refusal.
fn refusal(o: &Result<Parsed, ParseError>) -> Option<LimitError> {
    match o {
        Err(ParseError::Read(ReadError {
            detail: ReadDetail::Limit(l),
            ..
        })) => {
            assert_eq!(l.kind, LimitKind::DecodeWork, "{o:?}");
            Some(*l)
        }
        Err(ParseError::Read(ReadError {
            detail: ReadDetail::Eof { .. },
            ..
        })) => None,
        o => panic!("not a read failing where its bytes end: {o:?}"),
    }
}

/// A read failing at a class's first child counts as much work for 4
/// children as for 2^16, whether the class's names are all distinct or one
/// repeats, and every one of K such reads fails where its bytes end, as
/// UnityPy's does, under an allowance that holds a few reads' passes over
/// 2^16 children.
#[test]
fn reads_failing_at_a_class_first_child_cost_one_value() {
    let allowance = 8 * u64::try_from(N).unwrap() * STEP_WORK;
    for dup in [false, true] {
        for byte_size in [1, 0] {
            let what = format!("dup {dup}, byte_size {byte_size}");
            let (_, few, _) = read_all(&objects(K, 4, dup, byte_size), u64::MAX);
            let (load, many, outcomes) = read_all(&objects(K, N, dup, byte_size), u64::MAX);
            assert_eq!(many, few, "{what}: 2^16 children against 4");
            let (_, bounded, outcomes_bounded) =
                read_all(&objects(K, N, dup, byte_size), load + allowance);
            assert_eq!(bounded, many, "{what}");
            for o in outcomes.iter().chain(&outcomes_bounded) {
                assert_eq!(refusal(o), None, "{what}: {o:?}");
            }
        }
    }
}
