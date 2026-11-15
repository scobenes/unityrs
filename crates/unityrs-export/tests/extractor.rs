//! L9 tree replay: relative paths, lengths, SHA-256, returned identities and failures.
#![allow(clippy::format_collect, reason = "fixture digest spelling")]
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc};
use unityrs_core::{
    Bytes, ObjectRef, Parsed, RawStr,
    env::{EnvOptions, Environment, Source},
    limits::{Meter, MeteredVec},
};
use unityrs_export::extractor::{self, AssetFilter, Error, Options, Sink};
#[path = "extractor/budgets.rs"]
mod budgets;
#[path = "extractor/original.rs"]
mod original;
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
#[derive(Default)]
struct Files(BTreeMap<String, Vec<u8>>);
impl Sink for Files {
    fn directory(&mut self, _: &[u8]) -> Result<(), Error> {
        Ok(())
    }
    fn file(&mut self, relative: &[u8], bytes: &[u8]) -> Result<(), Error> {
        self.0.insert(hex(relative), bytes.to_vec());
        Ok(())
    }
}
struct Filter(Vec<i32>);
impl AssetFilter for Filter {
    fn accepts(&self, object: &ObjectRef, _: Option<&Parsed>) -> bool {
        self.0.contains(&object.class_id())
    }
}
fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
        .collect()
}
fn scene(input: &[u8]) -> Environment {
    let env = Environment::new(EnvOptions::unitypy());
    env.load(Source::Bytes {
        name: RawStr::from("synthetic.assets"),
        data: Bytes::copy_from_slice(input),
    })
    .unwrap();
    env
}
#[test]
fn extractor_tree_replays_the_l9_oracle() {
    let recorded: Value = serde_json::from_str(include_str!("fixtures/extractor.json")).unwrap();
    assert_eq!(recorded["reference"], "UnityPy 1.25.2");
    let cases = recorded["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 59);
    for case in cases {
        let label = case["case"].as_str().unwrap();
        let env = scene(&unhex(case["input"].as_str().unwrap()));
        let m = Meter::unlimited();
        let o = &case["options"];
        let filter = Filter(
            o["asset_filter"]
                .as_array()
                .map(|v| {
                    v.iter()
                        .map(|n| i32::try_from(n.as_i64().unwrap()).unwrap())
                        .collect()
                })
                .unwrap_or_default(),
        );
        let options = Options {
            use_container: o["use_container"].as_bool().unwrap_or(true),
            ignore_first_container_dirs: o["ignore_first_container_dirs"].as_i64().unwrap_or(0),
            append_path_id: o["append_path_id"].as_bool().unwrap_or(false),
            export_unknown_as_typetree: o["export_unknown_as_typetree"].as_bool().unwrap_or(false),
            asset_filter: (!filter.0.is_empty()).then_some(&filter as &dyn AssetFilter),
        };
        let mut files = Files::default();
        let report = extractor::extract_assets(&env, &options, &mut files, &m)
            .unwrap_or_else(|e| panic!("{label}: {e}"));
        let tree: BTreeMap<_,_> = files.0.iter().map(|(name,data)| (name.clone(),json!({"size":data.len(),"sha256":Sha256::digest(data).iter().map(|b|format!("{b:02x}")).collect::<String>()}))).collect();
        assert_eq!(
            json!(tree),
            case["expected"]["tree"],
            "{label}: tree; failures {:?}",
            report.failures
        );
        let identity = |obj: &ObjectRef| {
            json!({"serialized_file": obj.file().name().to_str().unwrap(),
            "path_id": obj.path_id(), "class_id": obj.class_id()})
        };
        let written: Vec<_> = report
            .written
            .as_slice()
            .iter()
            .map(|w| {
                let mut row = identity(&w.object);
                row["path"] = json!(hex(w.path.as_bytes()));
                row["size"] = json!(w.size);
                row
            })
            .collect();
        let failures: Vec<_> = report
            .failures
            .as_slice()
            .iter()
            .map(|f| {
                let mut row = identity(&f.object);
                row["path"] = json!(hex(f.path.as_bytes()));
                // These synthetic failures must stay typed mesh decode failures.
                assert!(
                    matches!(
                        f.error,
                        Error::Mesh(unityrs_mesh::MeshError::Decode(
                            unityrs_meshdecode::MeshError::NoAttribute("m_VertexData")
                        ))
                    ),
                    "{label}: {:?}",
                    f.error
                );
                row["reason"] = json!("mesh");
                row
            })
            .collect();
        let exported: Vec<_> = report.exported.as_slice().iter().map(|id|
            json!({"serialized_file":id.file.name().to_str().unwrap(),"path_id":id.path_id})).collect();
        assert_eq!(
            json!({"written":written,"failures":failures,"exported":exported}),
            case["expected"]["report"],
            "{label}: complete ordered report"
        );
        original::check(case);
        drop(report);
        assert_eq!(m.used(), 0, "{label}: leaked output charge");
    }
}
#[test]
fn filter_observes_metadata_then_fields_and_can_change_its_answer() {
    struct Fields;
    impl AssetFilter for Fields {
        fn accepts(&self, obj: &ObjectRef, parsed: Option<&Parsed>) -> bool {
            if let Some(p) = parsed {
                return p
                    .value()
                    .as_dict()
                    .and_then(|m| p.attr(m, b"m_Name"))
                    .is_none();
            }
            obj.class_id() == 49
        }
    }
    let input = unityrs_testkit::scene3d::file(
        unityrs_testkit::scene3d::Layout::Unity2022,
        &[],
        &[unityrs_testkit::ngui::Object::new(
            1,
            49,
            unityrs_testkit::ngui::st(
                "TextAsset",
                vec![
                    ("m_Name", unityrs_testkit::ngui::s("a")),
                    ("m_Script", unityrs_testkit::ngui::s("b")),
                ],
            ),
        )],
    )
    .unwrap();
    let env = scene(&input);
    let options = Options {
        use_container: false,
        asset_filter: Some(&Fields),
        ..Options::default()
    };
    let report =
        extractor::extract_assets(&env, &options, &mut Files::default(), &Meter::unlimited())
            .unwrap();
    assert!(report.written.is_empty() && report.failures.is_empty() && report.exported.is_empty());
}
#[test]
fn untrusted_paths_are_rejected_before_directory_slicing() {
    let m = Meter::unlimited();
    for path in [
        b"../x".as_slice(),
        b"/absolute",
        b"a/../x",
        b"a/\0x",
        b"C:/x",
        b"a\\b",
    ] {
        assert!(
            matches!(
                extractor::paths::container(path, 99, &m),
                Err(Error::UnsafePath)
            ),
            "{path:?}"
        );
    }
    assert_eq!(
        extractor::paths::container(b"a//b/c", 1, &m)
            .unwrap()
            .as_bytes(),
        b"b/c"
    );
    assert_eq!(
        extractor::paths::container(b"a/b/c", -1, &m)
            .unwrap()
            .as_bytes(),
        b"c"
    );
    for (name, stem, ext) in [
        (b".hidden".as_slice(), b".hidden".as_slice(), b"".as_slice()),
        (b"...x", b"...x", b""),
        (b"a.b.c", b"a.b", b".c"),
        (b"a/.x.ext", b"a/.x", b".ext"),
    ] {
        assert_eq!(extractor::paths::split(name), (stem, ext));
    }
}
#[test]
fn path_allocations_are_fallible_and_charged() {
    use unityrs_core::budget::{Counting, Reservation};
    let budget = Arc::new(Counting::new(0));
    let m = Meter::new(u64::MAX, budget, Reservation::Input);
    assert!(extractor::paths::container(b"a/b", 0, &m).is_err());
    assert_eq!(m.used(), 0);
    // Type-level guard: reports own their metered storage.
    let _: MeteredVec<u8> = MeteredVec::new(&m);
}

#[test]
fn output_budget_refusals_release_every_extractor_allocation() {
    use unityrs_core::budget::{Reservation, Unbounded};
    use unityrs_testkit::{
        ngui::{Object, s, st},
        scene3d::{Layout, file},
    };
    let input = file(
        Layout::Unity2022,
        &[],
        &[Object::new(
            1,
            49,
            st(
                "TextAsset",
                vec![("m_Name", s("text")), ("m_Script", s("payload"))],
            ),
        )],
    )
    .unwrap();
    let env = scene(&input);
    let mut completed = false;
    for memory in [0, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 16384] {
        let m = Meter::new(memory, Arc::new(Unbounded), Reservation::Input);
        let result = extractor::extract_assets(
            &env,
            &Options {
                use_container: false,
                ..Options::default()
            },
            &mut Files::default(),
            &m,
        );
        if let Ok(report) = &result {
            completed |= report.written.len() == 1;
        }
        assert!(m.used() <= memory);
        drop(result);
        assert_eq!(m.used(), 0, "budget {memory}");
    }
    assert!(completed);
}

#[test]
fn a_crawls_post_export_read_cannot_hide_a_work_refusal() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/extractor.json")).unwrap();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["case"] == "gameobject_cycles")
        .unwrap();
    let bytes = unhex(case["input"].as_str().unwrap());
    let env = scene(&bytes);
    let object = env.objects().next().unwrap().unwrap();
    let report = extractor::extract_assets(
        &env,
        &Options::default(),
        &mut Files::default(),
        &Meter::unlimited(),
    )
    .unwrap();
    assert_eq!(report.failures.len(), 0);
    let work = object.file().meter().decoded();
    assert!(work > 1);
    let mut options = EnvOptions::unitypy();
    let mut context = (*options.context).clone();
    context.limits.max_decode_work = work - 1;
    options.context = Arc::new(context);
    let env = Environment::new(options);
    env.load(Source::Bytes {
        name: RawStr::from("synthetic.assets"),
        data: Bytes::copy_from_slice(&bytes),
    })
    .unwrap();
    let result = extractor::extract_assets(
        &env,
        &Options::default(),
        &mut Files::default(),
        &Meter::unlimited(),
    );
    match result {
        Err(e) => assert!(e.resource_refusal(), "{e:?}"),
        Ok(report) => assert!(
            report
                .failures
                .as_slice()
                .iter()
                .any(|f| f.error.resource_refusal()),
            "terminal input work refusal was swallowed"
        ),
    }
}
