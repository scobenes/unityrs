//! Refuse every allocation and all later allocations, including source reads.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
};
use unityrs_core::{
    Bytes, File, FileContext, RawStr,
    budget::{BudgetExceeded, MemoryBudget, Permit, PermitHome, Reservation},
    env::{EnvOptions, Environment, Source},
    limits::{Limits, Meter},
    provider::{ProviderChain, ProviderError, TreeRequest, TypeTreeProvider},
    typetree::{NodeList, NodeRow, Tree},
};
use unityrs_testkit::counted::{self, Allocator, Tally};
#[path = "support/simple_cases.rs"]
mod cases;
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
fn all_allocations_are_charged_before_growth_and_can_fail() {
    counted::count_this_thread();
    counted::others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    let plain = cases::cases()
        .into_iter()
        .filter(|c| c.name != "font_empty")
        .map(|case| (case, Arc::new(ProviderChain::default())));
    for (case, providers) in plain.chain(script_cases()) {
        let budget = Arc::new(Budget(Arc::new(Home)));
        let mut context = FileContext::unitypy();
        context.budget = budget.clone();
        context.providers = providers;
        let mut options = EnvOptions::unitypy();
        options.context = Arc::new(context);
        let env = Environment::new(options);
        let File::Serialized(file) = env
            .load(Source::Bytes {
                name: RawStr::from("synthetic.assets"),
                data: Bytes::from(case.input),
            })
            .unwrap()
        else {
            panic!("file")
        };
        let obj = file.object(1).unwrap();
        assert_eq!(obj.class_id(), case.class_id);
        let meter = Meter::new(u64::MAX, budget, Reservation::Input);
        let output = unityrs_export::simple::export(&obj, &env, case.extension, &meter).unwrap();
        if case.name == "mono_trailing" {
            // check_read must fall back without allocating an Arc<Parsed>
            // diagnostic: the refusal sweep below reaches every allocation.
            assert_eq!(
                output.as_ref().unwrap().fallback,
                Some(unityrs_export::simple::Fallback::ReadFailed)
            );
        }
        if case.name == "mono_script_missing" {
            let output = output.as_ref().unwrap();
            // UnityPy tools/extractor.py:210-224 catches the KeyError from
            // files/ObjectReader.py:332-350 and writes the raw behaviour.
            assert_eq!(output.extension, ".bin");
            assert_eq!(
                output.fallback,
                Some(unityrs_export::simple::Fallback::ReadFailed)
            );
            assert_eq!(
                output.bytes.as_slice(),
                &[0, 0, 0, 0, 77, 0, 0, 0, 0, 0, 0, 0]
            );
        }
        if case.name == "mono_script_trailing" {
            let output = output.as_ref().unwrap();
            // files/ObjectReader.py:285-307 swallows the MonoScript's
            // check_read ValueError, keeping the class tree: JSON succeeds.
            assert_eq!(output.extension, ".json");
            assert_eq!(output.fallback, None);
            assert_eq!(output.bytes.as_slice(), b"{\n    \"m_Script\": {\n        \"m_FileID\": 0,\n        \"m_PathID\": 77\n    }\n}");
        }
        drop(output);
        let baseline = HELD.load(Ordering::Relaxed);
        BASE_HELD.store(baseline, Ordering::Relaxed);
        BASE.store(LIVE.load(Ordering::Relaxed), Ordering::Relaxed);
        OVER.store(0, Ordering::Relaxed);
        CALLS.store(0, Ordering::Relaxed);
        FAIL_AT.store(usize::MAX, Ordering::Relaxed);
        ARMED.store(true, Ordering::Relaxed);
        let result = unityrs_export::simple::export(&obj, &env, case.extension, &meter);
        ARMED.store(false, Ordering::Relaxed);
        let calls = CALLS.load(Ordering::Relaxed);
        assert_eq!(OVER.load(Ordering::Relaxed), 0, "{}", case.name);
        drop(result.unwrap());
        assert_eq!(HELD.load(Ordering::Relaxed), baseline);
        for at in 0..calls {
            CALLS.store(0, Ordering::Relaxed);
            FAIL_AT.store(at, Ordering::Relaxed);
            ARMED.store(true, Ordering::Relaxed);
            let answer = unityrs_export::simple::export(&obj, &env, case.extension, &meter);
            ARMED.store(false, Ordering::Relaxed);
            assert!(answer.is_err(), "{} allocation {at}", case.name);
            drop(answer);
            assert_eq!(meter.used(), 0);
            assert_eq!(HELD.load(Ordering::Relaxed), baseline);
        }
    }
    counted::stop_counting_this_thread();
}

#[derive(Debug)]
struct ScriptClasses {
    behaviour: Arc<Tree>,
    script: Arc<Tree>,
}
impl TypeTreeProvider for ScriptClasses {
    fn name(&self) -> &'static str {
        "synthetic-script"
    }
    fn accepts_scripts(&self) -> bool {
        true
    }
    fn tree(&self, req: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
        assert!(
            req.script.is_none(),
            "a mismatched MonoScript must keep the class tree"
        );
        Ok(match req.class_id {
            114 => Some(self.behaviour.clone()),
            115 => Some(self.script.clone()),
            _ => None,
        })
    }
}
fn script_cases() -> Vec<(cases::Case, Arc<ProviderChain>)> {
    use unityrs_testkit::serialized::{ObjectSpec, SerializedSpec, TypeSpec, build};
    let tree = |rows| {
        Tree::compile(
            NodeList::trusted(rows).unwrap().into_shared().unwrap(),
            &Limits::default(),
        )
        .unwrap()
    };
    let provider = Arc::new(ProviderChain::new(vec![Arc::new(ScriptClasses {
        behaviour: tree(vec![
            NodeRow::new(0, "MonoBehaviour", "Base"),
            NodeRow::new(1, "PPtr<MonoScript>", "m_Script"),
            NodeRow::new(2, "int", "m_FileID"),
            NodeRow::new(2, "SInt64", "m_PathID"),
        ]),
        script: tree(vec![
            NodeRow::new(0, "MonoScript", "Base"),
            NodeRow::new(1, "int", "unused"),
        ]),
    })]));
    [true, false]
        .into_iter()
        .map(|trailing| {
            let mut objects = vec![ObjectSpec {
                path_id: 1,
                data: vec![0, 0, 0, 0, 77, 0, 0, 0, 0, 0, 0, 0],
                ..Default::default()
            }];
            if trailing {
                objects.push(ObjectSpec {
                    path_id: 77,
                    type_id: 1,
                    data: vec![7, 0, 0, 0, 255],
                    ..Default::default()
                });
            }
            (
                cases::Case {
                    name: if trailing {
                        "mono_script_trailing"
                    } else {
                        "mono_script_missing"
                    },
                    class_id: 114,
                    extension: "",
                    input: build(&SerializedSpec {
                        type_trees: false,
                        types: vec![
                            TypeSpec {
                                class_id: 114,
                                ..Default::default()
                            },
                            TypeSpec {
                                class_id: 115,
                                ..Default::default()
                            },
                        ],
                        objects,
                        ..Default::default()
                    }),
                },
                provider.clone(),
            )
        })
        .collect()
}
