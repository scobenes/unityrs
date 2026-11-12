//! The winners across inputs (`docs/BATCH_SPEC.md` 4.5, steps 1 to 3;
//! `docs/plans/extract.md` E31, E32, E61): units decided first per unit
//! key, their keys reserved, then every other key among its plain
//! candidates. 4.6's prefix conflicts are the engine's phase 6, after this.
//!
//! [`decide`] holds every candidate in memory. It is the model, not the
//! engine's phase 6: that phase decides the same keys by streaming steps
//! over the committed records, `docs/plans/batch.md` 2.9's external sorts
//! under the phase budget, and its tests compare their decisions with
//! [`decide`]'s on the same candidates.

use std::collections::{BTreeMap, BTreeSet};

/// One input's candidate for a key: an output it made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The input's id.
    pub input: String,
    /// The output's key.
    pub key: String,
    /// Its unit key, or none for a plain candidate.
    pub unit: Option<String>,
    /// The output's MD5.
    pub md5: [u8; 16],
}

/// A `prefer` entry (1.5): an input, an MD5, or both.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Prefer {
    /// The preferred input.
    pub input: Option<String>,
    /// The preferred MD5.
    pub md5: Option<[u8; 16]>,
}

/// A candidate's rule (7.3's `collisions.rule`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Rule {
    /// E31 rule 1.
    PreferInput,
    /// E31 rule 2.
    PreferMd5,
    /// E31 rule 3.
    FirstInput,
    /// A plain candidate for a reserved key.
    Unit,
    /// The winning unit step 2 withheld.
    UnitConflict,
    /// E61.
    Withheld,
}

impl Rule {
    /// Its name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::PreferInput => "prefer_input",
            Self::PreferMd5 => "prefer_md5",
            Self::FirstInput => "first_input",
            Self::Unit => "unit",
            Self::UnitConflict => "unit_conflict",
            Self::Withheld => "withheld",
        }
    }
}

/// One candidate as decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decided {
    /// Its input.
    pub input: String,
    /// Its unit key.
    pub unit: Option<String>,
    /// Its rule.
    pub rule: Rule,
    /// Whether it is the key's row (the winner, or the row of a withheld
    /// key).
    pub winner: bool,
}

/// A key's decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    /// The key.
    pub key: String,
    /// Whether it is published.
    pub publish: bool,
    /// Why it is withheld: `withheld`, `unit_conflict`, `unit_lost` or
    /// `publish_incomplete`.
    pub reason: Option<&'static str>,
    /// Whether rule 3 decided it among candidates of different MD5s.
    pub flip: bool,
    /// The row's input.
    pub winner: String,
    /// Every candidate, by bytewise input.
    pub candidates: Vec<Decided>,
}

/// E31 among `options` (input, MD5), sorted by input: the chosen index and
/// its rule. `eligible` says whether rule 1's input has a candidate.
fn e31(options: &[(&str, [u8; 16])], prefer: Option<&Prefer>) -> (usize, Rule) {
    if let Some(p) = prefer {
        if let Some(want) = &p.input
            && let Some(i) = options.iter().position(|(input, _)| input == want)
        {
            return (i, Rule::PreferInput);
        }
        if let Some(md5) = &p.md5
            && let Some(i) = options.iter().position(|(_, m)| m == md5)
        {
            return (i, Rule::PreferMd5);
        }
    }
    (0, Rule::FirstInput)
}

/// Whether E61 withholds a key: its entry names a failed input.
fn e61(prefer: Option<&Prefer>, failed: &BTreeSet<String>) -> bool {
    prefer
        .and_then(|p| p.input.as_ref())
        .is_some_and(|i| failed.contains(i))
}

/// A unit key's step 1: the winning input, the rule, whether E61 withholds
/// it, and whether step 2 withheld its winner.
struct UnitKey {
    winner: String,
    rule: Rule,
    e61: bool,
    conflict: bool,
}

fn all_same(md5s: &[[u8; 16]]) -> bool {
    md5s.windows(2).all(|w| w.first() == w.get(1))
}

/// Decides every key of `candidates` (each input's outputs) under
/// `prefer`, with `failed` the inputs that failed in the run.
#[must_use]
pub fn decide(
    candidates: &[Candidate],
    prefer: &BTreeMap<String, Prefer>,
    failed: &BTreeSet<String>,
    publish_incomplete: bool,
) -> Vec<Decision> {
    let mut by_key: BTreeMap<&str, Vec<&Candidate>> = BTreeMap::new();
    for c in candidates {
        by_key.entry(c.key.as_str()).or_default().push(c);
    }
    for list in by_key.values_mut() {
        list.sort_by(|a, b| a.input.as_bytes().cmp(b.input.as_bytes()));
    }
    // Each unit's keys, by (unit key, input).
    let mut units: BTreeMap<(&str, &str), BTreeSet<&str>> = BTreeMap::new();
    for c in candidates {
        if let Some(u) = &c.unit {
            units
                .entry((u.as_str(), c.input.as_str()))
                .or_default()
                .insert(c.key.as_str());
        }
    }
    // Step 1.
    let mut decided: BTreeMap<&str, UnitKey> = BTreeMap::new();
    let unit_keys: BTreeSet<&str> = units.keys().map(|(u, _)| *u).collect();
    for &u in &unit_keys {
        let options: Vec<(&str, [u8; 16])> = by_key
            .get(u)
            .map(|list| {
                list.iter()
                    .filter(|c| c.unit.as_deref() == Some(u))
                    .map(|c| (c.input.as_str(), c.md5))
                    .collect()
            })
            .unwrap_or_default();
        let p = prefer.get(u);
        let (i, rule) = e31(&options, p);
        let winner = options
            .get(i)
            .map(|(input, _)| (*input).to_owned())
            .unwrap_or_default();
        decided.insert(
            u,
            UnitKey {
                winner,
                rule,
                e61: e61(p, failed),
                conflict: false,
            },
        );
    }
    let keeper = reserve(&mut decided, &units);
    let unit_rule = |c: &Candidate| -> Rule {
        let Some(u) = c.unit.as_deref() else {
            return Rule::Unit;
        };
        match decided.get(u) {
            Some(d) if d.e61 => Rule::Withheld,
            Some(d) if d.conflict && c.input == d.winner => Rule::UnitConflict,
            Some(d) => d.rule,
            None => Rule::FirstInput,
        }
    };
    let mut out = Vec::new();
    for (&key, list) in &by_key {
        let decision = if let Some(&u) = keeper.get(key) {
            reserved(key, list, u, &decided, &unit_rule)
        } else {
            plain(key, list, prefer.get(key), failed, &unit_rule)
        };
        out.push(decision);
    }
    if !publish_incomplete && !failed.is_empty() {
        for d in &mut out {
            if d.publish {
                d.publish = false;
                d.reason = Some("publish_incomplete");
            }
        }
    }
    out
}

/// Step 2: each key a unit's winner (or, withheld by E61, each of its
/// inputs) claims goes to the least unit key claiming it; a unit losing a
/// claim that E61 does not withhold is marked a conflict. The keeper of each
/// reserved key.
fn reserve<'a>(
    decided: &mut BTreeMap<&'a str, UnitKey>,
    units: &BTreeMap<(&'a str, &'a str), BTreeSet<&'a str>>,
) -> BTreeMap<&'a str, &'a str> {
    let claims = |u: &str, d: &UnitKey| {
        units
            .iter()
            .filter(move |((uk, input), _)| *uk == u && (d.e61 || *input == d.winner))
            .flat_map(|(_, keys)| keys.iter().copied())
            .collect::<Vec<&'a str>>()
    };
    let mut keeper: BTreeMap<&'a str, &'a str> = BTreeMap::new();
    for (&u, d) in decided.iter() {
        for k in claims(u, d) {
            let held = keeper.entry(k).or_insert(u);
            if u.as_bytes() < held.as_bytes() {
                *held = u;
            }
        }
    }
    for (&u, d) in decided.iter_mut() {
        if !d.e61 && claims(u, d).iter().any(|k| keeper.get(k) != Some(&u)) {
            d.conflict = true;
        }
    }
    keeper
}

/// Step 2's row for a key reserved by unit key `u`.
fn reserved(
    key: &str,
    list: &[&Candidate],
    u: &str,
    decided: &BTreeMap<&str, UnitKey>,
    unit_rule: &dyn Fn(&Candidate) -> Rule,
) -> Decision {
    let Some(d) = decided.get(u) else {
        return plain(key, list, None, &BTreeSet::new(), unit_rule);
    };
    let of_u: Vec<&&Candidate> = list
        .iter()
        .filter(|c| c.unit.as_deref() == Some(u))
        .collect();
    let row = of_u
        .iter()
        .find(|c| c.input == d.winner)
        .or_else(|| of_u.first())
        .map(|c| c.input.clone())
        .unwrap_or_default();
    let md5s: Vec<[u8; 16]> = of_u.iter().map(|c| c.md5).collect();
    let flip = d.rule == Rule::FirstInput && !all_same(&md5s);
    let (publish, reason) = if d.e61 {
        (false, Some("withheld"))
    } else if d.conflict {
        (false, Some("unit_conflict"))
    } else {
        (true, None)
    };
    Decision {
        key: key.to_owned(),
        publish,
        reason,
        flip,
        candidates: list
            .iter()
            .map(|c| Decided {
                input: c.input.clone(),
                unit: c.unit.clone(),
                rule: unit_rule(c),
                winner: c.input == row && c.unit.as_deref() == Some(u),
            })
            .collect(),
        winner: row,
    }
}

/// Step 3 for a key no unit reserved.
fn plain(
    key: &str,
    list: &[&Candidate],
    prefer: Option<&Prefer>,
    failed: &BTreeSet<String>,
    unit_rule: &dyn Fn(&Candidate) -> Rule,
) -> Decision {
    let plains: Vec<&&Candidate> = list.iter().filter(|c| c.unit.is_none()).collect();
    if plains.is_empty() {
        let row = list.first().map(|c| c.input.clone()).unwrap_or_default();
        return Decision {
            key: key.to_owned(),
            publish: false,
            reason: Some("unit_lost"),
            flip: false,
            candidates: list
                .iter()
                .map(|c| Decided {
                    input: c.input.clone(),
                    unit: c.unit.clone(),
                    rule: unit_rule(c),
                    winner: c.input == row,
                })
                .collect(),
            winner: row,
        };
    }
    let options: Vec<(&str, [u8; 16])> = plains.iter().map(|c| (c.input.as_str(), c.md5)).collect();
    let (i, rule) = e31(&options, prefer);
    let withheld = e61(prefer, failed);
    let row = options
        .get(i)
        .map(|(input, _)| (*input).to_owned())
        .unwrap_or_default();
    let md5s: Vec<[u8; 16]> = options.iter().map(|(_, m)| *m).collect();
    let plain_rule = if withheld { Rule::Withheld } else { rule };
    Decision {
        key: key.to_owned(),
        publish: !withheld,
        reason: withheld.then_some("withheld"),
        flip: rule == Rule::FirstInput && !all_same(&md5s),
        candidates: list
            .iter()
            .map(|c| Decided {
                input: c.input.clone(),
                unit: c.unit.clone(),
                rule: if c.unit.is_none() {
                    plain_rule
                } else {
                    unit_rule(c)
                },
                winner: c.unit.is_none() && c.input == row,
            })
            .collect(),
        winner: row,
    }
}

#[cfg(test)]
mod tests {
    //! E31's entries the fixture's cases do not reach (review round 1).

    use std::collections::{BTreeMap, BTreeSet};

    use super::{Candidate, Prefer, Rule, decide};

    fn plain(input: &str, md5: u8) -> Candidate {
        Candidate {
            input: input.to_owned(),
            key: "k.png".to_owned(),
            unit: None,
            md5: [md5; 16],
        }
    }

    fn one(prefer: Prefer, failed: &[&str]) -> (String, Rule, bool, bool) {
        let candidates = [plain("a", 1), plain("b", 2)];
        let prefer = BTreeMap::from([("k.png".to_owned(), prefer)]);
        let failed: BTreeSet<String> = failed.iter().map(|f| (*f).to_owned()).collect();
        let d = decide(&candidates, &prefer, &failed, true);
        let [k] = d.as_slice() else {
            panic!("one key");
        };
        let rule = k.candidates.iter().find(|c| c.winner).unwrap().rule;
        (k.winner.clone(), rule, k.publish, k.flip)
    }

    #[test]
    fn an_entrys_input_with_no_candidate_leaves_its_md5_to_decide() {
        let p = Prefer {
            input: Some("z".to_owned()),
            md5: Some([2; 16]),
        };
        assert_eq!(one(p, &[]), ("b".to_owned(), Rule::PreferMd5, true, false));
    }

    #[test]
    fn an_entry_naming_an_input_that_ran_and_made_nothing_is_rule_three() {
        let p = Prefer {
            input: Some("z".to_owned()),
            md5: None,
        };
        // `z` did not fail, so E61 does not withhold the key; it has no
        // candidate, so rule 1 does not choose; rule 3 does, among
        // different MD5s.
        assert_eq!(one(p, &[]), ("a".to_owned(), Rule::FirstInput, true, true));
    }

    #[test]
    fn rule_two_decides_a_plain_key() {
        let p = Prefer {
            input: None,
            md5: Some([2; 16]),
        };
        assert_eq!(one(p, &[]), ("b".to_owned(), Rule::PreferMd5, true, false));
        // An MD5 no candidate has leaves rule 3.
        let p = Prefer {
            input: None,
            md5: Some([9; 16]),
        };
        assert_eq!(one(p, &[]), ("a".to_owned(), Rule::FirstInput, true, true));
    }

    #[test]
    fn an_entry_naming_a_failed_input_withholds_its_key() {
        let p = Prefer {
            input: Some("z".to_owned()),
            md5: Some([2; 16]),
        };
        let (_, rule, publish, _) = one(p, &["z"]);
        assert_eq!((rule, publish), (Rule::Withheld, false));
    }
}
