//! Count actual Rust allocation requests around every native output boundary.
use pyo3::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use unityrs_testkit::counted::{Allocator, Tally};

static ACTIVE: AtomicBool = AtomicBool::new(false);
static REQUESTED: AtomicUsize = AtomicUsize::new(0);
static REFUSE_PATH: AtomicBool = AtomicBool::new(false);
struct Requests;
impl Tally for Requests {
    fn refuses(&self, size: usize) -> bool {
        // This hook observes every thread, including detached native workers.
        // A refusal must precede any input-sized allocator request at all.
        if ACTIVE.load(Ordering::Relaxed) {
            REQUESTED.fetch_add(size, Ordering::Relaxed);
        }
        REFUSE_PATH.load(Ordering::Relaxed) && (8 << 20..=(8 << 20) + 64).contains(&size)
    }
    fn grew(&self, _: usize, _: bool) {}
    fn shrank(&self, _: usize) {}
}

#[test]
fn path_snapshot_returns_memory_error_when_allocator_refuses() -> PyResult<()> {
    const CHILD: &str = "UNITYRS_PY_PATH_ALLOCATOR_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let scratch = unityrs_testkit::scratch::Scratch::new("py-path-allocator");
        let output = unityrs_testkit::child::output(
            std::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "allocation_tests::path_snapshot_returns_memory_error_when_allocator_refuses",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .current_dir(scratch.path()),
        )?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return Ok(());
    }
    let _serial = crate::tests::serial();
    Python::attach(|py| {
        crate::tree::allocation_tests::bytearray_copy(py)?;
        for expression in [c"b'x' * (8 << 20)", c"'x' * (8 << 20)"] {
            let input = py.eval(expression, None, None)?;
            let meter = unityrs_core::limits::Meter::unlimited();
            REFUSE_PATH.store(true, Ordering::Relaxed);
            let result = crate::path_snapshot::snapshot(&input, &meter);
            REFUSE_PATH.store(false, Ordering::Relaxed);
            assert!(
                matches!(result, Err(ref error) if error.is_instance_of::<pyo3::exceptions::PyMemoryError>(py))
            );
            assert_eq!(meter.used(), 0);
        }
        Ok(())
    })
}

pub(crate) fn refusing_large_input<T>(operation: impl FnOnce() -> T) -> T {
    REFUSE_PATH.store(true, Ordering::Relaxed);
    let result = operation();
    REFUSE_PATH.store(false, Ordering::Relaxed);
    result
}
#[global_allocator]
static ALLOCATOR: Allocator<Requests> = Allocator(Requests);

#[test]
fn native_output_entrypoints_charge_before_input_sized_allocations() -> PyResult<()> {
    let _serial = crate::tests::serial();
    Python::attach(|py| {
        let module = PyModule::new(py, "unityrs._native")?;
        crate::native(&module)?;
        let sys = py.import("sys")?;
        sys.getattr("modules")?
            .set_item("unityrs._native", &module)?;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        for directory in [root.join("python"), root.join("../../tools/parity")] {
            sys.getattr("path")?
                .call_method1("insert", (0, directory.to_string_lossy().as_ref()))?;
        }
        let binding = py.import("unityrs")?;
        binding.setattr("_native", &module)?;
        let cases = py
            .import("binding_y4_budget")?
            .getattr("native_cases")?
            .call1((binding,))?;
        assert_eq!(cases.len()?, 37, "maintain the native boundary matrix");
        let tracing = py.import("tracemalloc")?;
        for case in cases.try_iter()? {
            let case = case?;
            let name = case.get_item(0)?.extract::<String>()?;
            let call = case.get_item(1)?;
            tracing.call_method0("start")?;
            tracing.call_method0("reset_peak")?;
            REQUESTED.store(0, Ordering::Relaxed);
            ACTIVE.store(true, Ordering::Relaxed);
            let result = call.call0();
            ACTIVE.store(false, Ordering::Relaxed);
            let requested = REQUESTED.load(Ordering::Relaxed);
            let (_, peak) = tracing
                .call_method0("get_traced_memory")?
                .extract::<(usize, usize)>()?;
            tracing.call_method0("stop")?;
            assert!(
                matches!(result, Err(ref error) if error.is_instance_of::<pyo3::exceptions::PyMemoryError>(py)),
                "{name}: {result:?}"
            );
            assert!(
                requested < 1_000_000,
                "{name} requested {requested} bytes before refusal"
            );
            assert!(
                peak < 1_000_000,
                "{name} allocated {peak} Python bytes before refusal"
            );
            assert!(
                requested + peak < 1_000_000,
                "{name} exceeded the combined Rust/Python allocation allowance"
            );
        }
        Ok(())
    })
}
