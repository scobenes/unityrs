//! Preflight's case and normalization probe (`docs/BATCH_SPEC.md` 7.2): a sink
//! that folds two keys into one entry is refused before the `running` head.
use std::{
    ffi::CStr,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};
use unityrs_batch::sink::{Class, fs::FileSystem};
use unityrs_testkit::scratch::Scratch;

#[allow(dead_code)]
mod directory_support;
use directory_support::{Faults, fs, head, preflight};

const CASE: (&CStr, &CStr) = (c".unityrs-tmp-probe-a", c".unityrs-tmp-probe-A");
const FORM: (&CStr, &CStr) = (c".unityrs-tmp-probe-\u{e9}", c".unityrs-tmp-probe-e\u{301}");

/// The probe entries left in the root and in the run's own directory, each
/// named from the root.
fn leftovers(sink: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for dir in ["", "_unityrs/r"] {
        let Ok(entries) = fs::read_dir(sink.join(dir)) else {
            continue;
        };
        for entry in entries {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            if name.starts_with(".unityrs-tmp-probe-") {
                names.push(format!("{dir}/{name}"));
            }
        }
    }
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
fn sensitive_host() -> bool {
    let probe = Scratch::new("directory-probe-host");
    let folds = host_folds(probe.path());
    if folds != (false, false) {
        eprintln!("folding host: the adapter's folds would be the host's own");
    }
    folds == (false, false)
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
fn folding(
    pair: (&'static CStr, &'static CStr),
    opens: bool,
    fold_dir: Option<PathBuf>,
) -> Arc<Faults> {
    Arc::new(Faults {
        fold: Some(pair),
        fold_opens: opens,
        fold_dir,
        crossing: std::sync::atomic::AtomicUsize::new(usize::MAX),
        semantics: true,
        ..Faults::default()
    })
}
fn refused(sink: &Path, faults: Arc<Faults>, detail: &str) {
    let error = preflight(sink, 1, faults).err().unwrap();
    assert_eq!(error.class, Class::SinkRootUnusable, "{detail}");
    assert_eq!(error.call, detail);
    assert_eq!(error.errno, None);
    assert_eq!(
        error.code(),
        unityrs_batch::reason::Code::CouldNotStart,
        "exit 69"
    );
    assert_eq!(leftovers(sink), Vec::<String>::new(), "{detail}");
    assert!(!sink.join("_unityrs/r/manifest.json").exists());
}

#[test]
fn a_folding_sink_is_refused_with_a_fixed_detail_and_no_probe_left() {
    if !sensitive_host() {
        return;
    }
    for (pair, detail) in [
        (CASE, "case_insensitive"),
        (FORM, "normalization_insensitive"),
    ] {
        // `EEXIST` on the second create, then one inode for both.
        for opens in [false, true] {
            let scratch = Scratch::new("directory-probe-fold");
            refused(scratch.path(), folding(pair, opens, None), detail);
        }
    }
}

/// ext4's casefold is per directory: an empty root set `+F` passes it to the
/// `_unityrs` made in it, which can be cleared while empty. The root folds
/// though the metadata directories do not, and the reverse is refused too.
#[test]
fn a_root_or_a_metadata_directory_folding_alone_is_refused() {
    if !sensitive_host() {
        return;
    }
    for (pair, detail) in [
        (CASE, "case_insensitive"),
        (FORM, "normalization_insensitive"),
    ] {
        let scratch = Scratch::new("directory-probe-root-folds");
        fs::create_dir_all(scratch.join("_unityrs/r")).unwrap();
        let faults = folding(pair, false, Some(scratch.path().to_owned()));
        refused(scratch.path(), faults, detail);
        let scratch = Scratch::new("directory-probe-run-folds");
        fs::create_dir_all(scratch.join("_unityrs/r")).unwrap();
        let faults = folding(pair, false, Some(scratch.join("_unityrs/r")));
        refused(scratch.path(), faults, detail);
    }
}

/// The same on a real ext4 volume, where the host can set the attribute: a
/// container's overlay cannot, so this skips there.
#[cfg(target_os = "linux")]
#[test]
fn a_real_casefolded_root_over_a_plain_metadata_directory_is_refused() {
    use unityrs_testkit::child;
    let scratch = Scratch::new("directory-probe-ext4");
    let chattr = |flag: &str, path: &Path| {
        child::output(std::process::Command::new("chattr").arg(flag).arg(path))
            .is_ok_and(|out| out.status.success())
    };
    if !chattr("+F", scratch.path()) {
        eprintln!("no casefold here: the adapter's case stands for it");
        return;
    }
    fs::create_dir(scratch.join("_unityrs")).unwrap();
    assert!(chattr("-F", &scratch.join("_unityrs")));
    refused(scratch.path(), probing(true), "case_insensitive");
}

#[test]
fn a_sensitive_sink_passes_and_clears_its_probes_and_a_killed_attempts() {
    if !sensitive_host() {
        return;
    }
    for fallback in directory_support::resolvers() {
        let scratch = Scratch::new("directory-probe-pass");
        fs::create_dir_all(scratch.join("_unityrs/r")).unwrap();
        // A killed attempt's probes, one of each pair in each directory.
        for dir in [scratch.path().to_owned(), scratch.join("_unityrs/r")] {
            for name in [CASE.0, FORM.1] {
                fs::write(dir.join(name.to_str().unwrap()), b"").unwrap();
            }
        }
        let faults = probing(fallback);
        let directory = preflight(scratch.path(), 1, faults.clone()).unwrap();
        assert_eq!(leftovers(scratch.path()), Vec::<String>::new());
        let calls = faults.calls.lock().unwrap().clone();
        // Each pair in each directory: two clears, two creates, two removals.
        assert_eq!(calls.iter().filter(|c| **c == "unlink").count(), 16);
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

#[test]
fn a_failed_removal_of_the_first_probe_still_removes_the_second() {
    if !sensitive_host() {
        return;
    }
    // Unlinks 1 and 2 clear the root's case pair; 3 removes its first probe.
    let scratch = Scratch::new("directory-probe-cleanup");
    let faults = Arc::new(Faults {
        fail_nth: Some(("unlink", 3, libc::EIO)),
        crossing: std::sync::atomic::AtomicUsize::new(usize::MAX),
        semantics: true,
        ..Faults::default()
    });
    let error = preflight(scratch.path(), 1, faults.clone()).err().unwrap();
    assert_eq!(error.class, Class::SinkRootUnusable);
    assert_eq!((error.call, error.errno), ("unlinkat", Some(libc::EIO)));
    let calls = faults.calls.lock().unwrap().clone();
    assert_eq!(calls.iter().filter(|c| **c == "unlink").count(), 4);
    assert_eq!(leftovers(scratch.path()), ["/.unityrs-tmp-probe-a"]);
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
    refused(scratch.path(), probing(true), "case_insensitive");
    drop(preflight(scratch.path(), 1, fs(true)).unwrap());
}
