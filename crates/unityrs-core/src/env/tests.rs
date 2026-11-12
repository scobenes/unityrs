use std::io::Write;
use std::sync::Arc;

use super::{Cab, EnvOptions, Environment, Registry, read_path};
use crate::budget::OWNER_COST;
use crate::budget::{Reservation, Unbounded};
use crate::error::{Error, LimitKind};
use crate::limits::{Limits, Meter, heap_cost};

#[test]
fn unitypy_options_keep_resolver_loads_in_files_and_batch_options_do_not() {
    // Round 5 (probed on UnityPy 1.25.2): UnityPy keeps a resource
    // fallback's loads and `find_file`'s in `env.files` for good; the
    // batch engine's options keep them in the evictable LRU only.
    assert!(EnvOptions::unitypy().keep_resolver_loads);
    assert!(EnvOptions::default().keep_resolver_loads);
    let batch = EnvOptions::batch(Arc::new(crate::options::FileContext::unitypy()));
    assert!(!batch.keep_resolver_loads);
}

/// A temporary file in a folder of its own, removed with it when dropped,
/// so a failing test leaves nothing behind (`unityrs_testkit::scratch`).
struct TempFile(
    std::path::PathBuf,
    #[allow(dead_code, reason = "held for its drop, which removes the folder")]
    unityrs_testkit::scratch::Scratch,
);

fn temp_file(name: &str, len: usize) -> TempFile {
    let dir = unityrs_testkit::scratch::Scratch::new("core-read-path");
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(&vec![7_u8; len]).unwrap();
    TempFile(path, dir)
}

#[test]
fn lookups_take_the_names_string_and_a_meter_for_the_key_they_build() {
    // Round 4: `get_cab`, `find_file`, `Registry::get` and `resource`
    // took `&[u8]` and no meter, so the lowered key of a name an input
    // chose (and every resource candidate) could only be built
    // uncharged. Package D implements the bodies.
    use crate::files::{File, SerializedFile};
    use crate::raw::RawStr;
    type Lookup<S> = fn(&S, &RawStr, &Arc<Meter>) -> Result<Option<Cab>, Error>;
    // Round 11: `find_file` returns the file UnityPy's does, of any kind,
    // and takes its `is_dependency`.
    type Find = fn(&Environment, &RawStr, bool, &Arc<Meter>) -> Result<Option<File>, Error>;
    type Resource =
        fn(&Environment, &Arc<SerializedFile>, &RawStr, u64, u64) -> Result<bytes::Bytes, Error>;
    let _: Lookup<Environment> = Environment::get_cab;
    let _: Find = Environment::find_file;
    let _: Lookup<Registry> = Registry::get;
    let _: Resource = Environment::resource;
}

#[test]
fn a_path_is_refused_above_the_bound_before_it_is_read() {
    let path = temp_file("long", 8192);
    let limits = Limits {
        max_input_bytes: 4096,
        ..Limits::default()
    };
    let meter = Meter::new(1 << 20, Arc::new(Unbounded), Reservation::Input);
    let e = read_path(&path.0, &limits, &meter).unwrap_err();
    // The stat'd length is refused; a read to the bound would report 4097.
    assert!(
        matches!(&e, Error::Limit(l) if l.kind() == LimitKind::InputBytes && l.value() == 8192),
        "{e:?}"
    );
    assert_eq!(meter.used(), 0, "nothing was read or charged");
}

#[test]
fn a_path_is_charged_to_the_meter_and_released_with_its_bytes() {
    let path = temp_file("fits", 4096);
    let limits = Limits {
        max_input_bytes: 4096,
        ..Limits::default()
    };
    let meter = Meter::new(1 << 20, Arc::new(Unbounded), Reservation::Input);
    let bytes = read_path(&path.0, &limits, &meter).unwrap();
    assert_eq!(bytes.len(), 4096);
    assert_eq!(meter.used(), heap_cost(4096) + OWNER_COST);
    drop(bytes);
    assert_eq!(meter.used(), 0);
    // A meter too small for the file refuses before allocating.
    let small = Meter::new(1000, Arc::new(Unbounded), Reservation::Input);
    let e = read_path(&path.0, &limits, &small).unwrap_err();
    assert!(
        matches!(&e, Error::Limit(l) if l.kind() == LimitKind::InputMemory),
        "{e:?}"
    );
}

#[test]
fn a_path_needs_only_its_own_size_from_the_meter_and_the_budget() {
    // Round 3: the end-of-file check grew the full buffer by 64 KiB, so
    // the old and new blocks were charged together and a file over half
    // the budget (or one byte under max_input_bytes) was refused.
    use crate::budget::Counting;
    let len = 3 << 20;
    let path = temp_file("exact", len);
    let need = heap_cost(u64::try_from(len).unwrap()) + OWNER_COST;
    let limits = Limits {
        max_input_bytes: u64::try_from(len).unwrap() + 1,
        ..Limits::default()
    };
    let pool = Arc::new(Counting::new(need));
    let meter = Meter::new(need, pool.clone(), Reservation::Dependency);
    let bytes = read_path(&path.0, &limits, &meter).unwrap();
    assert_eq!(bytes.len(), len);
    assert_eq!((meter.used(), pool.used()), (need, need));
    drop(bytes);
    assert_eq!(pool.used(), 0);
}

#[cfg(unix)]
#[test]
fn a_file_longer_than_its_length_says_stops_at_the_bound() {
    // A device reports length 0 and never ends.
    let limits = Limits {
        max_input_bytes: 100_000,
        ..Limits::default()
    };
    let meter = Meter::new(1 << 30, Arc::new(Unbounded), Reservation::Input);
    let e = read_path(std::path::Path::new("/dev/zero"), &limits, &meter).unwrap_err();
    assert!(
        matches!(&e, Error::Limit(l) if l.kind() == LimitKind::InputBytes),
        "{e:?}"
    );
    assert_eq!(meter.used(), 0);
}
