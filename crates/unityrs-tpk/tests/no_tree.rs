//! Which of UnityPy's exceptions an object no tree answers raises, through
//! the core's object read and write (`docs/plans/core.md` 4.2; review round
//! 11: every case was the same `NoTypeTree`, and a binding must raise
//! UnityPy's). `tools/fixtures/no_tree.py` records each case: a stripped
//! file of one object, a synthetic TPK in place of UnityPy's, and what
//! `_get_typetree_node` did (`"tree"`, or the exception's name).

use std::path::Path;
use std::sync::Arc;

use serde_json::Value as J;
use unityrs_core::env::{EnvOptions, Environment, Source};
use unityrs_core::files::File;
use unityrs_core::limits::Limits;
use unityrs_core::provider::{NoTree, ProviderChain};
use unityrs_core::typetree::{NodeList, NodeRow, Tree};
use unityrs_core::walk::{ParseError, WriteError, WriteRules};
use unityrs_core::{Bytes, FileContext, RawStr};
use unityrs_tpk::TpkProvider;

fn hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// `TextAsset`'s tree (`m_Name`, `m_Script`), the one each case's file
/// was written with.
fn text_asset() -> Arc<Tree> {
    const ALIGN: i32 = 0x4000;
    let mut rows = vec![NodeRow::new(0, "TextAsset", "Base")];
    for name in ["m_Name", "m_Script"] {
        rows.extend([
            NodeRow::new(1, "string", name),
            NodeRow::new(2, "Array", "Array").with_meta_flag(ALIGN),
            NodeRow::new(3, "int", "size"),
            NodeRow::new(3, "char", "data"),
        ]);
    }
    Tree::compile(
        Arc::new(NodeList::trusted(rows).unwrap()),
        &Limits::default(),
    )
    .unwrap()
}

fn cases() -> Vec<J> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tpk/no_tree.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let fixture: J = serde_json::from_str(&text).unwrap();
    assert_eq!(fixture["unitypy"], "1.25.2");
    fixture["cases"].as_array().unwrap().clone()
}

/// What the core does with the case's object: `"tree"`, or the exception
/// its read's and its write's `NoTypeTree` name (`"none"` where it names
/// none).
fn outcome(case: &J) -> String {
    let tpk = TpkProvider::from_bytes(&hex(case["tpk"].as_str().unwrap()), &Limits::default());
    let mut ctx = FileContext::unitypy();
    ctx.providers = Arc::new(ProviderChain::new(vec![Arc::new(tpk.unwrap())]));
    let mut options = EnvOptions::unitypy();
    options.context = Arc::new(ctx);
    let env = Environment::new(options);
    let file = env.load(Source::Bytes {
        name: RawStr::from("CAB-N"),
        data: Bytes::from(hex(case["file"].as_str().unwrap())),
    });
    let Ok(File::Serialized(sf)) = file else {
        panic!("{case}: {file:?}")
    };
    let obj = sf.object(1).unwrap();
    if obj.tree(None).unwrap().is_some() {
        assert_eq!(obj.why_no_tree(), None, "{case}");
        return "tree".into();
    }
    let read = match obj.read_typetree(None, true) {
        Err(ParseError::NoTypeTree { why, .. }) => why,
        other => panic!("{case}: {other:?}"),
    };
    // A value to write: the object read with its own class's tree, given.
    let value = obj.read_typetree(Some(&text_asset()), true).unwrap();
    let write = match obj.write_typetree(&value, None, WriteRules::UnityPy) {
        Err(WriteError::NoTypeTree { why, .. }) => why,
        other => panic!("{case}: {other:?}"),
    };
    assert_eq!((read, write), (obj.why_no_tree(), read), "{case}");
    read.map_or("none", NoTree::exception).into()
}

/// Every recorded case gives UnityPy's outcome, and the cases cover every
/// exception and a tree.
#[test]
fn no_tree_names_the_exception_unitypy_raises() {
    let cases = cases();
    let mut seen = Vec::new();
    for case in &cases {
        let want = case["outcome"].as_str().unwrap();
        assert_eq!(outcome(case), want, "{case}");
        seen.push(want.to_owned());
    }
    for want in [
        "tree",
        "KeyError",
        "ValueError",
        "AssertionError",
        "NotImplementedError",
    ] {
        assert!(seen.iter().any(|s| s == want), "no case of {want}");
    }
}
