#![allow(dead_code)]
use super::harness::{Case, Expansion, candidate};
use unityrs_batch::reason::{KeyRule, LeftOut};
use unityrs_export::ngui::Prefer;

pub fn scene(blocked: bool) -> Case {
    let mut case = Case::new(vec![
        candidate("a", "a", Some("a"), 1),
        candidate("b", "a", Some("a"), 2),
        candidate("a", "shared", Some("a"), 1),
        candidate("b", "z", Some("z"), 3),
        candidate("b", "shared", Some("z"), 3),
        candidate("c", "shared", None, 4),
        candidate("a", "held", Some("held"), 1),
        candidate("b", "held", Some("held"), 2),
        candidate("a", "held/child", Some("held"), 1),
        candidate("a", "p", Some("p"), 1),
        candidate("b", "p", Some("p"), 2),
        candidate("a", "outside", Some("p"), 1),
        candidate("b", "outside", Some("p"), 2),
        candidate("a", "p/leaf", None, 1),
        candidate("a", "chosen", None, 1),
        candidate("b", "chosen", None, 2),
        candidate("a", "other", None, 1),
    ]);
    case.failed.insert("f".into());
    case.publish_incomplete = !blocked;
    case.prefer.insert(
        "held".into(),
        Prefer {
            input: Some("f".into()),
            md5: None,
        },
    );
    case.prefer.insert(
        "chosen".into(),
        Prefer {
            input: Some("b".into()),
            md5: None,
        },
    );
    case.archives = vec![
        "zz/archive".into(),
        "aa/archive".into(),
        "empty/archive".into(),
    ];
    for (input, key, path, archive, reason) in [
        ("a", "chosen", "loser", 0, None),
        ("b", "chosen", "q", 0, None),
        ("a", "other", "q", 0, None),
        ("a", "shared", "q/child", 0, None),
        ("a", "p/leaf", "valid", 0, None),
        (
            "a",
            "p/leaf",
            "/invalid",
            1,
            Some(LeftOut::UnwritablePath(KeyRule::LeadingSlash)),
        ),
        (
            "a",
            "p/leaf",
            "q/child",
            0,
            Some(LeftOut::UnwritablePath(KeyRule::TooLong)),
        ),
        ("a", "outside", "withheld", 0, None),
    ] {
        case.expansions.push(Expansion {
            input: input.into(),
            key: key.into(),
            path: path.into(),
            archive,
            reason,
            cut: reason.is_some(),
        });
    }
    case
}
