//! Resources: located without parsing, and loaded through the resolver.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use unityrs_core::budget::Reservation;
use unityrs_core::env::{
    CabResolver, DerefError, EnvOptions, Environment, Fetch, LoadMode, PPtr, Source,
};
use unityrs_core::error::Error;
use unityrs_core::files::{BundleFile, File};
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::env::{Obj, bundle, res_bytes, serialized};

use crate::{TempFile, batch, env, file_of, name_of, object, serialized_cab, src, stream};

fn res_bundle() -> Vec<u8> {
    bundle(&[
        (
            "CAB-R",
            serialized(
                &[],
                &[Obj::streamed(2, "tex", "archive:/CAB-R/CAB-R.resS", 3, 5)],
            ),
        ),
        ("CAB-R.resS", res_bytes(32, 0)),
    ])
}

/// A resource read on a second handle over a `Source::Bytes` dependency's
/// own bytes reads through that handle: the shared slot is never parsed,
/// and the dependency's meter (and so its pool) is charged nothing.
#[test]
fn a_resource_on_a_second_handle_over_a_dependencys_bytes_charges_the_slot_nothing() {
    let data = Bytes::from(res_bundle());
    let env = batch(Arc::new(FileContext::unitypy()), u64::MAX);
    let dep = env
        .load_dependency(Source::Bytes {
            name: RawStr::from("r.bundle"),
            data: data.clone(),
        })
        .unwrap();
    let dep_used = dep.meter().used();
    let parsed = env.stats().entries_parsed;

    let ctx = Arc::new(FileContext::unitypy());
    let meter = Meter::for_input(&ctx.limits, ctx.budget.clone(), Reservation::Input);
    let second = BundleFile::open(RawStr::from("r.bundle"), data, None, &meter, &ctx).unwrap();
    let File::Serialized(cab) = second.entry(0).unwrap() else {
        panic!()
    };
    let (path, offset, size) = stream(&object(&cab, 2));
    let got = env.resource(&cab, &path, offset, size).unwrap();
    assert_eq!(got.as_ref(), &res_bytes(32, 0)[3..8]);
    assert_eq!(dep.meter().used(), dep_used, "the slot's meter");
    assert_eq!(
        env.stats().entries_parsed,
        parsed,
        "the slot was not parsed"
    );
}

#[derive(Debug)]
struct BytesFetch(Bytes, AtomicUsize);

impl Fetch for BytesFetch {
    fn fetch(&self) -> Result<Bytes, Error> {
        self.1.fetch_add(1, Ordering::Relaxed);
        Ok(self.0.clone())
    }
}

/// Through the handle `load_dependency` returned, for every `Source` kind,
/// a resource read never parses the shared slot.
#[test]
fn a_resource_through_the_dependency_handle_parses_no_slot_for_every_source() {
    let data = Bytes::from(res_bundle());
    let file = TempFile::new("res", &data);
    let sources = [
        Source::Bytes {
            name: RawStr::from("r.bundle"),
            data: data.clone(),
        },
        Source::Path(file.0.clone()),
        Source::Fetch {
            name: RawStr::from("r.bundle"),
            fetch: Arc::new(BytesFetch(data.clone(), AtomicUsize::new(0))),
        },
    ];
    for source in sources {
        let env = batch(Arc::new(FileContext::unitypy()), u64::MAX);
        let File::Bundle(b) = env.load_dependency(source.clone()).unwrap() else {
            panic!()
        };
        let File::Serialized(cab) = b.entry(0).unwrap() else {
            panic!()
        };
        let (path, offset, size) = stream(&object(&cab, 2));
        let parsed = env.stats().entries_parsed;
        let got = env.resource(&cab, &path, offset, size).unwrap();
        assert_eq!(got.as_ref(), &res_bytes(32, 0)[3..8], "{source:?}");
        assert_eq!(env.stats().entries_parsed, parsed, "{source:?}");
        assert!(env.files().next().is_none());
    }
}

/// An evicted `Source::Path` dependency's resource resolves through its
/// slot, which opens the path again: the same bytes.
#[test]
fn an_evicted_path_dependency_resolves_through_its_slot() {
    let file = TempFile::new("evicted", &res_bundle());
    let other = TempFile::new("other", &bundle(&[("CAB-Z", serialized(&[], &[]))]));
    let env = batch(Arc::new(FileContext::unitypy()), 1);
    let File::Bundle(b) = env.load_dependency(Source::Path(file.0.clone())).unwrap() else {
        panic!()
    };
    let File::Serialized(cab) = b.entry(0).unwrap() else {
        panic!()
    };
    let (path, offset, size) = stream(&object(&cab, 2));
    let cab = Arc::new(cab);
    drop(b);
    env.load_dependency(Source::Path(other.0.clone())).unwrap();
    assert_eq!(env.stats().dependencies_evicted, 1);
    let got = env.resource(&cab, &path, offset, size).unwrap();
    assert_eq!(got.as_ref(), &res_bytes(32, 0)[3..8]);
}

/// A resource no candidate names is loaded through the resolver (UnityPy's
/// `load_dependencies`) and read; with none, `NotFound` names the basename.
#[test]
fn a_missing_resource_is_loaded_through_the_resolver_then_read() {
    #[derive(Debug)]
    struct One(Bytes);
    impl CabResolver for One {
        fn resolve(&self, name: &[u8]) -> Option<Source> {
            (name == b"CAB-W.resS").then(|| Source::Bytes {
                name: RawStr::from("CAB-W.resS"),
                data: self.0.clone(),
            })
        }
    }
    let data = serialized(
        &[],
        &[Obj::streamed(2, "tex", "archive:/CAB-W/CAB-W.resS", 1, 2)],
    );
    for keep in [true, false] {
        let mut o = EnvOptions::unitypy();
        o.keep_resolver_loads = keep;
        o.resolver = Some(Arc::new(One(Bytes::from(res_bytes(8, 4)))));
        let env = Environment::new(o);
        env.load(src("CAB-O", &data)).unwrap();
        let sf = serialized_cab(&env, "cab-o");
        let (path, offset, size) = stream(&object(&sf, 2));
        let got = env.resource(&sf, &path, offset, size).unwrap();
        assert_eq!(got.as_ref(), &res_bytes(8, 4)[1..3]);
        assert_eq!(env.files().count(), if keep { 2 } else { 1 });
    }
    let env = env(LoadMode::Eager);
    env.load(src("CAB-O", &data)).unwrap();
    let sf = serialized_cab(&env, "cab-o");
    let (path, offset, size) = stream(&object(&sf, 2));
    assert!(matches!(
        env.resource(&sf, &path, offset, size),
        Err(Error::NotFound(unityrs_core::NotFound::Resource(n))) if n.as_bytes() == b"CAB-W.resS"
    ));
}

/// Under `EnvOptions::batch()` (no `keep_resolver_loads`) every file the
/// resolver loads is an LRU dependency: none is in `files`, `objects` or
/// the container, and the pointer and the resource still resolve.
#[test]
fn batch_options_keep_no_resolver_load_in_files_objects_or_the_container() {
    #[derive(Debug)]
    struct SceneResolver(Vec<(Vec<u8>, Vec<u8>)>);
    impl CabResolver for SceneResolver {
        fn resolve(&self, name: &[u8]) -> Option<Source> {
            let (n, d) = self.0.iter().find(|(n, _)| n == name)?;
            Some(Source::Bytes {
                name: RawStr::new(n.clone()),
                data: Bytes::from(d.clone()),
            })
        }
    }
    let scene = unityrs_testkit::env::resolver_loads();
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let mut o = EnvOptions::batch(Arc::new(FileContext::unitypy()));
        o.load = mode;
        o.resolver = Some(Arc::new(SceneResolver(scene.resolver.clone())));
        let env = Environment::new(o);
        env.load(file_of(&scene, 0)).unwrap();
        let sf = serialized_cab(&env, "cab-h");
        let loose = PPtr {
            file_id: 2,
            path_id: 5,
        };
        assert_eq!(name_of(&env.deref(&sf, loose).unwrap()), "loose5");
        assert_eq!(env.container().unwrap().count(), 1, "{mode:?}");
        // `cab-dep` is a bundle: loaded, it is the pointer's container.
        let to_dep = PPtr {
            file_id: 1,
            path_id: 3,
        };
        assert!(matches!(
            env.deref(&sf, to_dep),
            Err(DerefError::RawTarget { .. })
        ));
        let (path, offset, size) = stream(&object(&sf, 2));
        let got = env.resource(&sf, &path, offset, size).unwrap();
        assert_eq!(got.as_ref(), &res_bytes(24, 9)[2..8]);
        assert_eq!(env.files().count(), 1, "{mode:?}");
        let objects: Vec<_> = env.objects().map(Result::unwrap).collect();
        assert_eq!(objects.len(), 3, "{mode:?}");
        assert!(objects.iter().all(|o| Arc::ptr_eq(o.file(), &sf)));
        assert!(env.stats().dependencies_loaded >= 2, "{mode:?}");
        // `cab-dep`'s own `CAB-H` took the key: no caller CAB is left with rows.
        assert_eq!(env.container().unwrap().count(), 0, "{mode:?}");
    }
}
