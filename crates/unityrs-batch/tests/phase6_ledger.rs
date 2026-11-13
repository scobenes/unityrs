//! Spec-derived archive rows and metadata have their own allowance.
//! This gate measures actual shared-counter claims, including retained files.
mod artifact_support;
mod common;
#[path = "ledger_support/empty_archives.rs"]
mod empty_archives;
#[path = "ledger_support/member_envelopes.rs"]
mod member_envelopes;

use serde_json::json;
use std::{io::Read, sync::Arc};
use unityrs_batch::{
    phase::Account,
    records::{self, Status},
    spool::{Counter, Directory, Full, Host, workspace::Workspace},
    work::{Work, rounded},
};
use unityrs_testkit::scratch::Scratch;

fn specification(
    archives: u32,
    inputs: u32,
    keys: empty_archives::Keys,
) -> unityrs_batch::spec::Validated {
    let mut doc = common::base();
    doc["inputs"] = (0..inputs)
        .map(|i| json!({"id":format!("i{i:06}"),"path":"/in/empty"}))
        .collect();
    doc["dependencies"] = json!([]);
    doc["archives"] = (0..archives)
        .map(|i| {
            json!({"key":empty_archives::family_key(i,keys),
                "members":[{"kind":"texture","path":"{name}.png"}]})
        })
        .collect();
    common::accepted(&doc)
}

/// Real B4 records, sealed and committed into numeric directories. No output
/// was selected, so each summary contributes exactly one 4 KiB work unit.
fn commit_empty_inputs(
    scratch: &Scratch,
    counter: &Arc<Counter>,
    account: &Account,
    spec: &unityrs_batch::spec::model::Spec,
) -> u64 {
    std::fs::create_dir(scratch.join("done")).unwrap();
    let mut spooled = 0;
    for i in 0..spec.input_count() {
        let mut directory = Directory::create(
            scratch.join(format!("attempt-{i}")),
            counter.clone(),
            Arc::new(Host),
        )
        .unwrap();
        let mut summary = artifact_support::summary(Status::Complete);
        let bundle = spec.bundle(i).unwrap();
        summary.input = bundle.id();
        let mut work = Work::new(u64::MAX);
        let end = records::Writer::new(&directory, account.meter())
            .unwrap()
            .finish(summary, &mut work)
            .unwrap();
        assert_eq!(end.spooled_part, 4096);
        assert!(
            std::fs::metadata(directory.path().join("records.ndjson"))
                .unwrap()
                .len()
                >= 256
        );
        assert_eq!(work.spooled_part(), end.spooled_part);
        spooled += end.spooled_part;
        directory.rename(scratch.join(format!("done/{i}"))).unwrap();
        let mut reader = records::read::Reader::new(
            std::fs::File::open(directory.path().join("records.ndjson")).unwrap(),
            bundle.id(),
            false,
            u64::try_from(spec.archives.len()).unwrap(),
            account.meter(),
        )
        .unwrap();
        assert!(matches!(
            reader.next(&|| false).unwrap(),
            Some(records::read::Row::Summary(s)) if s.spooled_part == end.spooled_part
        ));
        assert!(reader.next(&|| false).unwrap().is_none());
    }
    spooled
}

fn archive_allowance(spec: &unityrs_batch::spec::model::Spec, block: u64) -> u64 {
    spec.archives
        .iter()
        .map(|archive| {
            rounded(
                u64::try_from(serde_json::to_string(&archive.key).unwrap().len() - 2).unwrap()
                    + 512,
                block,
            )
            .unwrap()
        })
        .sum()
}

fn measure(archives: u32, inputs: u32, keys: empty_archives::Keys, block: u64, capped: bool) {
    let scratch = Scratch::new("phase6-ledger");
    let validated = specification(archives, inputs, keys);
    // Only the mandatory final row file is materialized here. The independent
    // encoder intentionally does not share any future production row helper.
    let rows: Vec<_> = validated
        .spec
        .archives
        .iter()
        .map(|a| empty_archives::row(&a.key))
        .collect();
    let row_bytes: u64 = rows.iter().map(|r| u64::try_from(r.len()).unwrap()).sum();
    let row_claim = rounded(row_bytes, block).unwrap();
    let k_blk = if block < 4096 && 4096 % block != 0 {
        2
    } else {
        block.div_ceil(4096).max(1)
    };
    let s = u64::from(inputs) * 4096 * k_blk;
    let committed_claim = u64::from(inputs) * block;
    let a = archive_allowance(&validated.spec, block);
    let allowance = 6 * s + 2 * a;
    let cap = committed_claim + allowance;
    let counter = Counter::new(cap, 0, block).unwrap();
    let account = Account::new(64 << 20);
    let spooled = commit_empty_inputs(&scratch, &counter, &account, &validated.spec);
    assert_eq!(spooled * k_blk, s);
    assert_eq!(counter.used(), committed_claim);
    let mut workspace =
        Workspace::create(scratch.join("w"), counter.clone(), Arc::new(Host)).unwrap();
    let mut writer = workspace.create_file(&|| false).unwrap();
    for row in &rows {
        let result = writer.append(row.as_bytes(), &|| false);
        assert!(
            result.is_ok(),
            "required row refused inside 6S + 2A: {result:?}"
        );
    }
    let file = writer.seal(&|| false).unwrap();
    // Independent bounded metadata encoding: version, original spec index,
    // counts, optional-list tag and expected-length tag. Keep both files live.
    let mut metadata = workspace.create_file(&|| false).unwrap();
    for index in 0..archives {
        metadata.append(&index.to_le_bytes(), &|| false).unwrap();
        metadata.append(&[0; 28], &|| false).unwrap();
    }
    let metadata = metadata.seal(&|| false).unwrap();
    let peak = counter.peak() - committed_claim;
    assert_eq!(workspace.claimed(), row_claim + metadata.claimed());
    assert_eq!(peak, workspace.claimed());
    assert!(
        row_claim <= a,
        "rows={row_claim} A={a} keys={keys:?} b={block}"
    );
    if matches!(
        keys,
        empty_archives::Keys::Quotes | empty_archives::Keys::Backslashes
    ) && block == 512
    {
        assert_eq!(row_claim, 548_352);
    }
    assert!(metadata.claimed() <= a);
    assert!(peak <= allowance);
    let mut actual = Vec::new();
    workspace
        .open(&file, &|| false)
        .unwrap()
        .read_to_end(&mut actual)
        .unwrap();
    assert_eq!(actual, rows.concat().as_bytes());
    for (row, archive) in actual
        .split_inclusive(|&b| b == b'\n')
        .zip(&validated.spec.archives)
    {
        let value: serde_json::Value = serde_json::from_slice(row).unwrap();
        assert_eq!(value["key"], archive.key);
        assert_eq!(value["status"], "empty");
        assert_eq!(value["members"], 0);
        assert_eq!(value["left_out"], 0);
    }
    if row_claim > 6 * s {
        assert!(peak > 6 * s);
    }
    println!(
        "EMPTY_ARCHIVES archives={archives} inputs={inputs} keys={keys:?} block={block} S={s} A={a} allowance={allowance} row_bytes={row_bytes} phase_peak={peak} total_peak={}",
        counter.peak()
    );
    if capped {
        refuse_excess(&mut workspace, &counter, cap);
        assert_eq!(account.snapshot().input.used, 0);
        return;
    }
    workspace.retire(metadata, &|| false).unwrap();
    workspace.retire(file, &|| false).unwrap();
    assert_eq!(counter.used(), committed_claim);
    assert_eq!(account.snapshot().input.used, 0);
}

fn refuse_excess(workspace: &mut Workspace, counter: &Counter, cap: u64) {
    let mut excess = workspace.create_file(&|| false).unwrap();
    loop {
        match excess.append(&[0; 4096], &|| false) {
            Ok(()) => {}
            Err(error) => {
                assert!(matches!(
                    error,
                    unityrs_batch::spool::Error::Full(Full::SizeBytes)
                ));
                let used = counter.used();
                assert!(used <= cap);
                drop(excess);
                assert_eq!(counter.used(), used, "drop retains partial claims");
                return;
            }
        }
    }
}

#[test]
fn empty_archive_family_fits_six_s_plus_two_a() {
    for block in [3072, 4096, 5003, 65_536] {
        for keys in empty_archives::FAMILIES {
            measure(256, 1, keys, block, false);
        }
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "scale: run in release, as CI's `scale` job does"
)]
fn scaled_empty_commits_fit_with_both_spec_streams_retained() {
    for inputs in [8, 64] {
        for block in [3072, 4096, 5003, 65_536] {
            for keys in empty_archives::FAMILIES {
                measure(256, inputs, keys, block, false);
            }
        }
    }
}

#[test]
fn writes_past_the_revised_allowance_still_refuse_and_retain_claims() {
    for block in [3072, 4096, 5003, 65_536] {
        for keys in empty_archives::FAMILIES {
            measure(256, 1, keys, block, true);
        }
    }
}

/// The singleton is the rounding-dominated edge of the written proof. Above
/// this finite block interval every frame fits in one block; below it check
/// every possible integer frame length, including maxima of repaired fields.
#[test]
fn singleton_envelope_includes_headers_catalogs_and_four_retained_streams() {
    const MAX: u64 = 6_277;
    for block in 1..=MAX + 16 {
        let k = if block < 4096 && 4096 % block != 0 {
            2
        } else {
            block.div_ceil(4096).max(1)
        };
        for frame in 1..=MAX {
            let stream = rounded(frame + 16, block).unwrap();
            let p = frame + 16 + block - 1;
            let catalog = rounded(48, block).unwrap();
            // At least 256 encoded summary bytes accompany the candidate.
            let s = k * (4096 + rounded(frame + 256, 4096).unwrap());
            assert!(
                4 * stream + 2 * p + 2 * catalog <= 6 * s,
                "b={block} frame={frame}"
            );
            assert!(5 * stream <= 6 * s, "direct b={block} frame={frame}");
        }
    }
}

#[test]
fn escaped_1024_byte_keys_fit_their_individual_stream_allowance_at_512() {
    for keys in [
        empty_archives::Keys::Quotes,
        empty_archives::Keys::Backslashes,
    ] {
        measure(256, 1, keys, 512, false);
    }
}

/// Exercise the actual complete handoff alongside B6b's independent encoder.
#[test]
fn complete_empty_archive_families_fit_the_encoded_allowance() {
    use unityrs_batch::{
        sort::{NoHooks, Options},
        winners::{self, Context, Prepared},
    };
    for block in [512, 3072, 4096, 5003, 65_536] {
        for keys in empty_archives::FAMILIES {
            let scratch = Scratch::new("complete-archive-gate");
            let spec = specification(256, 1, keys).spec;
            let counter = Counter::new(u64::MAX, 0, block).unwrap();
            let account = Account::new(64 << 20);
            let spooled = commit_empty_inputs(&scratch, &counter, &account, &spec);
            let baseline = counter.used();
            let states = [unityrs_batch::phase1::InputState {
                phase: unityrs_batch::phase1::InputPhase::Committed,
                failure_bundle: None,
                ..Default::default()
            }];
            let done = scratch.join("done");
            let inputs =
                winners::committed::Committed::new(&done, &states, &[false], &Host, &|| false)
                    .unwrap();
            let prepared = Prepared::from_spec(&spec).unwrap();
            let mut workspace =
                Workspace::create(scratch.join("w"), counter.clone(), Arc::new(Host)).unwrap();
            let result = winners::complete(&mut Context {
                spec: &spec,
                prepared: &prepared,
                inputs: &inputs,
                workspace: &mut workspace,
                account: &account,
                options: Options::default(),
                stopped: &|| false,
                hooks: &NoHooks,
                observer: &winners::NoObserver,
            })
            .unwrap();
            let k = if block < 4096 && 4096 % block != 0 {
                2
            } else {
                block.div_ceil(4096).max(1)
            };
            let s = spooled * k;
            let a = archive_allowance(&spec, block);
            let outcome_root = scratch.join("outcomes");
            std::fs::create_dir(&outcome_root).unwrap();
            let _outcomes = unityrs_batch::outcomes::Outcomes::create(
                &outcome_root,
                result.counts.winners,
                u64::try_from(spec.archives.len()).unwrap(),
                &counter,
                &Host,
                &[0; 128],
                &|| false,
            )
            .unwrap();
            let peak = counter.peak() - baseline;
            assert!(result.archives.as_ref().unwrap().claimed() <= a);
            assert!(result.metadata.as_ref().unwrap().claimed() <= a);
            assert!(peak <= 6 * s + 2 * a);
            assert_eq!(account.snapshot().input.used, 0);
            println!(
                "COMPLETE_ARCHIVES keys={keys:?} block={block} S={s} A={a} peak={peak} allowance={}",
                6 * s + 2 * a
            );
        }
    }
}

/// Pin the original measured B6c gate under Addendum 3.
#[test]
fn escaped_keys_fit_the_complete_pipeline_disk_gate() {
    use unityrs_batch::{
        sort::{NoHooks, Options},
        winners::{self, Context, Prepared},
    };
    let scratch = Scratch::new("escaped-archive-gate");
    let mut doc = common::base();
    doc["inputs"] = json!([{"id":"a","path":"/in/empty"}]);
    doc["dependencies"] = json!([]);
    doc["archives"] = (0..256).map(|i| json!({"key":empty_archives::escaped_key(i),"members":[{"kind":"texture","path":"{name}.png"}]})).collect();
    let spec = common::accepted(&doc).spec;
    let counter = Counter::new(u64::MAX, 0, 512).unwrap();
    let account = Account::new(64 << 20);
    let spooled = commit_empty_inputs(&scratch, &counter, &account, &spec);
    let baseline = counter.used();
    let states = [unityrs_batch::phase1::InputState {
        phase: unityrs_batch::phase1::InputPhase::Committed,
        failure_bundle: None,
        ..Default::default()
    }];
    let done = scratch.join("done");
    let inputs =
        winners::committed::Committed::new(&done, &states, &[false], &Host, &|| false).unwrap();
    let prepared = Prepared::from_spec(&spec).unwrap();
    let mut workspace =
        Workspace::create(scratch.join("w"), counter.clone(), Arc::new(Host)).unwrap();
    let result = winners::complete(&mut Context {
        spec: &spec,
        prepared: &prepared,
        inputs: &inputs,
        workspace: &mut workspace,
        account: &account,
        options: Options::default(),
        stopped: &|| false,
        hooks: &NoHooks,
        observer: &winners::NoObserver,
    })
    .unwrap();
    let a: u64 = spec
        .archives
        .iter()
        .map(|a| {
            rounded(
                u64::try_from(empty_archives::row(&a.key).len() - 102).unwrap() + 512,
                512,
            )
            .unwrap()
        })
        .sum();
    let allowance = 6 * spooled + 2 * a;
    let retained = workspace.claimed();
    assert_eq!(retained, 820_224);
    assert_eq!(counter.peak() - baseline, retained);
    assert_eq!(a, 655_360);
    assert_eq!(allowance, 1_335_296);
    assert!(retained <= allowance);
    assert_eq!(result.archives.unwrap().length(), 548_608);
    assert_eq!(result.metadata.unwrap().length(), 271_120);
    assert_eq!(account.snapshot().input.used, 0);
    println!("COMPLETE_ARCHIVE_GATE S={spooled} A={a} retained={retained} allowance={allowance}");
}
