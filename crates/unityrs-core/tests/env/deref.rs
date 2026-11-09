//! Dereferences outside the oracle's reach.

use std::sync::Arc;

use unityrs_core::budget::Reservation;
use unityrs_core::env::{DerefError, LoadMode, PPtr, Source};
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

/// A pointer to a nested bundle beside the asking CAB, then one to a CAB
/// inside it (scene `deref_sibling_nested`, which `env_fixtures.rs` replays
/// as UnityPy recorded it): the first reaches the nested bundle, which
/// holds no object (`RawTarget`, UnityPy's `AttributeError`); the second
/// finds `CAB-I`'s object 5, under both modes, whether the bundle is a
/// caller's or a `Source::Bytes` dependency, read from the handle
/// `load_dependency` returned or from a second handle over its bytes. Under
/// `Lazy` the first dereference registers the nested bundle's entries, as
/// a `get` of its name does (`PARITY.md` section 19.3, difference (b)), a
/// dependency's as dependency slots.
#[test]
fn a_pointer_to_a_nested_sibling_registers_its_entries() {
    let scene = unityrs_testkit::env::deref_sibling_nested();
    let data = Bytes::from(scene.files[0].1.clone());
    let to_inner = PPtr {
        file_id: 1,
        path_id: 5,
    };
    let to_cab_i = PPtr {
        file_id: 2,
        path_id: 5,
    };
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let caller = env(mode);
        let File::Bundle(own) = caller.load(file_of(&scene, 0)).unwrap() else {
            panic!()
        };
        let dependency = env(mode);
        let File::Bundle(returned) = dependency.load_dependency(file_of(&scene, 0)).unwrap() else {
            panic!()
        };
        let second_env = env(mode);
        let source = Source::Bytes {
            name: RawStr::from("o.bundle"),
            data: data.clone(),
        };
        second_env.load_dependency(source).unwrap();
        let ctx = Arc::new(FileContext::unitypy());
        let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
        let second = BundleFile::open(RawStr::from("o.bundle"), data.clone(), None, &meter, &ctx);
        let second = second.unwrap();
        for (env, bundle) in [
            (&caller, &own),
            (&dependency, &returned),
            (&second_env, &second),
        ] {
            let File::Serialized(cab_o) = bundle.entry(0).unwrap() else {
                panic!()
            };
            assert!(matches!(
                env.deref(&cab_o, to_inner),
                Err(DerefError::RawTarget { .. })
            ));
            let found = env.deref(&cab_o, to_cab_i).unwrap();
            assert_eq!(name_of(&found), "i5", "{mode:?}");
        }
    }
}
