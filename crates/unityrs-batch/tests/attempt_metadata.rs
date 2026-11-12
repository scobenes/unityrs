mod attempt_support;
use attempt_support::*;
use serde_json::json;
use std::fs;
use unityrs_batch::{
    attempt::{Hooks, Point, ReasonCode, State},
    reason::Code,
};
use unityrs_export::ngui::{self, NoHooks};
use unityrs_testkit::ngui::{self as kit, Object, SceneBundle, V, file, pair, pptr, s, st, vector};
fn asset(path_id: i64, dependencies: Vec<V>) -> Object {
    Object::new(
        path_id,
        142,
        st(
            "AssetBundle",
            vec![
                ("m_PreloadTable", vector(pptr("Object", (0, 0)), vec![])),
                (
                    "m_Container",
                    vector(
                        pair(
                            s(""),
                            st("AssetInfo", vec![("asset", pptr("Object", (0, 0)))]),
                        ),
                        vec![],
                    ),
                ),
                ("m Dependencies", vector(s(""), dependencies)),
            ],
        ),
    )
}
#[test]
fn first_asset_bundle_per_selected_file_and_diagnostic_subsequence() {
    let first = file(
        &[],
        &[
            asset(1, vec![s("a"), s("A"), s("missing"), s("missing")]),
            asset(2, vec![s("ignored-second")]),
        ],
        true,
    )
    .unwrap();
    let second = file(&[], &[asset(1, vec![s("later")])], true).unwrap();
    let nested = unityrs_testkit::env::bundle(&[(
        "CAB-child",
        file(&[], &[asset(1, vec![s("ignored-nested")])], true).unwrap(),
    )]);
    let replaced = file(&[], &[asset(1, vec![s("ignored-shadow")])], true).unwrap();
    let mut scene = simple(&[]);
    scene.bundles[0].deps = vec!["a".into(), "z".into()];
    scene.bundles[0].data = unityrs_testkit::env::bundle(&[
        ("CAB-SAME", replaced),
        ("cab-same", first.clone()),
        ("CAB-SAME", first),
        ("second", second),
        ("nested", nested),
    ]);
    for id in ["a", "z"] {
        scene.bundles.push(SceneBundle {
            id: id.into(),
            data: unityrs_testkit::env::bundle(&[("CAB-SAME", file(&[], &[], true).unwrap())]),
            deps: vec![],
        });
    }
    let h = Harness::new(&scene, "x");
    let end = h.run(limits(), &NoHooks);
    clear(&end);
    assert_eq!(end.state, State::Done, "{end:?}");
    let rows = h.rows();
    let diagnostics: Vec<_> = rows.iter().filter(|r| r["type"] == "diagnostic").collect();
    assert_eq!(diagnostics[0]["kind"], "same_named_cab");
    assert_eq!(diagnostics[0]["bundle"], "x");
    assert_eq!(diagnostics[0]["name"], "CAB-SAME");
    let names: Vec<_> = diagnostics
        .iter()
        .filter(|r| r["kind"] == "undeclared_dependency")
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    // Both selected files of repeated casing have their own first object;
    // an exact repeated path selects only its replacement in files() order.
    assert_eq!(
        names,
        vec![
            "A", "missing", "missing", "A", "missing", "missing", "later"
        ]
    );
    assert_eq!(diagnostics.last().unwrap()["kind"], "nested_container");
    assert_eq!(diagnostics.last().unwrap()["name"], "nested");
    assert_eq!(end.diagnostics, diagnostics.len() as u64);
}
#[test]
fn earlier_unsupported_shadowed_row_wins_before_metadata() {
    struct Stop;
    impl ngui::Hooks for Stop {}
    impl Hooks for Stop {
        fn point(&self, p: Point) {
            assert_ne!(p, Point::Metadata);
        }
    }
    let mut scene = simple(&[]);
    let mut web = b"UnityWebData1.0\0".to_vec();
    web.resize(64, 0);
    let malformed = asset(1, vec![V::Int(1)]);
    scene.bundles[0].data = unityrs_testkit::env::bundle(&[
        ("shadow", web),
        ("shadow", file(&[], &[malformed], true).unwrap()),
    ]);
    let h = Harness::new(&scene, "x");
    let end = h.run(limits(), &Stop);
    clear(&end);
    assert_eq!(end.state, State::Failed);
    assert_eq!(end.reason, Some(ReasonCode::EntryError));
    assert_eq!(end.entry_detail.as_str(), "unsupported shadow");
    assert_eq!(end.code, Code::Incomplete);
}
#[test]
fn member_records_preserve_expansion_order_and_final_archive_keys() {
    let scene = simple(&[kit::texture(1, b"image", 4, (1, 1), vec![1, 2, 3, 4])]);
    let h = Harness::new(&scene, "x");
    let templates = vec![
        ngui::MemberTemplate {
            archive: 1,
            kind: ngui::TemplateKind::Texture,
            path: ngui::Template::parse("{name}.png", ngui::TemplateKind::Texture).unwrap(),
        },
        ngui::MemberTemplate {
            archive: 0,
            kind: ngui::TemplateKind::Texture,
            path: ngui::Template::parse("../{name}", ngui::TemplateKind::Texture).unwrap(),
        },
    ];
    let end = h.run_members(
        limits(),
        &NoHooks,
        &templates,
        &["long-final-archive.zip", "other.zip"],
    );
    clear(&end);
    assert_eq!(end.state, State::Done);
    let rows = h.rows();
    let members: Vec<_> = rows.iter().filter(|r| r["type"] == "member").collect();
    assert_eq!(members.len(), 2);
    assert_eq!(members[0]["archive"], 1);
    assert_eq!(members[1]["archive"], 0);
    assert_eq!(members[0]["path"], "image.png");
    assert_eq!(members[1]["reason"], "unwritable_path(dot_segment)");
    assert_eq!(
        rows.last().unwrap()["spooled_part"],
        json!(end.spooled_part)
    );
    assert!(fs::metadata(h.directory.path().join("o0")).unwrap().len() > 0);
}

#[test]
fn peek_index_is_validated_once_before_attempts() {
    use unityrs_batch::attempt::{PeekEntry, PeekTable};
    let entry = |id| PeekEntry {
        id,
        rows: 0,
        file: None,
    };
    assert!(PeekTable::new(&[entry("a"), entry("b")]).is_ok());
    assert!(PeekTable::new(&[entry("b"), entry("a")]).is_err());
    assert!(PeekTable::new(&[entry("a"), entry("a")]).is_err());
}
