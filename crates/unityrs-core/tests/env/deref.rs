//! Dereferences outside the oracle's reach.

use std::sync::Arc;

use unityrs_core::budget::Reservation;
use unityrs_core::env::{DerefError, LoadMode, PPtr};
use unityrs_core::files::{BundleFile, File};
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::env::{Obj, serialized};

use crate::{env, file_of, name_of, serialized_cab, src};

/// A `from` the Environment never loaded (a second handle on the same
/// bytes) finds its siblings through its own bundle and every other CAB
/// through the Environment; once its bundle is dropped, a pointer that
/// needs the container is `ContainerDropped`, and one into its own file
/// still resolves.
#[test]
fn a_second_handle_derefs_through_its_bundle_and_the_environment() {
    let scene = unityrs_testkit::env::cross_bundle_pointer();
    let env = env(LoadMode::Eager);
    env.load(file_of(&scene, 1)).unwrap();
    let ctx = Arc::new(FileContext::unitypy());
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let a = BundleFile::open(
        RawStr::from("a.bundle"),
        Bytes::from(scene.files[0].1.clone()),
        None,
        &meter,
        &ctx,
    )
    .unwrap();
    let File::Serialized(cab_a) = a.entry(0).unwrap() else {
        panic!()
    };
    let to_b = PPtr {
        file_id: 1,
        path_id: 7,
    };
    assert_eq!(name_of(&env.deref(&cab_a, to_b).unwrap()), "b7");
    drop(a);
    assert!(matches!(
        env.deref(&cab_a, to_b),
        Err(DerefError::ContainerDropped { .. })
    ));
    let own = PPtr {
        file_id: 0,
        path_id: 2,
    };
    assert_eq!(name_of(&env.deref(&cab_a, own).unwrap()), "tex");
}

/// Without B, A's pointer finds nothing (`find_file` has no resolver).
#[test]
fn a_loose_pointer_without_its_target_is_not_found() {
    let scene = unityrs_testkit::env::loose_external_pointer();
    let env = env(LoadMode::Eager);
    env.load(file_of(&scene, 0)).unwrap();
    let sf = serialized_cab(&env, "cab-a");
    assert!(matches!(
        env.deref(
            &sf,
            PPtr {
                file_id: 1,
                path_id: 5
            }
        ),
        Err(DerefError::NotFound { .. })
    ));
}

/// `external_index`'s edges through `deref`: `i32::MIN` and `-len - 1`
/// are below the start, `-len` is the first external.
#[test]
fn negative_file_ids_index_from_the_end() {
    let data = serialized(
        &["archive:/X/CAB-T1", "archive:/X/CAB-T2"],
        &[Obj::named(1, "o", &[])],
    );
    let env = env(LoadMode::Eager);
    env.load(src("CAB-O", &data)).unwrap();
    env.load(src("CAB-T1", &serialized(&[], &[Obj::named(4, "t1", &[])])))
        .unwrap();
    let sf = serialized_cab(&env, "cab-o");
    let deref = |file_id| {
        env.deref(
            &sf,
            PPtr {
                file_id,
                path_id: 4,
            },
        )
    };
    assert!(matches!(
        deref(i32::MIN),
        Err(DerefError::FileIdBelowStart { .. })
    ));
    assert!(matches!(
        deref(-2),
        Err(DerefError::FileIdBelowStart { .. })
    ));
    assert_eq!(name_of(&deref(-1).unwrap()), "t1");
    assert!(matches!(deref(3), Err(DerefError::FileIdPastEnd { .. })));
}
