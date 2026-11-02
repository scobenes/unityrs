//! [`collect`]'s check for a name two inputs give (review round 9: it
//! compared each name with every earlier one).
//!
//! The oracle takes one input, so the rule is the one [`collect`] states:
//! the first name, in its own collection order, that a later input gives
//! and an earlier one gave fails the run before any input after it is
//! collected, and one input may give a name twice. The expected outcomes
//! below are worked out from that rule by hand.

use std::cell::Cell;
use std::path::PathBuf;

use unityrs_testkit::scratch::Scratch;

use super::name::NAME_STEPS;
use super::{Selection, collect};

/// A folder of index files, removed however the test ends
/// (`unityrs_testkit::scratch`).
struct Folder(Scratch);

impl Folder {
    fn new(what: &str) -> Self {
        Self(Scratch::new(&format!("dump-select-{what}")))
    }

    /// An index file `name` whose entries are bundles at `paths`.
    fn index<S: AsRef<str>>(&self, name: &str, paths: &[S]) -> PathBuf {
        let entries: Vec<String> = paths
            .iter()
            .map(|p| format!(r#"{{"path": "{}", "kind": "bundle"}}"#, p.as_ref()))
            .collect();
        let path = self.0.join(name);
        std::fs::write(&path, format!(r#"{{"files": [{}]}}"#, entries.join(", "))).unwrap();
        path
    }
}

/// The names [`collect`] selects from `inputs`, each an index, or its error.
fn names(inputs: &[&PathBuf]) -> Names {
    let sel = Selection {
        inputs: inputs.iter().map(|p| (*p).clone()).collect(),
        index: true,
        ..Selection::default()
    };
    collect(&sel).map(|items| {
        items
            .iter()
            .map(|i| String::from_utf8_lossy(&i.name).into_owned())
            .collect()
    })
}

/// What [`collect`] selects: the names, or the run's error.
type Names = Result<Vec<String>, String>;

#[test]
fn a_name_two_inputs_give_fails_the_run_at_its_first_repeat() {
    let dir = Folder::new("repeat");
    let xyz = dir.index("xyz.json", &["x", "y", "z"]);
    let wzyz = dir.index("wzyz.json", &["w", "z", "y", "z"]);
    let vv = dir.index("vv.json", &["v", "v"]);
    let uv = dir.index("uv.json", &["u", "v"]);
    let missing = dir.0.join("missing.json");
    let repeat = |name: &str| Err(format!("{name} is named by two inputs"));
    let ok = |names: &[&str]| Ok(names.iter().map(|n| (*n).to_owned()).collect());
    let cases: [(&[&PathBuf], Names); 8] = [
        // The later input's first name the earlier one gave, in its order.
        (&[&xyz, &wzyz], repeat("z")),
        (&[&wzyz, &xyz], repeat("y")),
        (&[&xyz, &xyz], repeat("x")),
        // One input may give a name twice, alone or after another.
        (&[&vv], ok(&["v", "v"])),
        (&[&xyz, &vv], ok(&["v", "v", "x", "y", "z"])),
        // Any earlier input counts, not only the one just before.
        (&[&vv, &xyz, &uv], repeat("v")),
        // The repeat fails the run before a later input is read.
        (&[&xyz, &wzyz, &missing], repeat("z")),
        (&[&xyz, &missing, &wzyz], Err(String::new())),
    ];
    for (inputs, want) in cases {
        let got = names(inputs);
        match (&got, &want) {
            // A missing index fails with its own message.
            (Err(e), Err(w)) if w.is_empty() => {
                assert!(e.starts_with(&missing.display().to_string()), "{e}");
            }
            _ => assert_eq!(got, want, "{inputs:?}"),
        }
    }
}

/// Two inputs of N names each, every name distinct. Every `Eq` and `Hash`
/// call on a kept name is a step (`name.rs`), and the check reaches a name's
/// bytes through nothing else, so it cannot take work it does not count. A
/// hash set takes a few steps a name (it hashes each name to look it up and
/// to keep it, and again as it grows); a scan of the names kept compares
/// each of the second input's names with all N of the first's, N² steps
/// (16,777,216 at N = 4096) that no bound linear in N admits.
#[test]
fn the_repeat_check_takes_a_few_steps_a_name() {
    const N: usize = 4096;
    let dir = Folder::new("steps");
    let a = dir.index(
        "a.json",
        &(0..N).map(|i| format!("a/{i}")).collect::<Vec<_>>(),
    );
    let b = dir.index(
        "b.json",
        &(0..N).map(|i| format!("b/{i}")).collect::<Vec<_>>(),
    );
    NAME_STEPS.with(|s| s.set(0));
    let got = names(&[&a, &b]).unwrap();
    let steps = NAME_STEPS.with(Cell::get);
    assert_eq!(got.len(), 2 * N);
    let n = u64::try_from(N).unwrap();
    // At least the second input's N lookups and the 2N names kept; at most
    // 16 steps a name.
    assert!(
        (3 * n..=16 * n).contains(&steps),
        "{steps} name steps for {} names",
        2 * N
    );
}
