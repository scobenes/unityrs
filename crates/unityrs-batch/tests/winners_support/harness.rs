#![allow(dead_code)]
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::PathBuf,
    sync::Arc,
};
use unityrs_batch::{
    phase::{Account, Error},
    records::{self, Record, Status},
    sort::{NoHooks, Options},
    spool::{
        Counter, Directory, FileSystem, Host,
        workspace::{Sealed, Workspace},
    },
    winners::{
        self, Context, Inputs, Prepared, State,
        stream::{By, Cursor},
    },
    work::Work,
};
use unityrs_export::ngui::{self, Candidate, OutputKind, Prefer, Rule};
use unityrs_testkit::scratch::Scratch;

pub fn candidate(input: &str, key: &str, unit: Option<&str>, md5: u8) -> Candidate {
    Candidate {
        input: input.into(),
        key: key.into(),
        unit: unit.map(str::to_owned),
        md5: [md5; 16],
    }
}
pub struct Commits {
    pub paths: Vec<PathBuf>,
    pub failed: Vec<bool>,
    pub order: Vec<u32>,
}
impl Inputs for Commits {
    fn open(&self, input: u32) -> Result<Box<dyn Read + Send>, Error> {
        Ok(Host.open(&self.paths[usize::try_from(input).unwrap()])?)
    }
    fn failed(&self, input: u32) -> Result<bool, Error> {
        Ok(self.failed[usize::try_from(input).unwrap()])
    }
    fn position(&self, ordinal: u32) -> u32 {
        self.order[usize::try_from(ordinal).unwrap()]
    }
}
pub struct Case {
    pub candidates: Vec<Candidate>,
    pub prefer: BTreeMap<String, Prefer>,
    pub failed: BTreeSet<String>,
    pub inputs: Vec<String>,
    pub archives: Vec<String>,
    pub publish_incomplete: bool,
}
impl Case {
    pub fn new(candidates: Vec<Candidate>) -> Self {
        let inputs = candidates
            .iter()
            .map(|c| c.input.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Self {
            candidates,
            prefer: BTreeMap::new(),
            failed: BTreeSet::new(),
            inputs,
            archives: Vec::new(),
            publish_incomplete: true,
        }
    }
    pub fn spec(&self) -> unityrs_batch::spec::Validated {
        let mut doc = super::common::base();
        let ids = self
            .inputs
            .iter()
            .chain(&self.failed)
            .cloned()
            .collect::<BTreeSet<_>>();
        doc["inputs"] = ids
            .iter()
            .map(|id| json!({"id":id,"path":"/in/synthetic"}))
            .collect();
        doc["dependencies"] = json!([]);
        doc["prefer"] = self
            .prefer
            .iter()
            .map(|(key, p)| {
                let mut value = serde_json::Map::new();
                if let Some(input) = &p.input {
                    value.insert("input".into(), json!(input));
                }
                if let Some(md5) = p.md5 {
                    value.insert("md5".into(), json!(hex(&md5)));
                }
                (key.clone(), serde_json::Value::Object(value))
            })
            .collect::<serde_json::Map<_, _>>()
            .into();
        doc["publish_incomplete"] = json!(self.publish_incomplete);
        doc["archives"] = self
            .archives
            .iter()
            .map(|key| json!({"key":key,"members":[{"kind":"texture","path":"{name}.png"}]}))
            .collect();
        unityrs_batch::spec::parse(serde_json::to_vec(&doc).unwrap()).unwrap()
    }
    pub fn commit(
        &self,
        scratch: &Scratch,
        counter: &Arc<Counter>,
        account: &Account,
        spec: &unityrs_batch::spec::model::Spec,
    ) -> (Commits, u64) {
        let mut paths = Vec::new();
        let mut failures = Vec::new();
        let mut spooled = 0;
        for i in 0..spec.input_count() {
            let id = spec.bundle(i).unwrap().id();
            let failed = self.failed.contains(id);
            let mut directory = Directory::create(
                scratch.join(format!("attempt-{i}")),
                counter.clone(),
                Arc::new(Host),
            )
            .unwrap();
            let mut work = Work::new(u64::MAX);
            let mut summary = super::artifact_support::summary(if failed {
                Status::Failed
            } else {
                Status::Complete
            });
            summary.input = id;
            if failed {
                summary.reason = Some(records::Reason::Text("source_missing".into()));
                spooled += records::write_failure_record(&directory, account.meter(), summary)
                    .unwrap()
                    .spooled_part;
            } else {
                let mut writer = records::Writer::new(&directory, account.meter()).unwrap();
                for (ordinal, c) in self.candidates.iter().filter(|c| c.input == id).enumerate() {
                    let ordinal = u64::try_from(ordinal).unwrap();
                    let mut hashes =
                        records::copy_output(&directory, ordinal, b"x", &mut work).unwrap();
                    hashes.md5 = c.md5;
                    writer
                        .append(
                            Record::Candidate(records::Candidate {
                                key: &c.key,
                                unit: c.unit.as_deref(),
                                input: id,
                                serialized_file: "CAB-synthetic".into(),
                                kind: OutputKind::AtlasJson,
                                path_id: 1,
                                hashes,
                                picture: None,
                                size: 1,
                                plan_index: ordinal,
                                cut: false,
                            }),
                            &mut work,
                        )
                        .unwrap();
                }
                spooled += writer.finish(summary, &mut work).unwrap().spooled_part;
            }
            directory
                .rename(scratch.join(format!("committed-{i}")))
                .unwrap();
            paths.push(directory.path().join("records.ndjson"));
            failures.push(failed);
        }
        let order = (0..paths.len())
            .map(|i| u32::try_from(i).unwrap())
            .collect();
        (
            Commits {
                paths,
                failed: failures,
                order,
            },
            spooled,
        )
    }
    pub fn compare(&self) {
        let expected = ngui::decide(&self.candidates, &self.prefer, &self.failed, true);
        let actual = self.select(Options::default(), false);
        assert_eq!(actual, expected);
    }
    pub fn select(&self, options: Options, prefix: bool) -> Vec<ngui::Decision> {
        let scratch = Scratch::new("winners");
        let validated = self.spec();
        let spec = &validated.spec;
        let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
        let account = Account::new(64 << 20);
        let (inputs, _) = self.commit(&scratch, &counter, &account, spec);
        let prepared = Prepared::from_spec(spec).unwrap();
        let mut workspace = Workspace::create(scratch.join("w"), counter, Arc::new(Host)).unwrap();
        let mut cx = Context {
            spec,
            prepared: &prepared,
            inputs: &inputs,
            workspace: &mut workspace,
            account: &account,
            options,
            stopped: &|| false,
            hooks: &NoHooks,
            observer: &winners::NoObserver,
        };
        let artifacts = if prefix {
            winners::run(&mut cx)
        } else {
            winners::steps123(&mut cx)
        }
        .unwrap();
        let result = self.decisions(&cx, &artifacts);
        assert_eq!(account.snapshot().input.used, 0);
        assert!(account.snapshot().input.peak <= 40 << 20);
        result
    }
    pub fn decisions(
        &self,
        cx: &Context<'_>,
        artifacts: &winners::Artifacts,
    ) -> Vec<ngui::Decision> {
        let units = rows(cx, artifacts.units.as_ref());
        let mut cursor = Cursor::new(artifacts.keys.as_ref(), &By::Key, cx).unwrap();
        let mut result = Vec::new();
        while let Some(row) = cursor.row().unwrap() {
            let mut candidates = self
                .candidates
                .iter()
                .filter(|c| c.key == row.key)
                .collect::<Vec<_>>();
            candidates.sort_by(|a, b| a.input.cmp(&b.input));
            assert_eq!(row.count, u64::try_from(candidates.len()).unwrap());
            let reserved = !row.unit.is_empty() && row.state != State::UnitLost;
            let candidates = candidates
                .into_iter()
                .map(|c| {
                    let rule = if let Some(unit) = &c.unit {
                        let d = units.iter().find(|r| r.key == *unit).unwrap();
                        if d.state == State::Withheld {
                            Rule::Withheld
                        } else if d.state == State::UnitConflict && d.input == c.input {
                            Rule::UnitConflict
                        } else {
                            model_rule(d.rule)
                        }
                    } else if reserved {
                        Rule::Unit
                    } else {
                        model_rule(row.rule)
                    };
                    ngui::winners::Decided {
                        input: c.input.clone(),
                        unit: c.unit.clone(),
                        rule,
                        winner: c.input == row.input
                            && c.unit.as_deref().unwrap_or_default() == row.unit,
                    }
                })
                .collect();
            result.push(ngui::Decision {
                key: row.key.into(),
                publish: row.state == State::Publish,
                reason: row.state.reason(),
                flip: row.flip,
                winner: row.input.into(),
                candidates,
            });
            cursor.advance(cx.stopped).unwrap();
        }
        assert_eq!(artifacts.counts.keys, u64::try_from(result.len()).unwrap());
        assert_eq!(
            artifacts.counts.collisions,
            u64::try_from(result.iter().filter(|d| d.candidates.len() > 1).count()).unwrap()
        );
        assert_eq!(
            artifacts.counts.flips,
            u64::try_from(result.iter().filter(|d| d.flip).count()).unwrap()
        );
        result
    }
}
#[derive(Debug)]
pub struct OwnedRow {
    pub key: String,
    pub unit: String,
    pub input: String,
    pub rule: winners::Rule,
    pub state: State,
    pub flip: bool,
    pub count: u64,
}
pub fn rows(cx: &Context<'_>, file: Option<&Sealed>) -> Vec<OwnedRow> {
    let mut cursor = Cursor::new(file, &By::Key, cx).unwrap();
    let mut result = Vec::new();
    while let Some(r) = cursor.row().unwrap() {
        result.push(OwnedRow {
            key: r.key.into(),
            unit: r.unit.into(),
            input: r.input.into(),
            rule: r.rule,
            state: r.state,
            flip: r.flip,
            count: r.count,
        });
        cursor.advance(cx.stopped).unwrap();
    }
    result
}
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut out, b| {
        write!(&mut out, "{b:02x}").unwrap();
        out
    })
}
pub fn model_rule(rule: winners::Rule) -> Rule {
    match rule {
        winners::Rule::FirstInput => Rule::FirstInput,
        winners::Rule::PreferMd5 => Rule::PreferMd5,
        winners::Rule::PreferInput => Rule::PreferInput,
        winners::Rule::Unit => Rule::Unit,
        winners::Rule::UnitConflict => Rule::UnitConflict,
        winners::Rule::Withheld => Rule::Withheld,
    }
}
