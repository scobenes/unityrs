//! Synthetic UnityPy 1.25.2 API records, written by tools/fixtures/providers.py.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{Value as J, json};
use std::sync::Arc;
use unityrs_core::RawStr;
use unityrs_core::config::{FallbackWarnings, get_fallback_version};
use unityrs_core::env::{EnvOptions, Environment};
use unityrs_core::limits::MeteredVec;
use unityrs_core::limits::{Limits, Meter};
use unityrs_core::provider::generator::{
    DumpEntry, DumpProvider, GeneratorProvider, ScriptGenerator, generator_node_list,
};
use unityrs_core::provider::{ProviderChain, ProviderError, TreeRequest, TypeTreeProvider};
use unityrs_core::typetree::Tree;
use unityrs_core::typetree::{NodeDict, NodeRow, TypeTreeNode};
use unityrs_core::{FileContext, Parsed, Value};
use unityrs_testkit::ByteWriter;
use unityrs_testkit::serialized::{ObjectSpec, SerializedSpec, TypeSpec, build};

fn fixture() -> J {
    let f: J = serde_json::from_str(include_str!("fixtures/providers/api.json")).unwrap();
    assert_eq!(f["unitypy"], "1.25.2");
    f
}

fn dict(v: &J) -> NodeDict {
    let int = |s| {
        v.get(s)
            .and_then(J::as_i64)
            .map(|i| i32::try_from(i).unwrap())
    };
    let text = |s| v.get(s).and_then(J::as_str).map(RawStr::from);
    NodeDict {
        level: int("m_Level"),
        ty: text("m_Type"),
        name: text("m_Name"),
        byte_size: int("m_ByteSize"),
        version: int("m_Version"),
        children: v.get("m_Children").map_or_else(Vec::new, |children| {
            children
                .as_array()
                .unwrap()
                .iter()
                .map(|child| {
                    let child = dict(child);
                    let row = NodeRow {
                        level: child.level.unwrap(),
                        ty: child.ty.unwrap(),
                        name: child.name.unwrap(),
                        byte_size: child.byte_size.unwrap(),
                        version: child.version.unwrap(),
                        ..child.fields
                    };
                    Arc::new(TypeTreeNode::new(&row, &child.children, &Meter::unlimited()).unwrap())
                })
                .collect()
        }),
        ..NodeDict::default()
    }
}

fn rows(rows: &[NodeRow]) -> J {
    J::Array(
        rows.iter()
            .map(|r| {
                json!([
                    r.level,
                    r.ty.to_str(),
                    r.name.to_str(),
                    r.byte_size,
                    r.version,
                    r.type_flags,
                    r.variable_count,
                    r.index,
                    r.meta_flag,
                    r.ref_type_hash
                ])
            })
            .collect(),
    )
}

fn snapshot(list: &unityrs_core::typetree::NodeList) -> J {
    json!({"rows": rows(list.rows()), "children": (0..list.len()).map(|i| {
        list.children(unityrs_core::typetree::NodeId(u32::try_from(i).unwrap()))
            .iter().map(|id| id.index()).collect::<Vec<_>>()
    }).collect::<Vec<_>>()})
}

#[test]
fn from_list_replays_defaults_errors_levels_and_in_place_relinking() {
    let f = fixture();
    let meter = Meter::unlimited();
    for c in f["from_list"].as_array().unwrap() {
        let input: Vec<_> = c["input"].as_array().unwrap().iter().map(dict).collect();
        match TypeTreeNode::from_list(&input, &meter) {
            Ok(node) => assert_eq!(
                snapshot(&node.node_list(&Limits::default()).unwrap()),
                c["value"],
                "{c}"
            ),
            Err(e) => {
                assert_eq!(e.python_kind(), c["error"].as_str().unwrap(), "{c}: {e}");
                assert_eq!(e.to_string(), c["message"].as_str().unwrap(), "{c}");
            }
        }
    }
    let input: Vec<_> = f["from_list"][1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .map(dict)
        .collect();
    let mut node = TypeTreeNode::from_list(&input, &meter).unwrap();
    node.relink().unwrap();
    assert_eq!(
        rows(node.node_list(&Limits::default()).unwrap().rows()),
        f["relinked"]
    );
    drop(node);
    assert_eq!(meter.used(), 0);
}

#[test]
fn relinking_level_jumps_snapshots_actual_edges() {
    let meter = Meter::unlimited();
    let mut graph = TypeTreeNode::from_nodes(
        MeteredVec::from_vec(
            vec![
                NodeRow::new(0, "Synthetic", "Base"),
                NodeRow::new(3, "int", "x"),
                NodeRow::new(1, "int", "y"),
            ],
            &meter,
        )
        .unwrap(),
    )
    .unwrap();
    graph.relink().unwrap();
    assert_eq!(
        snapshot(&graph.node_list(&Limits::default()).unwrap()),
        fixture()["jumped_relinked"]
    );
}

#[test]
fn relinking_traversal_preserves_repeated_nested_node_identities() {
    let meter = Meter::unlimited();
    let mut graph = TypeTreeNode::from_nodes(
        MeteredVec::from_vec(
            vec![
                NodeRow::new(0, "Synthetic", "Base"),
                NodeRow::new(1, "int", "x"),
                NodeRow::new(2, "int", "y"),
            ],
            &meter,
        )
        .unwrap(),
    )
    .unwrap();
    for want in fixture()["nested_relinks"].as_array().unwrap() {
        graph.relink().unwrap();
        assert_eq!(
            rows(graph.node_list(&Limits::default()).unwrap().rows()),
            *want
        );
    }
}

#[test]
fn nodes_override_reads_the_recorded_value_and_relinked_nodes_read_past_it() {
    let f = fixture();
    let meter = Meter::unlimited();
    let input: Vec<_> = f["from_list"][1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .map(dict)
        .collect();
    let mut nodes = TypeTreeNode::from_list(&input, &meter).unwrap();
    let env = Environment::new(EnvOptions::unitypy());
    let bytes = build(&SerializedSpec {
        type_trees: false,
        types: vec![TypeSpec {
            class_id: 987_654,
            ..TypeSpec::default()
        }],
        objects: vec![ObjectSpec {
            path_id: 1,
            data: 42_i32.to_le_bytes().to_vec(),
            ..ObjectSpec::default()
        }],
        ..SerializedSpec::default()
    });
    let unityrs_core::files::File::Serialized(file) = env
        .load(unityrs_core::env::Source::Bytes {
            name: RawStr::from("explicit.assets"),
            data: unityrs_core::Bytes::from(bytes),
        })
        .unwrap()
    else {
        panic!()
    };
    let obj = file.object(1).unwrap();
    let tree = Tree::compile(
        Arc::new(nodes.node_list(&Limits::default()).unwrap()),
        &Limits::default(),
    )
    .unwrap();
    let parsed = obj.read_typetree(Some(&tree), true).unwrap();
    assert_eq!(
        json_value(&parsed, parsed.value()),
        f["node_reads"][0]["value"]
    );
    assert_eq!(
        json_value(&parsed, parsed.value()),
        f["node_reads"][1]["value"]
    );
    nodes.relink().unwrap();
    let tree = Tree::compile(
        Arc::new(nodes.node_list(&Limits::default()).unwrap()),
        &Limits::default(),
    )
    .unwrap();
    assert!(
        matches!(obj.read_typetree(Some(&tree),true), Err(unityrs_core::walk::ParseError::Read(e))
        if e.kind == unityrs_core::walk::ReadErrorKind::OutOfBounds)
    );
    assert_eq!(f["node_reads"][2]["error"], "EOFError");
    assert_eq!(f["node_reads"][2]["message"], "read_i out of bounds");
    assert_eq!(f["node_reads"][0]["case"], "dictionary_nodes");
    assert_eq!(f["node_reads"][1]["case"], "node_instance");
    assert_eq!(f["node_reads"][2]["case"], "relinked_node_instances");
}

#[test]
fn empty_list_and_tuple_rejections_are_explicitly_deferred_to_dynamic_binding() {
    // Rust's nodes= accepts a compiled Tree. Dynamic list/tuple dispatch is a
    // binding responsibility; retain and check both oracle cases explicitly.
    assert_eq!(
        fixture()["node_reads"][3],
        json!({
            "case": "empty_list_constructor_rejection_deferred_to_binding",
            "error": "IndexError", "message": "list index out of range"
        })
    );
    assert_eq!(
        fixture()["node_reads"][4],
        json!({
            "case": "empty_tuple_type_rejection_deferred_to_binding",
            "error": "ValueError", "message": "nodes must be a list[dict] or TypeTreeNode"
        })
    );
}

#[test]
fn generator_miss_interpolates_decoded_script_names() {
    use unityrs_core::env::MonoBehaviourError;
    for c in fixture()["mono"]["miss_messages"].as_array().unwrap() {
        let error = MonoBehaviourError::MissingNode(unityrs_core::provider::ScriptIdent {
            assembly: RawStr::from(c["assembly"].as_str().unwrap()),
            namespace: RawStr::from(c["namespace"].as_str().unwrap()),
            class: RawStr::from(c["class"].as_str().unwrap()),
        });
        assert!(error.is_value_error());
        assert_eq!(c["error"], "ValueError");
        assert_eq!(error.to_string(), c["message"].as_str().unwrap());
    }
}

#[test]
fn fallback_returns_any_string_and_warns_once_per_message() {
    let meter = Meter::unlimited();
    let mut warnings = FallbackWarnings::new(&meter);
    for c in fixture()["fallback"].as_array().unwrap() {
        let value = c["input"].as_str().map(RawStr::from);
        let mut seen = Vec::new();
        let got = get_fallback_version(value.as_ref(), &mut warnings, |w| {
            seen.push(json!({"category": w.category(), "message": w.to_string()}));
        });
        match got {
            Ok(s) => assert_eq!(s.to_str(), c["value"].as_str()),
            Err(e) => {
                assert_eq!(e.python_kind(), c["error"].as_str().unwrap());
                assert_eq!(e.to_string(), c["message"].as_str().unwrap());
            }
        }
        assert_eq!(J::Array(seen), c["warnings"]);
    }
    drop(warnings);
    assert_eq!(meter.used(), 0);
}

#[test]
fn version_resolution_warns_before_parsing_and_shares_the_once_filter() {
    use unityrs_core::config::{FilteredWarnings, UnityVersionFallbackWarning};
    use unityrs_core::version::{VersionPolicy, resolve_serialized};
    let meter = Meter::unlimited();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recording = seen.clone();
    let mut policy = VersionPolicy::default();
    policy.fallback = Some(RawStr::from("not-a-version"));
    policy.fallback_warnings = Some(Arc::new(FilteredWarnings::new(
        &meter,
        move |w: UnityVersionFallbackWarning<'_>| {
            recording.lock().unwrap().push(w.to_string());
        },
    )));
    for _ in 0..2 {
        let mut charge = unityrs_core::limits::Charge::zero(&meter);
        assert!(resolve_serialized(&RawStr::from(""), None, &policy, &mut charge).is_err());
    }
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            fixture()["fallback"][3]["warnings"][0]["message"]
                .as_str()
                .unwrap()
        ]
    );
    drop(policy);
    assert_eq!(meter.used(), 0);
}

#[test]
fn synthetic_dump_styles_replay_generator_normalization_and_cache_keys() {
    let f = fixture();
    let meter = Meter::unlimited();
    let input: Vec<_> = f["from_list"][1]["input"]
        .as_array()
        .unwrap()
        .iter()
        .map(dict)
        .collect();
    let node = TypeTreeNode::from_list(&input, &meter).unwrap();
    let mut source = MeteredVec::new(&meter);
    for r in node.node_list(&Limits::default()).unwrap().rows() {
        let mut r = r.clone();
        r.byte_size = 99;
        r.version = 5;
        r.type_flags = Some(7);
        source.push(r).unwrap();
    }
    assert_eq!(rows(source.as_slice()), f["generator"]["input"]);
    let list = generator_node_list(source).unwrap();
    assert_eq!(rows(list.rows()), f["generator"]["rows"]);
    let tree = Tree::compile(Arc::new(list), &Limits::default()).unwrap();
    for style in ["api", "il2cpp"] {
        let entry = if style == "api" {
            DumpEntry::api(
                RawStr::from("Assembly.dll"),
                &RawStr::from("Synthetic"),
                RawStr::from("Example"),
                tree.clone(),
                &meter,
            )
            .unwrap()
        } else {
            DumpEntry::il2cpp(
                RawStr::from("Assembly.dll"),
                RawStr::from("Synthetic.Example"),
                tree.clone(),
            )
        };
        let mut entries = MeteredVec::new(&meter);
        entries.push(entry).unwrap();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let provider = GeneratorProvider::new(
            RecordingDump {
                dump: DumpProvider::new(entries),
                calls: calls.clone(),
            },
            &meter,
            8,
            Arc::from("script memory refused"),
        );
        let a = RawStr::from("Assembly");
        let full = RawStr::from("Synthetic.Example");
        let first = provider.get_nodes_up(&a, &full).unwrap().unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &provider.get_nodes_up(&a, &full).unwrap().unwrap()
        ));
        provider
            .get_nodes_up(&RawStr::from("Assembly.dll"), &full)
            .unwrap()
            .unwrap();
        assert_eq!(
            J::Array(calls.lock().unwrap().clone()),
            f["generator"]["calls"]
        );
        assert_eq!(provider.cached(), 2);
        assert!(
            provider
                .get_nodes_up(&a, &RawStr::from("Missing"))
                .unwrap()
                .is_none()
        );
    }
}

#[derive(Debug)]
struct RecordingDump {
    dump: DumpProvider,
    calls: Arc<std::sync::Mutex<Vec<J>>>,
}
impl ScriptGenerator for RecordingDump {
    fn get_nodes(
        &self,
        assembly: &RawStr,
        fullname: &RawStr,
    ) -> Result<Option<Arc<Tree>>, ProviderError> {
        self.calls
            .lock()
            .unwrap()
            .push(json!([assembly.to_str(), fullname.to_str()]));
        self.dump.get_nodes(assembly, fullname)
    }
}

fn compile(rows: Vec<NodeRow>) -> Arc<Tree> {
    Tree::compile(
        Arc::new(unityrs_core::typetree::NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap()
}

fn strings(names: &[&str]) -> Arc<Tree> {
    let mut rows = vec![NodeRow::new(0, "MonoScript", "Base")];
    for name in names {
        rows.extend([
            NodeRow::new(1, "string", *name),
            NodeRow::new(2, "Array", "Array").with_meta_flag(0x4000),
            NodeRow::new(3, "int", "size"),
            NodeRow::new(3, "char", "data"),
        ]);
    }
    compile(rows)
}

#[derive(Debug)]
struct Classes {
    head: Arc<Tree>,
    script: Arc<Tree>,
}
impl TypeTreeProvider for Classes {
    fn name(&self) -> &'static str {
        "synthetic"
    }
    fn tree(&self, req: &TreeRequest<'_>) -> Result<Option<Arc<Tree>>, ProviderError> {
        Ok(match req.class_id {
            114 => Some(self.head.clone()),
            115 => Some(self.script.clone()),
            _ => None,
        })
    }
}

#[path = "providers/catch.rs"]
mod catch;

#[derive(Debug)]
struct Recording {
    kind: String,
    tree: Arc<Tree>,
    calls: Arc<std::sync::Mutex<Vec<J>>>,
}
impl ScriptGenerator for Recording {
    fn get_nodes(
        &self,
        assembly: &RawStr,
        fullname: &RawStr,
    ) -> Result<Option<Arc<Tree>>, ProviderError> {
        self.calls
            .lock()
            .unwrap()
            .push(json!([assembly.to_str(), fullname.to_str()]));
        match self.kind.as_str() {
            "hit" => Ok(Some(self.tree.clone())),
            "miss" => Ok(None),
            kind => {
                let mut e = ProviderError::new("recording", Arc::from("synthetic"));
                let message = match kind {
                    "KeyError" => "'synthetic'",
                    "UnicodeDecodeError" => {
                        "'utf-8' codec can't decode byte 0xff in position 0: synthetic"
                    }
                    _ => "synthetic",
                };
                e.python_exception = Some(unityrs_core::provider::PythonException::new(
                    Arc::from(kind),
                    Arc::from(message),
                    matches!(
                        kind,
                        "ValueError" | "UnicodeDecodeError" | "CustomValueError"
                    ),
                ));
                Err(e)
            }
        }
    }
}

fn json_value(parsed: &Parsed, v: &Value) -> J {
    match v {
        Value::Int(i) => json!(i),
        Value::Str(s) => json!(s.to_str()),
        Value::Dict(m) => J::Object(
            parsed
                .entries(m)
                .map(|(n, v)| {
                    (
                        std::str::from_utf8(n).unwrap().to_owned(),
                        json_value(parsed, v),
                    )
                })
                .collect(),
        ),
        other => panic!("unexpected synthetic value {other:?}"),
    }
}

#[test]
#[allow(clippy::too_many_lines)] // Complete legacy SerializedFile with embedded script.
fn explicit_generation_with_embedded_script_does_not_require_a_file_version() {
    let head = compile(vec![
        NodeRow::new(0, "MonoBehaviour", "Base"),
        NodeRow::new(1, "PPtr<MonoScript>", "m_Script"),
        NodeRow::new(2, "int", "m_FileID"),
        NodeRow::new(2, "SInt64", "m_PathID"),
    ]);
    let script = strings(&["m_AssemblyName", "m_Namespace", "m_ClassName"]);
    let generated = compile(vec![
        NodeRow::new(0, "Synthetic", "Base"),
        NodeRow::new(1, "int", "x"),
    ]);
    let mut entries = MeteredVec::new(&Meter::unlimited());
    entries
        .push(DumpEntry::il2cpp(
            RawStr::from("Assembly.dll"),
            RawStr::from("Synthetic.Example"),
            generated,
        ))
        .unwrap();
    let mut context = FileContext::unitypy();
    context.providers = Arc::new(ProviderChain::new(vec![Arc::new(GeneratorProvider::new(
        DumpProvider::new(entries),
        &Meter::unlimited(),
        8,
        Arc::from("refused"),
    ))]));
    let mut options = EnvOptions::unitypy();
    options.context = Arc::new(context);
    let env = Environment::new(options);
    let spec_nodes = |tree: &Tree| {
        tree.list()
            .rows()
            .iter()
            .map(|r| {
                let mut n = unityrs_testkit::blob::NodeSpec::new(
                    u8::try_from(r.level).unwrap(),
                    r.ty.to_str().unwrap(),
                    r.name.to_str().unwrap(),
                );
                n.meta_flag = i32::try_from(r.meta_flag.unwrap_or(0)).unwrap();
                n
            })
            .collect()
    };
    let mut data = ByteWriter::le();
    data.i32(0).i64(2).i32(42);
    let mut script_data = ByteWriter::le();
    for s in ["Assembly", "Synthetic", "Example"] {
        script_data
            .i32(i32::try_from(s.len()).unwrap())
            .bytes(s.as_bytes())
            .align(4);
    }
    let bytes = build(&SerializedSpec {
        format: 6,
        types: vec![
            TypeSpec {
                class_id: 114,
                nodes: spec_nodes(&head),
                ..TypeSpec::default()
            },
            TypeSpec {
                class_id: 115,
                nodes: spec_nodes(&script),
                ..TypeSpec::default()
            },
        ],
        objects: vec![
            ObjectSpec {
                path_id: 1,
                type_id: 114,
                class_id: 114,
                data: data.into_inner(),
                ..ObjectSpec::default()
            },
            ObjectSpec {
                path_id: 2,
                type_id: 115,
                class_id: 115,
                data: script_data.into_inner(),
                ..ObjectSpec::default()
            },
        ],
        ..SerializedSpec::default()
    });
    let unityrs_core::files::File::Serialized(file) = env
        .load(unityrs_core::env::Source::Bytes {
            name: RawStr::from("old.assets"),
            data: unityrs_core::Bytes::from(bytes),
        })
        .unwrap()
    else {
        panic!()
    };
    assert!(file.version().is_none());
    let tree = env
        .generate_monobehaviour_node(&file.object(1).unwrap(), Some(&head))
        .unwrap();
    assert_eq!(
        rows(tree.tree.list().rows()),
        fixture()["mono"]["old_explicit"]["value"]
    );
}

#[test]
#[allow(clippy::too_many_lines)] // One oracle scenario: setup, explicit methods and fallback.
fn monobehaviour_methods_and_automatic_fallback_replay_the_synthetic_oracle() {
    let f = fixture();
    let head = compile(vec![
        NodeRow::new(0, "MonoBehaviour", "Base"),
        NodeRow::new(1, "PPtr<MonoScript>", "m_Script"),
        NodeRow::new(2, "int", "m_FileID"),
        NodeRow::new(2, "SInt64", "m_PathID"),
    ]);
    let generated = compile(vec![
        NodeRow::new(0, "Synthetic", "Base"),
        NodeRow::new(1, "int", "x"),
    ]);
    for c in f["mono"]["automatic"].as_array().unwrap() {
        let kind = c["kind"].as_str().unwrap();
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut providers: Vec<Arc<dyn TypeTreeProvider>> = vec![Arc::new(Classes {
            head: head.clone(),
            script: strings(&["m_AssemblyName", "m_Namespace", "m_ClassName"]),
        })];
        if kind != "none" {
            providers.push(Arc::new(GeneratorProvider::new(
                Recording {
                    kind: kind.into(),
                    tree: generated.clone(),
                    calls: calls.clone(),
                },
                &Meter::unlimited(),
                8,
                Arc::from("script memory refused"),
            )));
        }
        let mut context = FileContext::unitypy();
        context.providers = Arc::new(ProviderChain::new(providers));
        let mut options = EnvOptions::unitypy();
        options.context = Arc::new(context);
        let env = Environment::new(options);
        let mut data = ByteWriter::le();
        data.i32(0).i64(2).i32(42);
        let mut script = ByteWriter::le();
        for s in ["Assembly", "Synthetic", "Example"] {
            script
                .i32(i32::try_from(s.len()).unwrap())
                .bytes(s.as_bytes())
                .align(4);
        }
        let bytes = build(&SerializedSpec {
            type_trees: false,
            types: vec![
                TypeSpec {
                    class_id: 114,
                    ..TypeSpec::default()
                },
                TypeSpec {
                    class_id: 115,
                    ..TypeSpec::default()
                },
            ],
            objects: vec![
                ObjectSpec {
                    path_id: 1,
                    type_id: 0,
                    data: data.into_inner(),
                    ..ObjectSpec::default()
                },
                ObjectSpec {
                    path_id: 2,
                    type_id: 1,
                    data: script.into_inner(),
                    ..ObjectSpec::default()
                },
            ],
            ..SerializedSpec::default()
        });
        let unityrs_core::files::File::Serialized(file) = env
            .load(unityrs_core::env::Source::Bytes {
                name: RawStr::from("synthetic.assets"),
                data: unityrs_core::Bytes::from(bytes),
            })
            .unwrap()
        else {
            panic!()
        };
        let obj = file.object(1).unwrap();
        let parsed = obj.parse_monobehaviour_head(None).unwrap();
        assert_eq!(parsed.bytes_read, 12);
        assert_eq!(
            parsed
                .get(&[b"m_Script", b"m_PathID"])
                .and_then(Value::as_i64),
            f["mono"]["head"]["value"].as_i64()
        );
        match env.object_tree(&obj, None) {
            Ok(Some(tree)) => {
                let parsed = obj.read_typetree(Some(&tree.tree), false).unwrap();
                assert_eq!(json_value(&parsed, parsed.value()), c["value"], "{kind}");
            }
            Err(unityrs_core::walk::ParseError::Provider(e)) => {
                assert_eq!(
                    e.python_exception
                        .as_ref()
                        .map(unityrs_core::provider::PythonException::as_str),
                    c["error"].as_str()
                );
                assert_eq!(
                    e.python_exception.as_ref().unwrap().message(),
                    c["message"].as_str().unwrap()
                );
            }
            other => panic!("{kind}: {other:?}"),
        }
        let explicit = match env.generate_monobehaviour_node(&obj, None) {
            Ok(tree) => json!({"value": rows(tree.tree.list().rows())}),
            Err(unityrs_core::env::MonoBehaviourError::Parse(
                unityrs_core::walk::ParseError::Provider(e),
            )) => {
                let e = e.python_exception.unwrap();
                json!({"error": e.as_str(), "message": e.message()})
            }
            Err(e) => {
                assert!(e.is_value_error());
                json!({"error": "ValueError", "message": e.to_string()})
            }
        };
        assert_eq!(explicit, c["explicit"], "{kind}");
        if kind == "hit" {
            let tree = env.generate_monobehaviour_node(&obj, None).unwrap();
            assert_eq!(
                rows(tree.tree.list().rows()),
                f["mono"]["generated"]["value"]
            );
            // The adapter adds .dll after ObjectReader passes the recorded identity.
            assert_eq!(
                *calls.lock().unwrap(),
                f["mono"]["calls"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|c| {
                        let assembly = c[0].as_str().unwrap();
                        json!([format!("{assembly}.dll"), c[1]])
                    })
                    .collect::<Vec<_>>()
            );
        } else if kind == "none" {
            let error = env.generate_monobehaviour_node(&obj, None).unwrap_err();
            assert!(error.is_value_error());
            assert_eq!(f["mono"]["missing"]["error"], "ValueError");
            assert_eq!(
                error.to_string(),
                f["mono"]["missing"]["message"].as_str().unwrap()
            );
        }
    }
}
