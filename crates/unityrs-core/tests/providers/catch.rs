//! Replay the fallback's synthetic reader matrix.
use super::*;

#[derive(Debug)]
struct CatchClasses {
    head: Arc<Tree>,
    script: Arc<Tree>,
    missing: Option<unityrs_core::provider::NoTree>,
}
impl TypeTreeProvider for CatchClasses {
    fn name(&self) -> &'static str {
        "catch"
    }
    fn accepts_scripts(&self) -> bool {
        true
    }
    fn tree(&self, req: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
        if req.script.is_some() {
            return Ok(None);
        }
        Ok(match req.class_id {
            114 => Some(self.head.clone()),
            115 if self.missing.is_none() => Some(self.script.clone()),
            _ => None,
        })
    }
    fn why_none(
        &self,
        class_id: i32,
        _: Option<&unityrs_core::UnityVersion>,
    ) -> Option<unityrs_core::provider::NoTree> {
        (class_id == 115).then_some(self.missing).flatten()
    }
}

fn catch_kind(error: &unityrs_core::walk::ParseError) -> &'static str {
    use unityrs_core::env::DerefError;
    use unityrs_core::walk::{ParseError, ReadDetail};
    match error {
        ParseError::NoTypeTree { why: Some(why), .. } => why.exception(),
        ParseError::Deref(DerefError::Null) | ParseError::CheckRead { .. } => "ValueError",
        ParseError::Deref(DerefError::MissingObject { .. }) => "KeyError",
        ParseError::Deref(DerefError::FileIdBelowStart { .. }) => "IndexError",
        ParseError::Deref(_) => "FileNotFoundError",
        ParseError::Read(r) => match &r.detail {
            ReadDetail::Python(kind) => kind.split(':').next().unwrap(),
            ReadDetail::Eof { exception, .. } => exception,
            ReadDetail::PairChildren
            | ReadDetail::ArrayChildren
            | ReadDetail::NegativeLength(_)
            | ReadDetail::RefTypeNotFound
            | ReadDetail::NoRefTypes => "ValueError",
            ReadDetail::RefTypeNoTree => "AttributeError",
            ReadDetail::Stream(e) => e.exception(),
            other => panic!("unexpected detail: {other:?}"),
        },
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn automatic_catch_scope_replays_reader_lookup_and_dereference_failures() {
    replay_catch_scope(None);
}

#[test]
fn malformed_zero_byte_pair_preserves_python_index_error() {
    replay_catch_scope(Some("python_pair"));
}

#[test]
fn missing_script_version_entry_falls_back() {
    replay_catch_scope(Some("missing_entry"));
}

#[test]
fn python_struct_error_propagates() {
    replay_catch_scope(Some("python_eof"));
}

#[test]
fn reader_attribute_and_stream_errors_follow_their_actual_exception_categories() {
    use unityrs_core::env::MonoBehaviourError;
    use unityrs_core::stream::StreamRefusal;
    use unityrs_core::walk::{ParseError, ReadDetail, ReadError, ReadErrorKind};
    let f = fixture();
    let oracle = f["mono"]["automatic"].as_array().unwrap();
    let cases = [
        (ReadDetail::RefTypeNoTree, "AttributeError"),
        (
            ReadDetail::Stream(StreamRefusal::NegativePosition),
            "ValueError",
        ),
        (
            ReadDetail::Stream(StreamRefusal::PositionTooLarge),
            "ValueError",
        ),
        (
            ReadDetail::Stream(StreamRefusal::NegativeRead),
            "ValueError",
        ),
        (
            ReadDetail::Stream(StreamRefusal::ReadTooLarge),
            "OverflowError",
        ),
        (
            ReadDetail::Stream(StreamRefusal::ReadSizeOverflow),
            "OverflowError",
        ),
    ];
    for (detail, kind) in cases {
        let case = oracle.iter().find(|c| c["kind"] == kind).unwrap();
        let error = MonoBehaviourError::Parse(ParseError::Read(ReadError {
            kind: ReadErrorKind::Other,
            at: 0,
            detail,
        }));
        assert_eq!(
            error.is_value_error(),
            case.get("error").is_none(),
            "{kind}"
        );
    }
}

#[test]
fn boost_boolean_bounds_value_error_falls_back() {
    replay_catch_scope(Some("boost_bool"));
}

#[test]
fn absent_refs_keep_each_readers_exception() {
    replay_catch_scope(Some("python_absent_refs"));
    replay_catch_scope(Some("boost_absent_refs"));
}

#[test]
fn script_pointer_must_be_built_in_object_mode() {
    for kind in [
        "python_non_pptr",
        "boost_non_pptr",
        "boost_extra_pptr",
        "boost_bool_pointer",
        "boost_float_pointer",
        "python_bool_pointer",
        "python_float_pointer",
    ] {
        replay_catch_scope(Some(kind));
    }
}

#[test]
fn script_identity_interpolates_numeric_fields() {
    for (kind, fullname) in [
        ("numeric_namespace", "7.Example"),
        ("numeric_class", "Synthetic.7"),
        ("bool_namespace", "True.Example"),
        ("float_namespace", "1.5.Example"),
    ] {
        let f = fixture();
        let case = f["catch_scope"]
            .as_array()
            .unwrap()
            .iter()
            .find(|case| case["kind"] == kind)
            .unwrap();
        assert_eq!(case["calls"].as_array().unwrap().len(), 2, "{kind}");
        assert_eq!(case["calls"][0][1], fullname, "{kind}");
        replay_catch_scope(Some(kind));
    }
}

#[test]
fn every_reader_exception_boundary_matches_the_oracle() {
    use unityrs_core::env::MonoBehaviourError;
    use unityrs_core::walk::{ParseError, ReadDetail, ReadError, ReadErrorKind};
    let f = fixture();
    let cases = f["reader_exceptions"].as_array().unwrap();
    assert_eq!(cases.len(), 126);
    for case in cases {
        let kind = case["error"].as_str().unwrap();
        // Static names cover native reader errors. The dynamic representation
        // preserves ancestry for exceptions supplied by external Python code.
        let detail = match kind {
            "ValueError" => Some("ValueError"),
            "UnicodeError" => Some("UnicodeError"),
            "UnicodeDecodeError" => Some("UnicodeDecodeError"),
            "UnicodeEncodeError" => Some("UnicodeEncodeError"),
            "UnicodeTranslateError" => Some("UnicodeTranslateError"),
            "IndexError" => Some("IndexError"),
            "EOFError" => Some("EOFError"),
            "AttributeError" => Some("AttributeError"),
            "error" => Some("error"),
            "OverflowError" => Some("OverflowError"),
            "TypeError" => Some("TypeError"),
            "KeyError" => Some("KeyError"),
            _ => None,
        };
        let error = if let Some(kind) = detail {
            ParseError::Read(ReadError {
                kind: ReadErrorKind::Other,
                at: 0,
                detail: ReadDetail::Python(kind),
            })
        } else {
            let mut error = ProviderError::new("matrix", Arc::from("synthetic"));
            error.python_exception = Some(unityrs_core::provider::PythonException::new(
                Arc::from(kind),
                Arc::from("synthetic"),
                case["value_error"].as_bool().unwrap(),
            ));
            ParseError::Provider(error)
        };
        assert_eq!(
            MonoBehaviourError::Parse(error).is_value_error(),
            case["caught"].as_bool().unwrap(),
            "{case}"
        );
        assert_eq!(case["explicit"]["error"], kind, "{case}");
    }
}

#[test]
fn malformed_inputs_replay_every_reader_and_fallback_stage() {
    let f = fixture();
    let cases = f["malformed_readers"].as_array().unwrap();
    assert_eq!(cases.len(), 174);
    replay_cases(cases, None);
}

#[allow(clippy::too_many_lines)] // Replay the actual reader and dereference catch boundaries.
fn replay_catch_scope(only: Option<&str>) {
    let cases = fixture();
    let cases = cases["catch_scope"].as_array().unwrap();
    assert_eq!(cases.len(), 41);
    replay_cases(cases, only);
}

#[allow(clippy::too_many_lines)] // Replay the actual reader and dereference catch boundaries.
fn replay_cases(cases: &[J], only: Option<&str>) {
    use unityrs_core::provider::NoTree;
    if let Some(only) = only {
        assert!(
            cases.iter().any(|c| c["kind"] == only),
            "missing oracle case {only}"
        );
    }
    for case in cases {
        let kind = case["kind"].as_str().unwrap();
        if only.is_some_and(|only| only != kind) {
            continue;
        }
        let node_rows = |key: &str| {
            case[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    NodeRow::new(
                        i32::try_from(r[0].as_i64().unwrap()).unwrap(),
                        r[1].as_str().unwrap(),
                        r[2].as_str().unwrap(),
                    )
                    .with_meta_flag(r[8].as_i64().unwrap_or(0))
                })
                .collect()
        };
        let head = compile(node_rows("head"));
        let script_rows = node_rows("script");
        let missing = match kind {
            "missing_entry" => Some(NoTree::NoEntry),
            "unknown_class" => Some(NoTree::UnknownClass),
            "no_release_root" => Some(NoTree::NoReleaseRoot),
            "no_version" => Some(NoTree::NoVersion),
            _ => None,
        };
        let mut context = FileContext::unitypy();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        context.providers = Arc::new(ProviderChain::new(vec![
            Arc::new(CatchClasses {
                head: head.clone(),
                script: compile(script_rows),
                missing,
            }),
            Arc::new(GeneratorProvider::new(
                Recording {
                    kind: "miss".into(),
                    tree: head.clone(),
                    calls: calls.clone(),
                },
                &Meter::unlimited(),
                8,
                Arc::from("script memory refused"),
            )),
        ]));
        let mut options = EnvOptions::unitypy();
        options.context = Arc::new(context);
        let env = Environment::new(options);
        let bytes: Vec<u8> = case["file"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| u8::try_from(b.as_u64().unwrap()).unwrap())
            .collect();
        let scratch = unityrs_testkit::scratch::Scratch::new("providers");
        let source = if case["reader"] == "python_stream" {
            let path = scratch.join("matrix.assets");
            std::fs::write(&path, bytes).unwrap();
            unityrs_core::env::Source::Path(path)
        } else {
            unityrs_core::env::Source::Bytes {
                name: RawStr::from("catch.assets"),
                data: unityrs_core::Bytes::from(bytes),
            }
        };
        let unityrs_core::files::File::Serialized(file) = env.load(source).unwrap() else {
            panic!()
        };
        let obj = file.object(1).unwrap();
        let error = env.generate_monobehaviour_node(&obj, None).unwrap_err();
        match error {
            unityrs_core::env::MonoBehaviourError::Parse(error) => assert_eq!(
                catch_kind(&error),
                case["explicit"]["error"],
                "{kind}: {error:?}"
            ),
            unityrs_core::env::MonoBehaviourError::MissingNode(ident) => {
                assert_eq!(case["explicit"]["error"], "ValueError", "{kind}");
                assert_eq!(
                    json!([
                        ident.assembly.to_str(),
                        Some(format!(
                            "{}.{}",
                            ident.namespace.to_str().unwrap(),
                            ident.class.to_str().unwrap()
                        ))
                    ]),
                    case["calls"][0],
                    "{kind}"
                );
            }
            other @ unityrs_core::env::MonoBehaviourError::NoGenerator => {
                panic!("{kind}: {other:?}")
            }
        }
        match env.object_tree(&obj, None) {
            Ok(Some(tree)) => {
                assert!(case["automatic"].get("error").is_none(), "{kind}");
                assert!(Arc::ptr_eq(&tree.tree, &head));
            }
            Err(error) => assert_eq!(catch_kind(&error), case["automatic"]["error"], "{kind}"),
            other => panic!("{kind}: {other:?}"),
        }
        if let Some(expected) = case.get("calls") {
            // Recording is behind get_nodes_up, which adds the .dll suffix.
            let expected: Vec<J> = expected
                .as_array()
                .unwrap()
                .iter()
                .map(|ask| json!([format!("{}.dll", ask[0].as_str().unwrap()), ask[1]]))
                .collect();
            assert_eq!(*calls.lock().unwrap(), expected, "{kind}");
        }
    }
}
