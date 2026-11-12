//! Phase 1's time where it is slowest (review round 2's P1, round 3's
//! G4, round 4's W1-W3), with its peak `p` (`measure.rs`), on the
//! test's thread. Ignored: run it with
//!
//! ```text
//! cargo test --release -p unityrs-batch --test slowest -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Each prints a `ROW` line per shape, and fails when one peaks at 512 MiB
//! or more.

mod measuring;

use std::sync::atomic::Ordering;

use measuring::{HEAD, LIVE, fill, measure_at, row, short_id};
use unityrs_batch::spec::MAX_SPEC_BYTES;
use unityrs_batch::spec::fields::VIEW_WORK;
use unityrs_testkit::counted::{count_this_thread, others_are_not_counted};
use unityrs_testkit::scratch::Scratch;

/// A host of four `xn--` labels, 243 bytes, each decoding to distinct CJK
/// characters: the most punycode a host within its 253-byte cap holds.
const XN_HOST: &str = "xn--nvq83prc91h0uj3pr2hh7uo4juk2ac3mf74aqtl2u3bwjd3xc29l31mvyi.\
xn--mks6mz9be5v8uq65fdio01bf2qzjbfxw6qa47zlo1awfcm4m06bfum.\
xn--9irv1lh0dzxgdvkbvberz0hfreyrt8oersn6ilftrq2gl0xw38bmo2a.\
xn--r0qv7iqjwyb2ao4s6rof2md9bvx0c0ioi8loppijng5br2iz5qo4tpv9a";

/// A 253-byte host of uppercase letters, which IDNA maps to lowercase.
fn upper_host() -> String {
    let label = "A".repeat(63);
    format!("{label}.{label}.{label}.{}", "A".repeat(61))
}

/// A filler: the spec's rest for a given room.
type Filler = Box<dyn Fn(usize) -> String>;

/// The legal dependencies that fill a spec's rest, each with a unique id
/// (`f` and a short id): the candidates for phase 1's slowest parse.
fn fillers() -> Vec<(&'static str, Filler)> {
    let upper = upper_host();
    let lists = (0..64).map(short_id).collect::<Vec<_>>().join(r#"",""#);
    vec![
        (
            "xn-- hosts",
            Box::new(|i| format!(r#"{{"id":"f/{}","url":"https://{XN_HOST}/"}}"#, short_id(i))),
        ),
        (
            "uppercase hosts",
            Box::new(move |i| format!(r#"{{"id":"f/{}","url":"https://{upper}/"}}"#, short_id(i))),
        ),
        (
            "unique path dependencies",
            Box::new(|i| format!(r#"{{"id":"f/{}","path":"/"}}"#, short_id(i))),
        ),
        (
            "deps lists outside every view",
            Box::new(move |i| {
                format!(
                    r#"{{"id":"f/{}","path":"/","deps":["{lists}"]}}"#,
                    short_id(i)
                )
            }),
        ),
    ]
}

/// A spec of `inputs` and `deps`, then `filler(i)` dependencies while it
/// fits 64 MiB.
fn compose(inputs: &[String], deps: &[String], filler: Option<&dyn Fn(usize) -> String>) -> String {
    let mut s = String::from(HEAD);
    s.push_str(r#""inputs":["#);
    s.push_str(&inputs.join(","));
    s.push_str(r#"],"dependencies":["#);
    s.push_str(&deps.join(","));
    if let Some(f) = filler {
        let cap = usize::try_from(MAX_SPEC_BYTES).expect("64 MiB fits");
        let mut i = 0;
        loop {
            let it = f(i);
            if s.len() + it.len() + 3 > cap {
                break;
            }
            if i > 0 || !deps.is_empty() {
                s.push(',');
            }
            s.push_str(&it);
            i += 1;
        }
    }
    s.push_str("]}");
    s
}

/// `n` inputs, each listing `dep`.
fn inputs_of(n: usize, dep: &str) -> Vec<String> {
    (0..n)
        .map(|i| format!(r#"{{"id":"in/{i}","path":"/i","deps":["{dep}"]}}"#))
        .collect()
}

/// A dense set of `set` dependencies, each listing every other.
fn dense_set(set: usize) -> Vec<String> {
    (0..set)
        .map(|d| {
            let ids: Vec<String> = (0..set)
                .filter(|&k| k != d)
                .map(|k| format!(r#""{}""#, short_id(k)))
                .collect();
            format!(
                r#"{{"id":"{}","path":"/d","deps":[{}]}}"#,
                short_id(d),
                ids.join(",")
            )
        })
        .collect()
}

/// The view's members: 65,535, so with its input a view of 65,536.
const MEMBERS: usize = 65_535;

/// A scattered view: `total` dependencies in document order, of which
/// [`MEMBERS`] spread evenly are one view's members, each listing the next
/// (a ring, so each input's view is them all) and `per - 1` others, far
/// apart; the rest list nothing. So each step of the walk reads a list
/// and stamps far from the last (claude-a's scattered index). `source` is
/// each bundle's `"path"` member, or empty for id-only bundles, which fit
/// the most bundles in 64 MiB and are refused (`source`) only after the
/// walk.
fn scattered(total: usize, per: usize, shared: &str, source: &str) -> Vec<String> {
    let stride = total / MEMBERS;
    let m = MEMBERS;
    let short_id = |i: usize| format!("{shared}{}", short_id(i));
    (0..total)
        .map(|pos| {
            let id = short_id(pos);
            if pos % stride != 0 || pos / stride >= m {
                return format!(r#"{{"id":"{id}"{source}}}"#);
            }
            let k0 = pos / stride;
            let mut ids = vec![format!(r#""{}""#, short_id(((k0 + 1) % m) * stride))];
            // Offsets 2..m-1, distinct for each member: 40,503 is prime to
            // m - 2 = 65,533.
            for k in 1..per {
                let off = 2 + (k * 40_503 + k0 * 9_973) % (m - 2);
                ids.push(format!(r#""{}""#, short_id(((k0 + off) % m) * stride)));
            }
            format!(r#"{{"id":"{id}"{source},"deps":[{}]}}"#, ids.join(","))
        })
        .collect()
}

/// Phase 1's time on the slowest shapes (review round 2's P1): the walk at
/// the budget's most on a dense set and on a scattered view of 65,536
/// members among about 2M bundles; the slowest legal fillers alone; and
/// the walk at the budget beside each filler. At 2^30 and at 2^28, and the
/// scattered walk at 2^27 and 2^26 too.
#[test]
#[ignore = "a measurement: run with --release --ignored --nocapture"]
fn phase_1_time_on_the_slowest_shapes() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    let dir = Scratch::new("batch-measure-time");
    let cap = usize::try_from(MAX_SPEC_BYTES).expect("64 MiB fits");
    let budgets = [1u64 << 30, 1 << 28];

    // The fillers alone, under the budget that matters least to them.
    for (name, f) in fillers() {
        // A set of 65 holds the ids the deps-list filler names.
        let text = compose(&inputs_of(1, "0"), &dense_set(65), Some(f.as_ref()));
        let (m, outcome) = measure_at(&dir, "filler", &text, VIEW_WORK);
        row(&format!("{name} alone"), VIEW_WORK, &outcome, &m);
    }

    // Scattered: 2,000,000 dependencies, the members' lists as long as fits
    // or a ring alone (a visit for each id read, the most visits a budget
    // admits); with ids that share their first five bytes too, which the
    // sort of ids reads past.
    for budget in [1u64 << 30, 1 << 28, 1 << 27, 1 << 26] {
        for (shared, ring) in [("", false), ("ppppp", false), ("", true), ("ppppp", true)] {
            let total = 2_000_000;
            let mut per = if ring { 1 } else { 64 };
            let text = loop {
                let work = (MEMBERS * per + 1) as u64;
                let n = usize::try_from(budget / work).expect("fits");
                let first = format!("{shared}{}", short_id(0));
                let text = compose(
                    &inputs_of(n, &first),
                    &scattered(total, per, shared, r#","path":"/""#),
                    None,
                );
                if text.len() <= cap {
                    break text;
                }
                per -= 1;
            };
            let (m, outcome) = measure_at(&dir, "scattered", &text, budget);
            drop(text);
            let ids = if shared.is_empty() {
                ""
            } else {
                ", ids sharing five bytes"
            };
            row(
                &format!("scattered walk, {MEMBERS} members of {per} ids among {total}{ids}"),
                budget,
                &outcome,
                &m,
            );
        }
    }

    for budget in budgets {
        // Dense: the largest set whose lists fit beside the inputs the
        // budget then admits.
        let mut set = 4000;
        let text = loop {
            let per = (set * (set - 1) + 1) as u64;
            let n = usize::try_from(budget / per).expect("fits");
            let text = compose(&inputs_of(n, &short_id(0)), &dense_set(set), None);
            if text.len() <= cap {
                break text;
            }
            set -= 50;
        };
        let (m, outcome) = measure_at(&dir, "dense", &text, budget);
        drop(text);
        row(&format!("dense walk, a set of {set}"), budget, &outcome, &m);

        // Combined (claude-b's shape): a dense set of 1,200, whose walk
        // reaches the budget, beside each filler.
        let set = 1200;
        let per = (set * (set - 1) + 1) as u64;
        let n = usize::try_from(budget / per).expect("fits");
        let (inputs, deps) = (inputs_of(n, &short_id(0)), dense_set(set));
        for (name, f) in fillers() {
            let text = compose(&inputs, &deps, Some(f.as_ref()));
            let (m, outcome) = measure_at(&dir, "combined", &text, budget);
            drop(text);
            row(
                &format!("dense walk of a set of {set} beside {name}"),
                budget,
                &outcome,
                &m,
            );
        }
    }
}

/// The scattered walk of [`scattered`] on id-only bundles, as many as fit
/// 64 MiB beside the members' lists of `per` ids and the inputs `budget`
/// admits: the most bundles, so the most stamps and lookups, a walk can
/// have (claude-c's M1).
fn scattered_id_only(budget: u64, per: usize, shared: &str) -> (usize, String) {
    let cap = usize::try_from(MAX_SPEC_BYTES).expect("64 MiB fits");
    let work = (MEMBERS * per + 1) as u64;
    let n = usize::try_from(budget / work).expect("fits");
    let first = format!("{shared}{}", short_id(0));
    let mut total = 5_000_000;
    loop {
        let text = compose(
            &inputs_of(n, &first),
            &scattered(total, per, shared, ""),
            None,
        );
        if text.len() <= cap {
            return (total, text);
        }
        total -= total / 50;
    }
}

/// Phase 1 on the shapes review round 3 found (claude-c's M1 and L3, its
/// unconfirmed pointer): `prefer` entries naming inputs, each looked up in
/// the inputs' index; the scattered walk on id-only bundles at 2^28,
/// 2^27 and 2^26; the bundles' index at its largest; and a 64 MiB unknown member's
/// name of characters a JSON pointer escapes.
#[test]
#[ignore = "a measurement: run with --release --ignored --nocapture"]
fn phase_1_on_round_3_s_shapes() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    let dir = Scratch::new("batch-measure-round-3");
    let cap = usize::try_from(MAX_SPEC_BYTES).expect("64 MiB fits");

    // 100,000 inputs, 1,900,000 id-only dependencies, then `prefer`
    // entries, each naming an input, to the cap.
    let inputs: Vec<String> = (0..100_000)
        .map(|i| format!(r#"{{"id":"in/{i}","path":"/i"}}"#))
        .collect();
    let deps: Vec<String> = (0..1_900_000)
        .map(|i| format!(r#"{{"id":"{}"}}"#, short_id(i)))
        .collect();
    let prefix = format!(
        r#"{HEAD}"inputs":[{}],"dependencies":[{}],"prefer":{{"#,
        inputs.join(","),
        deps.join(",")
    );
    drop((inputs, deps));
    let text = fill(
        &prefix,
        |i| format!(r#""{}":{{"input":"in/{}"}}"#, short_id(i), i % 100_000),
        "}}",
    );
    let entries = text.matches(r#"{"input""#).count();
    let (m, outcome) = measure_at(&dir, "prefer", &text, VIEW_WORK);
    drop(text);
    row(
        &format!("{entries} prefer entries naming 100,000 inputs, 1,900,000 id-only dependencies"),
        VIEW_WORK,
        &outcome,
        &m,
    );

    for budget in [1u64 << 28, 1 << 27, 1 << 26] {
        for shared in ["", "ppppp"] {
            for per in [1, 8, 37] {
                let (total, text) = scattered_id_only(budget, per, shared);
                let (m, outcome) = measure_at(&dir, "scattered", &text, budget);
                drop(text);
                let ids = if shared.is_empty() {
                    ""
                } else {
                    ", ids sharing five bytes"
                };
                row(
                    &format!(
                        "scattered walk, {MEMBERS} members of {per} ids among {total} id-only bundles{ids}"
                    ),
                    budget,
                    &outcome,
                    &m,
                );
            }
        }
    }

    // The bundles' index at its largest: id-only unique dependencies, the
    // input listing one, so `deps` builds the index over them all.
    let text = fill(
        &format!(r#"{HEAD}"inputs":[{{"id":"_","path":"/i","deps":["0"]}}],"dependencies":["#),
        |i| format!(r#"{{"id":"{}"}}"#, short_id(i)),
        "]}",
    );
    let (m, outcome) = measure_at(&dir, "index", &text, VIEW_WORK);
    drop(text);
    row(
        "id-only unique dependencies, one listed: the index at its largest",
        VIEW_WORK,
        &outcome,
        &m,
    );

    // A 64 MiB unknown member's name of `/`, then of `~`, each of which
    // its pointer writes as two bytes.
    for c in ['/', '~'] {
        let one = r#""inputs":[{"id":"_","path":"/i"}],"#;
        let room = cap - HEAD.len() - one.len() - r#""":0}"#.len();
        let text = format!(r#"{HEAD}{one}"{}":0}}"#, c.to_string().repeat(room));
        let (m, outcome) = measure_at(&dir, "key", &text, VIEW_WORK);
        drop(text);
        let outcome: String = outcome.chars().take(24).collect();
        row(
            &format!("a 64 MiB unknown member's name of {c}"),
            VIEW_WORK,
            &format!("{outcome}..."),
            &m,
        );
    }
}

/// Each of `0..m`'s successor in one cycle through them all, in a fixed
/// pseudo-random order (a Fisher-Yates shuffle driven by splitmix64 from
/// `seed`), or in ascending order when `seed` is `None`.
fn cycle(m: usize, seed: Option<u64>) -> Vec<usize> {
    let mut order: Vec<usize> = (0..m).collect();
    if let Some(mut x) = seed {
        let mut next = || {
            x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let z = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            let z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        };
        for i in (1..m).rev() {
            let j = usize::try_from(next() % (i as u64 + 1)).expect("fits");
            order.swap(i, j);
        }
    }
    let mut succ = vec![0; m];
    for t in 0..m {
        succ[order[t]] = order[(t + 1) % m];
    }
    succ
}

/// `count` disjoint rings of [`MEMBERS`] among `total` id-only bundles,
/// and `n` inputs cycling among them (input `i` lists ring `i % count`'s
/// first member), or listing nothing when `walk` is false (the same
/// document without its walk). Ring `j`'s member `k` is the bundle at
/// `(k * count + j) * stride`, `stride` spreading them over all `total`,
/// and lists its successor in [`cycle`] (seeded with `j` when `permuted`),
/// so a permuted walk reads each list and stamps each bundle in an order
/// that neither a stride prefetcher nor a sequential read of the walk's
/// targets helps (review round 4's W1). The rest list nothing.
fn rings(
    total: usize,
    count: usize,
    n: usize,
    (permuted, walk, shared): (bool, bool, &str),
) -> String {
    let stride = total / (count * MEMBERS);
    let succ: Vec<Vec<usize>> = (0..count)
        .map(|j| cycle(MEMBERS, permuted.then_some(j as u64)))
        .collect();
    let id = |pos: usize| format!("{shared}{}", short_id(pos));
    let deps: Vec<String> = (0..total)
        .map(|pos| {
            let q = pos / stride;
            if pos % stride != 0 || q >= count * MEMBERS {
                return format!(r#"{{"id":"{}"}}"#, id(pos));
            }
            let (j, k) = (q % count, q / count);
            let next = (succ[j][k] * count + j) * stride;
            format!(r#"{{"id":"{}","deps":["{}"]}}"#, id(pos), id(next))
        })
        .collect();
    let inputs: Vec<String> = (0..n)
        .map(|i| {
            if walk {
                let first = (i % count) * stride;
                format!(r#"{{"id":"in/{i}","path":"/i","deps":["{}"]}}"#, id(first))
            } else {
                format!(r#"{{"id":"in/{i}","path":"/i"}}"#)
            }
        })
        .collect();
    compose(&inputs, &deps, None)
}

/// The rings [`phase_1_on_round_4_s_walks`] measures: a name, whether the
/// most bundles that fit hold one ring (`Some(1)`), a few (`Some(8)`) or
/// as many as all of them make (`None`), and whether each is permuted.
const RING_SHAPES: [(&str, Option<usize>, bool); 4] = [
    ("one ring, ascending", Some(1), false),
    ("one ring, permuted", Some(1), true),
    (
        "8 permuted rings spread among the most bundles",
        Some(8),
        true,
    ),
    ("disjoint permuted rings over every bundle", None, true),
];

/// Phase 1 on the walks review round 4 asked for (W1), at 2^26 down to
/// 2^23: [`RING_SHAPES`] over id-only bundles, the inputs cycling among
/// the rings. Each prints an `NS` line too: its time less that of the
/// same document whose inputs list nothing, per id read.
#[test]
#[ignore = "a measurement: run with --release --ignored --nocapture"]
fn phase_1_on_round_4_s_walks() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    let dir = Scratch::new("batch-measure-round-4");
    let cap = usize::try_from(MAX_SPEC_BYTES).expect("64 MiB fits");
    let budgets = [1u64 << 26, 1 << 25, 1 << 24, 1 << 23];
    let inputs = |budget: u64| usize::try_from(budget / (MEMBERS as u64 + 1)).expect("fits");

    for shared in ["", "ppppp"] {
        for (shape, few, permuted) in RING_SHAPES {
            // The bundles: as many as fit beside the most inputs measured.
            let n = inputs(budgets[0]);
            let mut total = if few.is_some() { 5_000_000 } else { 2_600_000 };
            let count = loop {
                let count = few.unwrap_or(total / MEMBERS);
                if rings(total, count, n, (permuted, true, shared)).len() <= cap {
                    break count;
                }
                total -= total / 50;
            };
            let without = rings(total, count, n, (permuted, false, shared));
            let (c, _) = measure_at(&dir, "rings", &without, budgets[0]);
            drop(without);
            let ids_note = if shared.is_empty() {
                ""
            } else {
                ", ids sharing five bytes"
            };
            let shape =
                format!("{shape}: {count} of {MEMBERS} among {total} id-only bundles{ids_note}");
            for budget in budgets {
                let n = inputs(budget);
                let ids = n * (MEMBERS + 1);
                let text = rings(total, count, n, (permuted, true, shared));
                let (m, outcome) = measure_at(&dir, "rings", &text, budget);
                drop(text);
                row(&shape, budget, &outcome, &m);
                #[allow(clippy::cast_precision_loss)]
                let ns = (m.secs - c.secs) * 1e9 / ids as f64;
                println!(
                    "NS | {shape} | 2^{} | {n} inputs, {ids} ids | {:.3} | {:.3} | {ns:.1}",
                    budget.ilog2(),
                    m.secs,
                    c.secs
                );
            }
        }
    }
}

/// Phase 1 on the documents review round 4 asked for, at `VIEW_WORK`:
/// `url` bundles of 32 one-character headers (W2), and 64 MiB of digits,
/// and of halfway decimals, under an unknown member (W3).
#[test]
#[ignore = "a measurement: run with --release --ignored --nocapture"]
fn phase_1_on_round_4_s_documents() {
    count_this_thread();
    others_are_not_counted(|| LIVE.load(Ordering::Relaxed));
    let dir = Scratch::new("batch-measure-round-4");
    let cap = usize::try_from(MAX_SPEC_BYTES).expect("64 MiB fits");

    // W2: `url` bundles of 32 one-character headers, distinct ASCII
    // case-insensitively, as many as fit.
    let names: Vec<char> = ('0'..='9').chain('a'..='v').collect();
    let headers: Vec<String> = names.iter().map(|c| format!(r#""{c}":"x""#)).collect();
    let headers = headers.join(",");
    let text = fill(
        &format!(r#"{HEAD}"inputs":[{{"id":"_","path":"/i"}}],"dependencies":["#),
        |i| {
            format!(
                r#"{{"id":"{}","url":"https://e.net/","headers":{{{headers}}}}}"#,
                short_id(i)
            )
        },
        "]}",
    );
    let count = text.matches(r#""headers""#).count();
    let (m, outcome) = measure_at(&dir, "headers", &text, VIEW_WORK);
    drop(text);
    row(
        &format!("{count} url bundles of 32 one-character headers"),
        VIEW_WORK,
        &outcome,
        &m,
    );

    // W3: a 64 MiB digit run under an unknown member, as an integer and as
    // a fraction; then 64 MiB of halfway decimals, the bignum's case.
    let one = r#""inputs":[{"id":"_","path":"/i"}],"#;
    let room = cap - HEAD.len() - one.len() - r#""zz":}"#.len();
    let digits = |lead: &str| {
        let run: String = "1234567890"
            .chars()
            .cycle()
            .take(room - lead.len())
            .collect();
        format!("{lead}{run}")
    };
    for (shape, value) in [
        ("a 64 MiB integer", digits("")),
        ("a 64 MiB fraction", digits("0.")),
    ] {
        let text = format!(r#"{HEAD}{one}"zz":{value}}}"#);
        drop(value);
        let (m, outcome) = measure_at(&dir, "digits", &text, VIEW_WORK);
        drop(text);
        row(
            &format!("{shape} under an unknown member"),
            VIEW_WORK,
            &outcome,
            &m,
        );
    }
    let text = fill(
        &format!(r#"{HEAD}{one}"zz":["#),
        |_| "2.4703282292062327e-324".to_owned(),
        "]}",
    );
    let count = text.matches("e-324").count();
    let (m, outcome) = measure_at(&dir, "halfway", &text, VIEW_WORK);
    drop(text);
    row(
        &format!("{count} halfway decimals under an unknown member"),
        VIEW_WORK,
        &outcome,
        &m,
    );
}
