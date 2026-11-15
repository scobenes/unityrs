//! Exceptions inside tree lookup use the same raw fallback as a missing tree.
use std::sync::Arc;
use unityrs_core::{
    Bytes, File, FileContext, RawStr,
    env::{EnvOptions, Environment, Source},
    limits::{Limits, Meter},
    provider::{ProviderChain, ProviderError, TreeRequest, TypeTreeProvider},
    typetree::{NodeList, NodeRow, Tree},
};
use unityrs_export::simple::{self, Fallback};
use unityrs_testkit::serialized::{ObjectSpec, SerializedSpec, TypeSpec};
#[derive(Debug)]
struct Provider(Result<Arc<Tree>, ProviderError>);
impl TypeTreeProvider for Provider {
    fn name(&self) -> &'static str {
        "synthetic"
    }
    fn tree(&self, _: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
        self.0.clone().map(Some)
    }
    fn accepts_scripts(&self) -> bool {
        true
    }
}
fn run(
    answer: Result<Arc<Tree>, ProviderError>,
) -> Result<Option<simple::Export<'static>>, simple::Error> {
    run_provider(Arc::new(Provider(answer)))
}
fn run_provider(
    provider: Arc<dyn TypeTreeProvider>,
) -> Result<Option<simple::Export<'static>>, simple::Error> {
    let mut context = FileContext::unitypy();
    context.providers = Arc::new(ProviderChain::new(vec![provider]));
    let mut options = EnvOptions::unitypy();
    options.context = Arc::new(context);
    let env = Environment::new(options);
    let mut data = 0_i32.to_le_bytes().to_vec();
    data.extend_from_slice(&77_i64.to_le_bytes());
    let input = unityrs_testkit::serialized::build(&SerializedSpec {
        type_trees: false,
        types: vec![TypeSpec {
            class_id: 114,
            ..Default::default()
        }],
        objects: vec![ObjectSpec {
            path_id: 1,
            data,
            ..Default::default()
        }],
        ..Default::default()
    });
    let File::Serialized(file) = env
        .load(Source::Bytes {
            name: RawStr::from("synthetic.assets"),
            data: Bytes::from(input),
        })
        .unwrap()
    else {
        panic!("file")
    };
    simple::export(&file.object(1).unwrap(), &env, "", &Meter::unlimited())
}
#[test]
fn a_semantic_provider_failure_falls_back() {
    let export = run(Err(ProviderError::new(
        "synthetic",
        "failed to find a schema".into(),
    )))
    .unwrap()
    .unwrap();
    assert_eq!(export.extension, ".bin");
    assert_eq!(export.fallback, Some(Fallback::ReadFailed));
    assert_eq!(
        export.bytes.as_slice(),
        &[0, 0, 0, 0, 77, 0, 0, 0, 0, 0, 0, 0]
    );
}
#[test]
fn a_missing_script_object_falls_back() {
    let nodes = NodeList::trusted(vec![
        NodeRow::new(0, "MonoBehaviour", "Base"),
        NodeRow::new(1, "PPtr<MonoScript>", "m_Script"),
        NodeRow::new(2, "int", "m_FileID"),
        NodeRow::new(2, "SInt64", "m_PathID"),
    ])
    .unwrap();
    let tree = Tree::compile(nodes.into_shared().unwrap(), &Limits::default()).unwrap();
    let export = run(Ok(tree)).unwrap().unwrap();
    assert_eq!(export.extension, ".bin");
    assert_eq!(export.fallback, Some(Fallback::ReadFailed));
    assert_eq!(
        export.bytes.as_slice(),
        &[0, 0, 0, 0, 77, 0, 0, 0, 0, 0, 0, 0]
    );
}
#[test]
fn provider_memory_allocator_and_limit_refusals_propagate() {
    use unityrs_core::{
        LimitError, LimitKind,
        budget::{AllocError, BudgetExceeded, Reservation},
    };
    for variant in 0..3 {
        let mut error = ProviderError::new("synthetic", "refused".into());
        match variant {
            0 => {
                error.memory = Some(BudgetExceeded {
                    requested: 9,
                    what: Reservation::Provider,
                });
            }
            1 => {
                error.allocator = Some(AllocError {
                    requested: 9,
                    what: Reservation::Provider,
                });
            }
            _ => error.limit = Some(LimitError::refused(LimitKind::TreeDepth, 20, 10)),
        }
        assert!(run(Err(error)).is_err());
    }
}

#[test]
fn a_missing_tree_is_queried_once_before_raw_fallback() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Debug)]
    struct Miss(Arc<AtomicUsize>);
    impl TypeTreeProvider for Miss {
        fn name(&self) -> &'static str {
            "miss"
        }
        fn tree(&self, _: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(None)
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let export = run_provider(Arc::new(Miss(calls.clone())))
        .unwrap()
        .unwrap();
    assert_eq!(export.fallback, Some(Fallback::NoTypeTree));
    assert_eq!(export.extension, ".bin");
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}
