//! Shader output, LZ4, tables and sorting charge before every allocation.
use std::fmt::Write;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use unityrs_core::{
    Bytes, File, FileContext, RawStr,
    budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation},
    env::{EnvOptions, Environment, Source},
    limits::Meter,
};
use unityrs_testkit::counted::{self, Allocator, Tally};
#[path = "support/shader_int.rs"]
mod shader_int;
const INT_DEFAULTS: &[f32] = &[
    1.,
    -3.,
    0.,
    -0.,
    1.25,
    0.1,
    16_777_215.,
    16_777_216.,
    16_777_218.,
    f32::MAX,
    f32::INFINITY,
];
static LIVE: AtomicUsize = AtomicUsize::new(0);
static BASE: AtomicUsize = AtomicUsize::new(0);
static HELD: AtomicU64 = AtomicU64::new(0);
static BASE_HELD: AtomicU64 = AtomicU64::new(0);
static OVER: AtomicU64 = AtomicU64::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static FAIL_AT: AtomicUsize = AtomicUsize::new(usize::MAX);
struct Watching;
impl Tally for Watching {
    fn grew(&self, size: usize, _: bool) {
        let now = LIVE.fetch_add(size, Ordering::Relaxed) + size;
        if ARMED.load(Ordering::Relaxed) {
            let above = now.saturating_sub(BASE.load(Ordering::Relaxed)) as u64;
            let charge = HELD
                .load(Ordering::Relaxed)
                .saturating_sub(BASE_HELD.load(Ordering::Relaxed));
            OVER.fetch_max(above.saturating_sub(charge), Ordering::Relaxed);
        }
    }
    fn shrank(&self, size: usize) {
        LIVE.fetch_sub(size, Ordering::Relaxed);
    }
    fn refuses(&self, _: usize) -> bool {
        if !counted::counted() || !ARMED.load(Ordering::Relaxed) {
            return false;
        }
        CALLS.fetch_add(1, Ordering::Relaxed) >= FAIL_AT.load(Ordering::Relaxed)
    }
}
#[global_allocator]
static ALLOC: Allocator<Watching> = Allocator(Watching);
#[derive(Debug)]
struct Home;
impl PermitHome for Home {
    fn release(&self, bytes: u64) {
        HELD.fetch_sub(bytes, Ordering::Relaxed);
    }
}
#[derive(Debug)]
struct Budget(Arc<Home>);
impl MemoryBudget for Budget {
    fn try_reserve(&self, bytes: u64, _: Reservation) -> Result<Permit, BudgetExceeded> {
        HELD.fetch_add(bytes, Ordering::Relaxed);
        Ok(Permit::new(bytes, self.0.clone()))
    }
}
#[test]
fn shader_allocations_are_charged_and_allocator_refusals_propagate() {
    counted::count_this_thread();
    counted::others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/shader.json")).unwrap();
    let modern: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/shader_modern.json")).unwrap();
    let legacy: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/shader_legacy.json")).unwrap();
    let ints = [false, true].map(|blob| {
        let mut input = String::new();
        for byte in shader_int::shader_with_properties(5, INT_DEFAULTS, blob) {
            write!(&mut input, "{byte:02x}").unwrap();
        }
        serde_json::json!({"case": format!("synthetic_int_blob_{blob}"), "input": input})
    });
    for case in fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| {
            c["oracle"].get("text").is_some()
                || matches!(
                    c["case"].as_str(),
                    Some("script_missing" | "script_string" | "script_invalid_utf8")
                )
        })
        .chain(
            modern["cases"]
                .as_array()
                .unwrap()
                .iter()
                .chain(legacy["cases"].as_array().unwrap())
                .filter(|c| c["rust"].get("text").is_some()),
        )
        .chain(ints.iter())
    {
        let text = case["input"].as_str().unwrap();
        let input: Vec<u8> = text
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect();
        let budget = Arc::new(Budget(Arc::new(Home)));
        let mut context = FileContext::unitypy();
        context.budget = budget.clone();
        let mut options = EnvOptions::unitypy();
        options.context = Arc::new(context);
        let env = Environment::new(options);
        let File::Serialized(file) = env
            .load(Source::Bytes {
                name: RawStr::from("synthetic.assets"),
                data: Bytes::from(input),
            })
            .unwrap()
        else {
            panic!("file")
        };
        let obj = file.object(1).unwrap();
        let meter = Meter::new(u64::MAX, budget, Reservation::Input);
        drop(
            unityrs_export::shader::export(&obj, &env, &meter)
                .unwrap_or_else(|e| panic!("{}: {e}", case["case"])),
        );
        let baseline = HELD.load(Ordering::Relaxed);
        BASE_HELD.store(baseline, Ordering::Relaxed);
        BASE.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
        OVER.store(0, Ordering::Relaxed);
        CALLS.store(0, Ordering::Relaxed);
        FAIL_AT.store(usize::MAX, Ordering::Relaxed);
        ARMED.store(true, Ordering::Relaxed);
        let result = unityrs_export::shader::export(&obj, &env, &meter);
        ARMED.store(false, Ordering::Relaxed);
        assert_eq!(OVER.load(Ordering::Relaxed), 0, "{}", case["case"]);
        let calls = CALLS.load(Ordering::Relaxed);
        drop(result.unwrap());
        assert_eq!(HELD.load(Ordering::Relaxed), baseline);
        for at in 0..calls {
            CALLS.store(0, Ordering::Relaxed);
            FAIL_AT.store(at, Ordering::Relaxed);
            ARMED.store(true, Ordering::Relaxed);
            let result = unityrs_export::shader::export(&obj, &env, &meter);
            ARMED.store(false, Ordering::Relaxed);
            assert!(result.is_err(), "{} allocation {at}", case["case"]);
            drop(result);
            assert_eq!(meter.used(), 0);
            assert_eq!(HELD.load(Ordering::Relaxed), baseline);
        }
        FAIL_AT.store(usize::MAX, Ordering::Relaxed);
    }
}

fn work_meter(work: u64) -> Arc<Meter> {
    Meter::with_decode_work(
        u64::MAX,
        work,
        Arc::new(unityrs_core::budget::Unbounded),
        Reservation::Input,
    )
}

#[test]
fn int_property_output_obeys_work_and_memory_bounds() {
    for blob in [false, true] {
        let env = Environment::new(EnvOptions::unitypy());
        let File::Serialized(file) = env
            .load(Source::Bytes {
                name: RawStr::from("int.assets"),
                data: Bytes::from(shader_int::shader_with_properties(5, INT_DEFAULTS, blob)),
            })
            .unwrap()
        else {
            panic!("file")
        };
        let parsed = file.object(1).unwrap().read_typetree(None, false).unwrap();
        let meter = work_meter(u64::MAX);
        let output = unityrs_export::shader::export_parsed(&parsed, (2021, 3), &meter).unwrap();
        assert!(
            output
                .as_slice()
                .windows(b"Integer) = 340282346638528859811704183484516925440\n".len())
                .any(|w| w == b"Integer) = 340282346638528859811704183484516925440\n")
        );
        drop(output);
        let work = meter.decoded();
        let short = work_meter(work - 1);
        assert!(matches!(
            unityrs_export::shader::export_parsed(&parsed, (2021, 3), &short),
            Err(unityrs_export::shader::Error::Charge(unityrs_core::limits::ChargeError::Limit(ref e)))
                if e.kind() == unityrs_core::error::LimitKind::DecodeWork
        ));
        assert_eq!(short.used(), 0);
        let exact = work_meter(work);
        drop(unityrs_export::shader::export_parsed(&parsed, (2021, 3), &exact).unwrap());
        assert_eq!(exact.used(), 0);
        let no_memory = Meter::new(
            0,
            Arc::new(unityrs_core::budget::Unbounded),
            Reservation::Input,
        );
        assert!(matches!(
            unityrs_export::shader::export_parsed(&parsed, (2021, 3), &no_memory),
            Err(unityrs_export::shader::Error::Charge(_))
        ));
        assert_eq!(no_memory.used(), 0);
    }
}

#[test]
fn no_blob_outputs_obey_the_work_bound() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/shader.json")).unwrap();
    for name in [
        "script_missing",
        "script_string",
        "script_empty",
        "script_utf8",
        "script_invalid_utf8",
    ] {
        let case = fixture["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["case"] == name)
            .unwrap();
        let input = case["input"]
            .as_str()
            .unwrap()
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u8::from_str_radix(std::str::from_utf8(p).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        let env = Environment::new(EnvOptions::unitypy());
        let File::Serialized(file) = env
            .load(Source::Bytes {
                name: RawStr::from("empty.assets"),
                data: Bytes::from(input),
            })
            .unwrap()
        else {
            panic!("file")
        };
        let parsed = file.object(1).unwrap().read_typetree(None, false).unwrap();
        for version in [(5, 4), (5, 5), (2021, 3)] {
            if version < (5, 5) && matches!(name, "script_missing" | "script_invalid_utf8") {
                continue;
            }

            let meter = work_meter(u64::MAX);
            drop(unityrs_export::shader::export_parsed(&parsed, version, &meter).unwrap());
            let work = meter.decoded();
            let short = work_meter(work - 1);
            assert!(
                matches!(unityrs_export::shader::export_parsed(&parsed, version, &short),
            Err(unityrs_export::shader::Error::Charge(unityrs_core::limits::ChargeError::Limit(ref e)))
                if e.kind() == unityrs_core::error::LimitKind::DecodeWork)
            );
            assert_eq!(short.used(), 0);
            let exact = work_meter(work);
            drop(unityrs_export::shader::export_parsed(&parsed, version, &exact).unwrap());
            assert_eq!(exact.used(), 0);
        }
    }
}

#[test]
fn malformed_block_and_invalid_script_are_charged_before_scanning() {
    use unityrs_export::shader::{self, Error};
    use unityrs_testkit::{
        ngui::{Object, V, st},
        scene3d::{Layout, file},
    };
    for script in [false, true] {
        let fields = if script {
            let mut script = vec![b'a'; 1 << 20];
            *script.last_mut().unwrap() = 0xff;
            vec![("m_Script", V::Bytes(script))]
        } else {
            let mut packed = vec![0xff; 1 << 20];
            packed[0] = 0xf0;
            vec![
                ("compressedBlob", V::Bytes(packed)),
                (
                    "platforms",
                    V::Vector("vector".into(), Box::new(V::Int(0)), vec![V::Int(0)]),
                ),
                (
                    "compressedLengths",
                    V::Vector("vector".into(), Box::new(V::Int(0)), vec![V::Int(1 << 20)]),
                ),
                (
                    "decompressedLengths",
                    V::Vector("vector".into(), Box::new(V::Int(0)), vec![V::Int(i32::MAX)]),
                ),
                (
                    "offsets",
                    V::Vector("vector".into(), Box::new(V::Int(0)), vec![V::Int(0)]),
                ),
            ]
        };
        let data = file(
            Layout::Unity2022,
            &[],
            &[Object::new(1, 48, st("Shader", fields))],
        )
        .unwrap();
        let env = Environment::new(EnvOptions::unitypy());
        let File::Serialized(file) = env
            .load(Source::Bytes {
                name: RawStr::from("synthetic.assets"),
                data: Bytes::from(data),
            })
            .unwrap()
        else {
            panic!("file")
        };
        let parsed = file.object(1).unwrap().read_typetree(None, false).unwrap();
        let version = if script { (5, 4) } else { (2022, 3) };
        let meter = work_meter(u64::MAX);
        let error = shader::export_parsed(&parsed, version, &meter).unwrap_err();
        if script {
            assert!(matches!(error, Error::Utf8 | Error::Utf8Bytes(_)));
        } else {
            assert!(matches!(
                error,
                Error::Core(unityrs_core::Error::Decompress { .. })
            ));
        }
        let charged = meter.decoded();
        assert!(charged >= (1 << 20), "unaccounted scan: {charged}");
        for work in [shader::HEADER.len() as u64, charged - 1] {
            let meter = work_meter(work);
            let error = shader::export_parsed(&parsed, version, &meter).unwrap_err();
            assert!(
                matches!(error, Error::Charge(unityrs_core::limits::ChargeError::Limit(ref e)) if e.kind() == unityrs_core::error::LimitKind::DecodeWork),
                "{error}"
            );
            assert_eq!(meter.used(), 0);
        }
        let meter = work_meter(charged);
        let error = shader::export_parsed(&parsed, version, &meter).unwrap_err();
        assert!(
            !matches!(error, Error::Charge(_)),
            "exact work allowance refused: {error}"
        );
    }
}

#[test]
fn field_name_searches_charge_hits_and_misses_at_the_work_bound() {
    use unityrs_export::shader::{self, Error};
    use unityrs_testkit::{
        ngui::{Object, V},
        scene3d::{Layout, file},
    };
    for hit in [false, true] {
        let name = "x".repeat(128);
        let mut fields: Vec<_> = (0..32).map(|i| (format!("{name}{i}"), V::Int(0))).collect();
        if hit {
            fields.insert(0, ("m_Script".into(), V::Bytes(Vec::new())));
        }
        let data = file(
            Layout::Unity2022,
            &[],
            &[Object::new(1, 48, V::Struct("Shader".into(), fields))],
        )
        .unwrap();
        let env = Environment::new(EnvOptions::unitypy());
        let File::Serialized(file) = env
            .load(Source::Bytes {
                name: RawStr::from("synthetic.assets"),
                data: Bytes::from(data),
            })
            .unwrap()
        else {
            panic!("file")
        };
        // This search exercises the legacy script path; modern shaders require
        // m_ParsedForm even when a script field is retained.
        let parsed = file.object(1).unwrap().read_typetree(None, false).unwrap();
        let meter = work_meter(u64::MAX);
        let answer = shader::export_parsed(&parsed, (5, 4), &meter);
        assert_eq!(answer.is_ok(), hit);
        let work = meter.decoded();
        assert!(
            work >= 32 * 128,
            "field names were searched without charging"
        );
        drop(answer);
        let bounded = work_meter(work - 1);
        assert!(matches!(
            shader::export_parsed(&parsed, (5, 4), &bounded),
            Err(Error::Charge(_))
        ));
        let exact = work_meter(work);
        let answer = shader::export_parsed(&parsed, (5, 4), &exact);
        if hit {
            drop(answer.unwrap());
        } else {
            assert!(matches!(answer, Err(Error::NoneBytes)));
        }
        assert_eq!(exact.used(), 0);
    }
}

#[test]
fn modern_detection_charges_for_passes_without_fields() {
    use unityrs_export::shader::{self, Error};
    use unityrs_testkit::{
        ngui::{Object, V, st},
        scene3d::{Layout, file},
    };
    let mut work = Vec::new();
    for count in [1, 1000] {
        let pass = st("SerializedPass", vec![]);
        let passes = V::Vector("vector".into(), Box::new(pass.clone()), vec![pass; count]);
        let sub = st("SerializedSubShader", vec![("m_Passes", passes)]);
        let data = file(
            Layout::Unity2022,
            &[],
            &[Object::new(
                1,
                48,
                st(
                    "Shader",
                    vec![
                        ("compressedBlob", V::Bytes(vec![1])),
                        (
                            "platforms",
                            V::Vector("vector".into(), Box::new(V::Int(0)), vec![]),
                        ),
                        (
                            "m_ParsedForm",
                            st(
                                "SerializedShader",
                                vec![(
                                    "m_SubShaders",
                                    V::Vector("vector".into(), Box::new(sub.clone()), vec![sub]),
                                )],
                            ),
                        ),
                    ],
                ),
            )],
        )
        .unwrap();
        let env = Environment::new(EnvOptions::unitypy());
        let File::Serialized(file) = env
            .load(Source::Bytes {
                name: RawStr::from("synthetic.assets"),
                data: Bytes::from(data),
            })
            .unwrap()
        else {
            panic!("file")
        };
        let object = file.object(1).unwrap();
        let meter = work_meter(u64::MAX);
        assert!(matches!(
            shader::export(&object, &env, &meter),
            Err(Error::Field("m_Name"))
        ));
        work.push(meter.decoded());
        let limited = work_meter(meter.decoded() - 1);
        assert!(matches!(
            shader::export(&object, &env, &limited),
            Err(Error::Charge(_))
        ));
        assert_eq!(limited.used(), 0);
    }
    assert!(
        work[1] >= work[0] + 999,
        "pass traversal was not charged: {work:?}"
    );
}
