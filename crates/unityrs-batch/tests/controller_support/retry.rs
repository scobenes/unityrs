//! Retry census occurrences independently: covering one occurrence of a
//! stack must not hide another occurrence that a child's schedule missed.
use super::{Tally, Target, children, refusals, unreached_site};
use std::collections::BTreeSet;

pub fn cover(
    kind: &str,
    targets: &[Target],
    expected: &[&'static refusals::Exclusion],
    tally: &mut Tally,
) -> (BTreeSet<u64>, usize) {
    cover_with(kind, targets, expected, tally, |targets| {
        children(kind, targets).collect()
    })
}

fn cover_with(
    kind: &str,
    targets: &[Target],
    expected: &[&'static refusals::Exclusion],
    tally: &mut Tally,
    mut attempt: impl FnMut(&[Target]) -> Vec<(Target, std::process::Output)>,
) -> (BTreeSet<u64>, usize) {
    let strict = kind.starts_with("store");
    let mut covered = BTreeSet::new();
    let missed = pending(targets, if strict { 32 } else { 1 }, |targets| {
        let mut missed = Vec::new();
        for (target @ (site, aim), output) in attempt(targets) {
            if strict && unreached_site(kind, site, &output) {
                missed.push(target);
            } else if tally.add(kind, site, &output, expected) {
                covered.extend(aim.map(|(key, _)| key));
            }
        }
        missed
    });
    tally.unreached += missed.len();
    tally.failures += missed.len();
    for (site, aim) in &missed {
        eprintln!("UNREACHED_STORE_TARGET kind={kind} site={site} aim={aim:?}");
    }
    (covered, missed.len())
}

fn pending(
    targets: &[Target],
    rounds: usize,
    mut attempt: impl FnMut(&[Target]) -> Vec<Target>,
) -> Vec<Target> {
    let mut missed = targets.to_vec();
    for _ in 0..rounds {
        missed = attempt(&missed);
        if missed.is_empty() {
            break;
        }
    }
    missed
}

#[test]
fn a_missed_occurrence_is_retried_after_its_stack_was_covered() {
    let first = (1, Some((0xabc, 1)));
    let second = (2, Some((0xabc, 2)));
    let mut visits = Vec::new();
    let mut tally = Tally::default();
    let (covered, missed) = cover_with("store", &[first, second], &[], &mut tally, |targets| {
        visits.push(targets.to_vec());
        targets
            .iter()
            .map(|&target| {
                (
                    target,
                    output(target.0, visits.len() == 1 && target == second),
                )
            })
            .collect()
    });
    assert_eq!(missed, 0);
    assert_eq!(covered, BTreeSet::from([0xabc]));
    assert_eq!((tally.unreached, tally.failures), (0, 0));
    assert_eq!(visits, [vec![first, second], vec![second]]);
}

#[test]
fn an_occurrence_that_never_runs_remains_a_failed_target() {
    let target = (2, Some((0xabc, 2)));
    let mut tally = Tally::default();
    let (covered, missed) = cover_with("store", &[target], &[], &mut tally, |targets| {
        targets
            .iter()
            .map(|&target| (target, output(target.0, true)))
            .collect()
    });
    assert_eq!(covered, BTreeSet::new());
    assert_eq!(missed, 1);
    assert_eq!((tally.unreached, tally.failures), (1, 1));
}

#[cfg(test)]
fn output(site: usize, unreached: bool) -> std::process::Output {
    use std::os::unix::process::ExitStatusExt;
    std::process::Output {
        status: std::process::ExitStatus::from_raw(0),
        stdout: if unreached {
            format!("CONTROLLER_UNREACHED kind=store site={site}\n").into_bytes()
        } else {
            Vec::new()
        },
        stderr: if unreached {
            Vec::new()
        } else {
            format!("REFUSED_ALLOCATION site={site} size=64\n").into_bytes()
        },
    }
}
