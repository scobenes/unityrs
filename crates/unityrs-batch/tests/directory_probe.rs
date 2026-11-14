//! Preflight's case and normalization probe (`docs/BATCH_SPEC.md` 7.2): a sink
//! that folds two keys into one entry is refused before the `running` head.
use std::{ffi::CStr, fs, path::Path, sync::Arc};
use unityrs_batch::sink::{Class, fs::FileSystem};
use unityrs_testkit::scratch::Scratch;

#[allow(dead_code)]
mod directory_support;
use directory_support::{Faults, fs, head, preflight};

const CASE: (&CStr, &CStr) = (c".unityrs-tmp-probe-a", c".unityrs-tmp-probe-A");
const FORM: (&CStr, &CStr) = (c".unityrs-tmp-probe-\u{e9}", c".unityrs-tmp-probe-e\u{301}");

/// The entries left in the run's own directory but its head.
fn leftovers(sink: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(sink.join("_unityrs/r"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != "manifest.json")
        .collect();
    names.sort();
    names
}
/// Whether the host folds names in `dir`, by case and by normalization: the
/// native probe's answers, where no adapter folds.
fn host_folds(dir: &Path) -> (bool, bool) {
    fs::write(dir.join("fold-a"), b"").unwrap();
    fs::write(dir.join("fold-\u{e9}"), b"").unwrap();
    let folds = (
        dir.join("fold-A").exists(),
        dir.join("fold-e\u{301}").exists(),
    );
    fs::remove_file(dir.join("fold-a")).unwrap();
    fs::remove_file(dir.join("fold-\u{e9}")).unwrap();
    folds
}
/// `fs(fallback)` with the probe on, as production's adapter has it.
fn probing(fallback: bool) -> Arc<Faults> {
    Arc::new(Faults {
        fallback: fallback.then_some(libc::ENOSYS),
        crossing: std::sync::atomic::AtomicUsize::new(usize::MAX),
        semantics: true,
        ..Faults::default()
    })
}
fn folding(pair: (&'static CStr, &'static CStr), opens: bool) -> Arc<Faults> {
    Arc::new(Faults {
        fold: Some(pair),
        fold_opens: opens,
        crossing: std::sync::atomic::AtomicUsize::new(usize::MAX),
        semantics: true,
        ..Faults::default()
    })
}

#[test]
fn a_folding_sink_is_refused_with_a_fixed_detail_and_no_probe_left() {
    let probe = Scratch::new("directory-probe-host");
    if host_folds(probe.path()) != (false, false) {
        eprintln!("folding host: the adapter's folds are the host's own");
        return;
    }
    for (pair, detail) in [
        (CASE, "case_insensitive"),
        (FORM, "normalization_insensitive"),
    ] {
        // `EEXIST` on the second create, then one inode for both.
        for opens in [false, true] {
            let scratch = Scratch::new("directory-probe-fold");
            let error = preflight(scratch.path(), 1, folding(pair, opens))
                .err()
                .unwrap();
            assert_eq!(error.class, Class::SinkRootUnusable, "{detail} {opens}");
            assert_eq!(error.call, detail);
            assert_eq!(error.errno, None);
            assert_eq!(
                error.code(),
                unityrs_batch::reason::Code::CouldNotStart,
                "exit 69"
            );
            assert_eq!(
                leftovers(scratch.path()),
                Vec::<String>::new(),
                "{detail} {opens}"
            );
            assert!(!scratch.join("_unityrs/r/manifest.json").exists());
        }
    }
}

#[test]
fn a_sensitive_sink_passes_and_clears_its_probes_and_a_killed_attempts() {
    let probe = Scratch::new("directory-probe-host");
    if host_folds(probe.path()) != (false, false) {
        eprintln!("folding host: no sensitive sink to pass");
        return;
    }
    for fallback in directory_support::resolvers() {
        let scratch = Scratch::new("directory-probe-pass");
        fs::create_dir_all(scratch.join("_unityrs/r")).unwrap();
        // A killed attempt's probes, one of each pair.
        for name in [CASE.0, FORM.1] {
            fs::write(scratch.join("_unityrs/r").join(name.to_str().unwrap()), b"").unwrap();
        }
        let faults = probing(fallback);
        let directory = preflight(scratch.path(), 1, faults.clone()).unwrap();
        assert_eq!(leftovers(scratch.path()), Vec::<String>::new());
        let calls = faults.calls.lock().unwrap().clone();
        // Each pair: two clears, two creates, two removals.
        assert_eq!(calls.iter().filter(|c| **c == "unlink").count(), 8);
        directory.write_head(&head(1), true).unwrap();
        assert_eq!(leftovers(scratch.path()), Vec::<String>::new());
    }
}

#[test]
fn a_superseded_attempt_never_probes_and_a_probe_failure_refuses() {
    // Production's adapter probes; a test adapter that fakes the host may not.
    assert!(unityrs_batch::sink::fs::Host.probe_sink_semantics());
    let scratch = Scratch::new("directory-probe-off");
    let faults = fs(true);
    preflight(scratch.path(), 1, faults.clone()).unwrap();
    assert!(!faults.calls.lock().unwrap().contains(&"unlink"));
    let scratch = Scratch::new("directory-probe-fence");
    fs::create_dir_all(scratch.join("_unityrs/r")).unwrap();
    fs::write(scratch.join("_unityrs/r/manifest.json"), head(2)).unwrap();
    let faults = probing(true);
    let error = preflight(scratch.path(), 2, faults.clone()).err().unwrap();
    assert_eq!(error.class, Class::Superseded);
    assert!(!faults.calls.lock().unwrap().contains(&"unlink"));
    assert_eq!(leftovers(scratch.path()), Vec::<String>::new());
    // A clear the filesystem refuses is the root's own failure.
    let scratch = Scratch::new("directory-probe-unlink");
    let faults = Arc::new(Faults {
        fail: Some(("unlink", libc::EIO)),
        crossing: std::sync::atomic::AtomicUsize::new(usize::MAX),
        semantics: true,
        ..Faults::default()
    });
    let error = preflight(scratch.path(), 1, faults).err().unwrap();
    assert_eq!(error.class, Class::SinkRootUnusable);
    assert_eq!((error.call, error.errno), ("unlinkat", Some(libc::EIO)));
}

/// The host's own volume, unfaked: on a case-insensitive one (the default
/// APFS volume) the real probe refuses the sink, case first. With the probe
/// off, as the portable adapters have it, the same volume preflights.
#[cfg(target_os = "macos")]
#[test]
fn the_hosts_case_insensitive_volume_is_refused() {
    let scratch = Scratch::new("directory-probe-native");
    if !host_folds(scratch.path()).0 {
        eprintln!("case-sensitive volume: nothing to refuse");
        return;
    }
    let error = preflight(scratch.path(), 1, probing(true)).err().unwrap();
    assert_eq!(error.class, Class::SinkRootUnusable);
    assert_eq!(error.call, "case_insensitive");
    assert_eq!(leftovers(scratch.path()), Vec::<String>::new());
    drop(preflight(scratch.path(), 1, fs(true)).unwrap());
}
