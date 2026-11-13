//! Independent pairwise membership model, public rows and B8 list handoff.
mod artifact_support;
mod common;
#[path = "winners_support/harness.rs"]
mod harness;
#[path = "winners_support/probe.rs"]
mod probe;
use harness::{Case, Expansion, candidate};
use std::{
    collections::BTreeMap,
    sync::{Arc, atomic::Ordering},
};
use unityrs_batch::{
    phase::Account,
    reason::{KeyRule, LeftOut},
    sort::{NoHooks, Options},
    spool::{Counter, Host, workspace::Workspace},
    winners::{self, Context, Prepared, rowread},
};
use unityrs_testkit::scratch::Scratch;

fn scene() -> Case {
    let mut case = Case::new(
        (0..9)
            .flat_map(|i| {
                [
                    candidate("a", &format!("k{i}"), None, 1),
                    candidate("b", &format!("k{i}"), None, 2),
                ]
            })
            .collect(),
    );
    case.archives = vec![
        "z/archive".into(),
        "a/archive".into(),
        "invalid/archive".into(),
        "empty/archive".into(),
        "a/x".into(),
        "a!".into(),
    ];
    for (i, path) in ["p", "p", "p/x", "p!", "t", "t/bad", "é/x", "é", "/bad"]
        .into_iter()
        .enumerate()
    {
        for input in ["a", "b"] {
            case.expansions.push(Expansion {
                input: input.into(),
                key: format!("k{i}"),
                archive: 0,
                path: path.into(),
                reason: if i == 5 || i == 8 {
                    Some(LeftOut::UnwritablePath(KeyRule::LeadingSlash))
                } else {
                    None
                },
                cut: i == 8,
            });
        }
    }
    // Same displayed path as a valid expansion: invalids never compete.
    case.expansions.push(Expansion {
        input: "a".into(),
        key: "k0".into(),
        archive: 0,
        path: "p/x".into(),
        reason: Some(LeftOut::UnwritablePath(KeyRule::TooLong)),
        cut: true,
    });
    case.expansions.push(Expansion {
        input: "a".into(),
        key: "k0".into(),
        archive: 2,
        path: "/bad".into(),
        reason: Some(LeftOut::UnwritablePath(KeyRule::LeadingSlash)),
        cut: false,
    });
    // Bytewise archive order puts `a!` before `a/x`; slash-first reverses it.
    for archive in [4, 5] {
        case.expansions.push(Expansion {
            input: "a".into(),
            key: "k2".into(),
            archive,
            path: "member".into(),
            reason: None,
            cut: false,
        });
    }
    case
}
fn model(case: &Case) -> Vec<serde_json::Value> {
    let mut rows:Vec<_> = case.expansions.iter().filter(|m| m.input == "a").map(|m| {
        let same: Vec<_> = case.expansions.iter().filter(|r| r.input == "a" && r.archive == m.archive && r.path == m.path && r.reason.is_none()).collect();
        let keeper = same.iter().map(|r|r.key.as_str()).min();
        let reason = m.reason.or_else(|| if keeper != Some(m.key.as_str()) { Some(LeftOut::DuplicateMember) } else if case.expansions.iter().any(|r| r.input == "a" && r.archive == m.archive && r.reason.is_none() && r.path.starts_with(&format!("{}/",m.path))) { Some(LeftOut::PrefixConflict) } else { None });
        serde_json::json!({"archive":case.archives[m.archive],"path":m.path,"key":m.key,"input":m.input,"md5":"01".repeat(16),"included":reason.is_none(),"reason":reason.map(|r|r.to_string()),"cut":m.cut})
    }).collect();
    rows.sort_by(|a, b| {
        a["archive"]
            .as_str()
            .cmp(&b["archive"].as_str())
            .then_with(|| {
                // Independent order model: slash has rank zero, every other
                // byte its value plus one; iterator ordering handles prefixes.
                let rank = |b| if b == b'/' { 0 } else { u16::from(b) + 1 };
                a["path"]
                    .as_str()
                    .unwrap()
                    .bytes()
                    .map(rank)
                    .cmp(b["path"].as_str().unwrap().bytes().map(rank))
            })
            .then(a["key"].as_str().cmp(&b["key"].as_str()))
            .then(a["input"].as_str().cmp(&b["input"].as_str()))
    });
    rows
}
fn raw(cx: &Context<'_>, file: Option<&unityrs_batch::spool::workspace::Sealed>) -> Vec<u8> {
    use std::io::Read;
    let mut out = Vec::new();
    if let Some(f) = file {
        cx.workspace
            .open(f, &|| false)
            .unwrap()
            .read_to_end(&mut out)
            .unwrap();
    }
    out
}
#[test]
#[allow(clippy::too_many_lines)] // One fixture checks public rows and every handoff reader across sort settings.
fn membership_is_deterministic_and_matches_pairwise_model() {
    let mut canonical = None;
    for cut in [1, 2, 3, 8, u64::MAX] {
        for fan_in in [2, 3, 16] {
            for reverse in [false, true] {
                let mut case = scene();
                // Duplicate paths compete across output kinds, without kind
                // affecting path/key preference or their display ordering.
                case.kinds
                    .insert("k0".into(), unityrs_export::ngui::OutputKind::Texture);
                case.kinds
                    .insert("k1".into(), unityrs_export::ngui::OutputKind::Sprite);
                if reverse {
                    case.candidates.reverse();
                    case.expansions.reverse();
                }
                let expected = model(&case);
                let scratch = Scratch::new("members");
                let spec = case.spec().spec;
                let account = Account::new(64 << 20);
                let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
                let (mut commits, _) = case.commit(&scratch, &counter, &account, &spec);
                if reverse {
                    commits.order.reverse();
                }
                let prepared = Prepared::from_spec(&spec).unwrap();
                let mut workspace =
                    Workspace::create(scratch.join("w"), counter, Arc::new(Host)).unwrap();
                let mut cx = Context {
                    spec: &spec,
                    prepared: &prepared,
                    inputs: &commits,
                    workspace: &mut workspace,
                    account: &account,
                    options: Options {
                        run_records: cut,
                        fan_in,
                        ..Options::default()
                    },
                    stopped: &|| false,
                    hooks: &NoHooks,
                    observer: &winners::NoObserver,
                };
                let handoff = winners::complete(&mut cx).unwrap();
                let actual = raw(&cx, handoff.archive_members.as_ref());
                let values: Vec<serde_json::Value> = actual
                    .split(|&b| b == b'\n')
                    .filter(|b| !b.is_empty())
                    .map(|b| serde_json::from_slice(b).unwrap())
                    .collect();
                assert_eq!(values, expected);
                let bytes = [
                    raw(&cx, handoff.outputs.as_ref()),
                    raw(&cx, handoff.collisions.as_ref()),
                    raw(&cx, handoff.archives.as_ref()),
                    actual,
                ];
                let archive_order = [
                    "a!",
                    "a/archive",
                    "a/x",
                    "empty/archive",
                    "invalid/archive",
                    "z/archive",
                ];
                let archive_keys: Vec<String> = bytes[2]
                    .split(|&b| b == b'\n')
                    .filter(|b| !b.is_empty())
                    .map(|b| {
                        serde_json::from_slice::<serde_json::Value>(b).unwrap()["key"]
                            .as_str()
                            .unwrap()
                            .to_owned()
                    })
                    .collect();
                assert_eq!(archive_keys, archive_order);
                if let Some(expected) = &canonical {
                    assert_eq!(&bytes, expected);
                } else {
                    canonical = Some(bytes);
                }
                for (file, kind) in [
                    (handoff.outputs.as_ref(), rowread::File::Outputs),
                    (handoff.collisions.as_ref(), rowread::File::Collisions),
                    (handoff.archives.as_ref(), rowread::File::Archives),
                    (handoff.archive_members.as_ref(), rowread::File::Members),
                ] {
                    let mut reader =
                        rowread::Reader::open(cx.workspace, file, kind, account.meter(), &|| false)
                            .unwrap();
                    while reader.next(&|| false).unwrap().is_some() {}
                }
                let mut metadata = winners::archives::Reader::open(
                    cx.workspace,
                    handoff.metadata.as_ref().unwrap(),
                    account.meter(),
                    &|| false,
                )
                .unwrap();
                let mut listed = BTreeMap::new();
                let mut metadata_keys = Vec::new();
                while let Some(a) = metadata.next(&|| false).unwrap() {
                    metadata_keys.push(a.key.to_owned());
                    assert_eq!(a.key, spec.archives[usize::try_from(a.index).unwrap()].key);
                    let mut count = 0;
                    if let Some(list) = a.list {
                        let mut reader = winners::member::Reader::open(
                            cx.workspace,
                            &list,
                            account.meter(),
                            &|| false,
                        )
                        .unwrap();
                        while let Some(m) = reader.next(&|| false).unwrap() {
                            assert_eq!(m.archive, a.key);
                            assert!(m.reason.is_none());
                            count += 1;
                        }
                    }
                    assert_eq!(count, a.members);
                    listed.insert(a.key.to_owned(), (a.members, a.left_out));
                }
                assert_eq!(listed["invalid/archive"], (0, 1));
                assert_eq!(listed["empty/archive"], (0, 0));
                assert_eq!(listed["a!"], (1, 0));
                assert_eq!(listed["a/x"], (1, 0));
                assert_eq!(metadata_keys, archive_order);
                drop(metadata);
                assert_eq!(account.snapshot().input.used, 0);
                assert!(account.snapshot().input.peak <= 40 << 20);
            }
        }
    }
}

#[test]
fn complete_pipeline_ledger_instruments_each_pass_at_production_settings() {
    for block in [3072, 4096, 5003, 65_536] {
        let case = scene();
        let scratch = Scratch::new("members-ledger");
        let spec = case.spec().spec;
        let account = Account::new(64 << 20);
        let counter = Counter::new(u64::MAX, 0, block).unwrap();
        let (commits, spooled) = case.commit(&scratch, &counter, &account, &spec);
        let baseline = counter.used();
        let probe = probe::Probe::new(counter.clone(), baseline, block);
        let mut workspace = Workspace::create(
            scratch.join("w"),
            counter.clone(),
            Arc::new(probe::Fs(probe.clone())),
        )
        .unwrap();
        let prepared = Prepared::from_spec(&spec).unwrap();
        let handoff = winners::complete(&mut Context {
            spec: &spec,
            prepared: &prepared,
            inputs: &commits,
            workspace: &mut workspace,
            account: &account,
            options: Options::default(),
            stopped: &|| false,
            hooks: &NoHooks,
            observer: probe.as_ref(),
        })
        .unwrap();
        probe.sample();
        let k = if block < 4096 && 4096 % block != 0 {
            2
        } else {
            block.div_ceil(4096).max(1)
        };
        let s = spooled * k;
        let a: u64 = spec
            .archives
            .iter()
            .map(|a| {
                unityrs_batch::work::rounded(
                    u64::try_from(serde_json::to_string(&a.key).unwrap().len() - 2).unwrap() + 512,
                    block,
                )
                .unwrap()
            })
            .sum();
        for (pass, (peak, bound)) in probe.peaks.iter().zip(&probe.bounds).enumerate() {
            let peak = peak.load(Ordering::Relaxed);
            let bound = bound.load(Ordering::Relaxed);
            println!(
                "MEMBERS_LEDGER block={block} pass={pass} S={s} A={a} peak={peak} envelope={bound}"
            );
            assert!(peak <= 6 * s + 2 * a);
            assert!(bound <= 6 * s + 2 * a);
        }
        assert!(handoff.metadata.is_some());
        assert_eq!(account.snapshot().input.used, 0);
    }
}
