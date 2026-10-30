//! What a reach of a nested container under `LoadMode::Lazy` reaches, what
//! it costs and whom it charges (plan section 3.16, R2, R4 and R5): a
//! dropped container another handle cannot stand in for is reached, a
//! reach its input's meter refuses is counted as that open's work until the
//! allowance keeps the refusal, and a reach through a job's handle charges
//! a shared dependency's own meter.

use std::sync::Arc;

use unityrs_core::env::{EnvOptions, Environment, LoadMode, Source};
use unityrs_core::files::File;
use unityrs_core::limits::Meter;
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_testkit::env::{bundle, res_bytes};

use super::{TempFile, key, object, stream, unlimited};

/// `CAB-I` of a bundle laid out as the scene `resource_in_nested_cab`:
/// the first entry of the nested bundle `INNER`.
fn inner_cab(outer: &File) -> Arc<unityrs_core::files::SerializedFile> {
    let File::Bundle(outer) = outer else {
        panic!("o.bundle is a bundle");
    };
    let File::Bundle(inner) = outer.entry(0).unwrap() else {
        panic!("INNER is a bundle");
    };
    let File::Serialized(cab) = inner.entry(0).unwrap() else {
        panic!("CAB-I is a SerializedFile");
    };
    cab
}

/// Two dependency loads of one path (the same input opened twice, whose
/// handles share no bytes): the second's slot for `INNER` cannot be told
/// to stand in for the first's from the first's handle, so the first's is
/// reached when the second drops it, and a reader inside the first's
/// `INNER` reads its sibling `.resS` as UnityPy does (the scene
/// `nested_reloaded`, recorded in `tests/fixtures/d/env.json`: `28292a2b`;
/// review round 5: the drop was taken for a stand-in, nothing reached it,
/// and the read missed every candidate).
#[test]
fn a_reader_inside_the_first_of_two_loads_of_one_path_reads_its_sibling_resource() {
    let scene = unityrs_testkit::env::resource_in_nested_cab();
    let file = TempFile::new("reload", &scene.files[0].1);
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let mut o = EnvOptions::batch(Arc::new(FileContext::unitypy()));
        o.load = mode;
        let env = Environment::new(o);
        let first = env.load_dependency(Source::Path(file.0.clone())).unwrap();
        drop(env.load_dependency(Source::Path(file.0.clone())).unwrap());
        let cab = inner_cab(&first);
        let (path, offset, size) = stream(&object(&cab, 1));
        let bytes = env.resource(&cab, &path, offset, size).unwrap();
        assert_eq!(bytes.as_ref(), [0x28, 0x29, 0x2a, 0x2b], "{mode:?}");
    }
}

/// `o.bundle`'s nested bundle `INNER` of `n` raw entries, and `p.bundle`,
/// whose raw entry `INNER` drops it (the scene `nested_raw_replaced`'s
/// shape, larger).
fn raw_replaced(n: usize) -> (Bytes, Bytes) {
    let names: Vec<String> = (0..n).map(|i| format!("R{i}.resS")).collect();
    let entries: Vec<(&str, Vec<u8>)> = names
        .iter()
        .map(|name| (name.as_str(), res_bytes(4, 10)))
        .collect();
    let o = bundle(&[("INNER", bundle(&entries))]);
    let p = bundle(&[("INNER", res_bytes(4, 50))]);
    (Bytes::from(o), Bytes::from(p))
}

fn lazy_batch(ctx: FileContext) -> Environment {
    let mut o = EnvOptions::batch(Arc::new(ctx));
    o.load = LoadMode::Lazy;
    Environment::new(o)
}

fn bytes(name: &str, data: &Bytes) -> Source {
    Source::Bytes {
        name: RawStr::from(name),
        data: data.clone(),
    }
}

/// A reach the input's own meter refuses is counted as that open's work
/// (`crate::once`'s rule), so once the open's allowance is used up the
/// refusal is `DecodeWork` and kept, and no later lookup does the reach's
/// work again (review round 5: every lookup by name redid it, uncounted,
/// with no bound per input: the `ZeroSizeElements` class). The limits are
/// measured on an unlimited run: room for the load and the container's
/// parse but not for the reach's registrations, and work for an unrefused
/// reach alone.
#[test]
fn a_reach_the_meter_refuses_is_counted_until_it_is_kept() {
    let (o, p) = raw_replaced(200);
    let measure = lazy_batch(FileContext::unitypy());
    let outer = measure.load(bytes("o.bundle", &o)).unwrap();
    let meter = outer.meter().clone();
    let File::Bundle(b) = &outer else {
        panic!("o.bundle is a bundle")
    };
    drop(b.entry(0).unwrap());
    let parsed = meter.used();
    drop(measure.load(bytes("p.bundle", &p)).unwrap());
    let (reached, work) = (meter.used(), meter.decoded());
    assert!(
        measure
            .get_cab(&key("R7.resS"), &unlimited())
            .unwrap()
            .is_some()
    );
    assert!(
        reached > parsed + 4096,
        "the reach registers: {parsed} -> {reached}"
    );

    let mut ctx = FileContext::unitypy();
    ctx.limits.max_input_memory = parsed + (reached - parsed) / 2;
    ctx.limits.max_decode_work = work;
    let env = lazy_batch(ctx);
    let outer = env.load(bytes("o.bundle", &o)).unwrap();
    let meter: Arc<Meter> = outer.meter().clone();
    // The drop's reach is refused by the meter, counted, and kept.
    drop(env.load(bytes("p.bundle", &p)).unwrap());
    assert!(
        meter.work_used_up().is_some(),
        "the refused reach's grants were not counted: {} of {work}",
        meter.decoded()
    );
    let (granted, done) = (meter.granted(), meter.decoded());
    // A lookup, the key listing and the container pass do the work no
    // more. The first pass meets the kept refusal reading the container's
    // entries, and is kept as UnityPy keeps a failed pass (the index built).
    let pass = env.container().map(|_| ()).map_err(|e| e.to_string());
    assert!(pass.is_err_and(|e| e.starts_with("limit DecodeWork")));
    let pass = env.container().map(|_| ()).map_err(|e| e.to_string());
    for i in 0..50 {
        drop(
            env.get_cab(&key(&format!("NONE{i}")), &unlimited())
                .unwrap(),
        );
        let _ = env.registry().keys().count();
        let again = env.container().map(|_| ()).map_err(|e| e.to_string());
        assert_eq!(again, pass);
    }
    assert_eq!(meter.granted(), granted, "a lookup redid the refused reach");
    assert_eq!(meter.decoded(), done, "a lookup counted more work");
}

/// A batch job's resource read that reaches a shared dependency's nested
/// container through the job's own handle charges the registrations to
/// the dependency's meter (the pool's), never the job's: once the job has
/// let go of its handles, its meter holds nothing (review round 5: the
/// shared registry's entries kept 3,388 bytes on the ended job's meter for
/// the Environment's life).
#[test]
fn a_jobs_read_leaves_no_charge_on_its_meter() {
    let scene = unityrs_testkit::env::resource_in_nested_cab();
    let data = Bytes::from(scene.files[0].1.clone());
    for mode in [LoadMode::Eager, LoadMode::Lazy] {
        let mut o = EnvOptions::batch(Arc::new(FileContext::unitypy()));
        o.load = mode;
        let shared = Environment::new(o.clone());
        drop(shared.load_dependency(bytes("o.bundle", &data)).unwrap());
        let job = Environment::new(o);
        let outer = job.load(bytes("o.bundle", &data)).unwrap();
        let job_meter = outer.meter().clone();
        let cab = inner_cab(&outer);
        let (path, offset, size) = stream(&object(&cab, 1));
        let read = shared.resource(&cab, &path, offset, size).unwrap();
        assert_eq!(read.as_ref(), [0x28, 0x29, 0x2a, 0x2b], "{mode:?}");
        drop((read, path, cab, outer, job));
        assert_eq!(job_meter.used(), 0, "{mode:?}: the ended job's meter");
        assert!(
            shared
                .get_cab(&key("CAB-I.resS"), &unlimited())
                .unwrap()
                .is_some(),
            "{mode:?}"
        );
    }
}
