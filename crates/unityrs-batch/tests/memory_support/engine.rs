fn phase(n: u64) -> Phase {
    [
        Phase::Validate,
        Phase::Preflight,
        Phase::Start,
        Phase::Stage,
        Phase::Extract,
        Phase::Winners,
        Phase::Publish,
        Phase::Finish,
    ][usize::try_from(n).unwrap()]
}

#[test]
fn memory_child() {
    let Some(job) = std::env::var_os("X12_JOB") else {
        return;
    };
    let root = Path::new(&job).parent().unwrap();
    // Synthetic answer-table storage is outside the counted engine window.
    let resolver = std::env::var("X12_RESOLVE")
        .ok()
        .map(|text| unityrs_batch::run::hooks::Resolve::parse(&text).unwrap());
    let tls = std::env::var("X12_TLS").ok().map(|address| {
        (
            unityrs_batch::run::hooks::Resolve::parse(&format!("localhost={address}")).unwrap(),
            vec![rustls::pki_types::CertificateDer::from(tls::ROOT.to_vec())],
        )
    });
    let config = memory_support::Config {
        hold: std::env::var("X12_HOLD")
            .ok()
            .map(|n| phase(n.parse().unwrap())),
        ready: fs::OpenOptions::new()
            .append(true)
            .open(root.join("ready"))
            .unwrap(),
        release: root.join("release"),
        report: fs::OpenOptions::new()
            .append(true)
            .open(root.join("report"))
            .unwrap(),
        inject: std::env::var_os("X12_INJECT").is_some(),
        freeze: std::env::var_os("X12_FREEZE").is_some(),
        freeze_attempt: std::env::var_os("X12_ATTEMPT").is_some(),
    };
    // Step 1 has no production Rust heap (the separate startup probe pins
    // this). Parse test hooks and prepare their adapters outside the window.
    let mut startup = process::Startup::prepare().unwrap();
    startup.hooks().skip_host_checks = true;
    startup.hooks().sort_options = Some(unityrs_batch::sort::Options {
        run_bytes: 4096,
        run_records: 16,
        fan_in: 2,
    });
    if let Some(resolver) = resolver {
        startup.hooks().resolve = Some(resolver);
    }
    if let Some((resolver, roots)) = tls {
        startup.hooks().resolve = Some(resolver);
        startup.hooks().extra_roots = roots;
    }
    startup.hooks().tally = Some(Tally {
        register: memory_support::register,
        mark: memory_support::mark,
    });
    startup.hooks().files = Some(unityrs_batch::run::hooks::FileSystems {
        spool: std::sync::Arc::new(unityrs_batch::start::spool::Host),
        sink: std::sync::Arc::new(memory_support::Sink),
        started: std::sync::Arc::new(memory_support::Spool),
    });
    memory_support::start(config);
    let clock = Clock::process(Instant::now());
    memory::at(memory::Point::Phase(Phase::Validate));
    let (prepared, measured) = alloc::measured(|| {
        phase1::prepare(spec::parse(spec::read::read_job(Path::new(&job)).unwrap()).unwrap())
            .unwrap()
    });
    let bound = bound::memory_bound(
        &prepared.spec,
        measured,
        bound::CHECK_ENV_BLOCK,
        &SELECTED,
    );
    memory_support::selected(bound, prepared.spec.threads, measured.peak);
    process::run(startup, clock, prepared, bound.total(), None)
}

#[test]
fn startup_heap_child() {
    if std::env::var_os("X12_STARTUP_HEAP").is_none() { return; }
    unityrs_batch::diagnostic::panic::install();
    let (_, measured) = alloc::measured(|| process::Startup::prepare().unwrap());
    assert_eq!(measured, alloc::Measured::default(), "step-1 production heap changed");
}

#[test]
fn startup_without_test_hooks_has_no_rust_heap() {
    let out = child::output(Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "startup_heap_child", "--nocapture"])
        .env("X12_STARTUP_HEAP", "1")
        .env_remove("UNITYRS_TEST_TRACE")
        .env_remove("UNITYRS_TEST_POINT")).unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

