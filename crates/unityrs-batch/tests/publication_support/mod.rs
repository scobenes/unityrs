#![allow(dead_code)]
use std::{path::PathBuf, sync::Arc};
use unityrs_batch::{
    phase::Account,
    phase1::{self, AttemptFacts, InputPhase, InputState},
    records::{self, Record, Status, Summary},
    sort::{NoHooks, Options},
    spool::{Counter, Directory, FileSystem, Host, workspace::Workspace},
    winners::{self, Handoff},
    work::Work,
};
use unityrs_testkit::scratch::Scratch;

pub struct Scene {
    pub scratch: Scratch,
    pub prepared: phase1::PreparedRun,
    pub done: PathBuf,
    pub counter: Arc<Counter>,
    pub workspace: Workspace,
    pub handoff: Handoff,
    pub spooled: u64,
}
impl Scene {
    pub fn new(keys: &[&str], archives: bool, block: u64) -> Self {
        Self::with_failure(keys, archives, block, false)
    }
    pub fn with_failure(keys: &[&str], archives: bool, block: u64, failed: bool) -> Self {
        let scratch = Scratch::new("publication");
        let mut spec = crate::common::base();
        spec["run_id"] = serde_json::json!("r");
        if failed {
            spec["publish_incomplete"] = serde_json::json!(false);
        }
        spec["dependencies"] = serde_json::json!([]);
        spec["inputs"] = serde_json::json!([{"id":"z","path":scratch.join("z.bundle")},{"id":"a","path":scratch.join("a.bundle")}]);
        if archives {
            spec["archives"] = serde_json::json!([
            {"key":"z.zip","members":[{"kind":"atlas_json","path":"{input}.json"}]},
            {"key":"a.zip","members":[{"kind":"atlas_json","path":"{input}.json"}]}]);
        }
        let mut prepared = phase1::prepare(
            unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap()).unwrap(),
        )
        .unwrap();
        let counter = Counter::new(u64::MAX, 0, block).unwrap();
        let account = Account::new(64 << 20);
        let done = scratch.join("done");
        std::fs::create_dir(&done).unwrap();
        let mut spooled = 0;
        for index in 0..2 {
            let mut owner = Directory::create(
                scratch.join(format!("attempt-{index}")),
                counter.clone(),
                Arc::new(Host),
            )
            .unwrap();
            let failure = failed && index == 1;
            let summary = if failure {
                records::write_failure_record(
                    &owner,
                    account.meter(),
                    Summary {
                        input: prepared.spec.bundle(index).unwrap().id(),
                        status: Status::Failed,
                        reason: Some(records::Reason::Text("source_missing".into())),
                        detail: None,
                        figures: None,
                        spooled_part: 0,
                        claimed_bytes: 0,
                        cut: false,
                    },
                )
                .unwrap()
            } else {
                records(&owner, &account, &prepared, index, keys, archives)
            };
            spooled += summary.spooled_part;
            owner.rename(done.join(index.to_string())).unwrap();
            prepared.inputs[index] = InputState {
                phase: InputPhase::Committed,
                failure_bundle: failure.then_some(1),
                received: (!failure).then(|| AttemptFacts {
                    state: unityrs_batch::attempt::State::Done,
                    figures: summary.figures.unwrap(),
                    spooled_part: summary.spooled_part,
                    claimed_bytes: summary.claimed_bytes,
                    outputs: if index == 0 { keys.len() as u64 } else { 0 },
                    skips_expected: 1,
                    diagnostics: 1,
                    code: unityrs_batch::reason::Code::Complete,
                }),
                committed: Some(owner.into_claim().unwrap()),
            };
        }
        let inputs =
            winners::committed::Committed::received(&done, &prepared.inputs, &Host, &|| false)
                .unwrap();
        let mut workspace =
            Workspace::create(scratch.join("w"), counter.clone(), Arc::new(Host)).unwrap();
        let handoff = winners::complete(&mut winners::Context {
            spec: &prepared.spec,
            prepared: &prepared.winners,
            inputs: &inputs,
            workspace: &mut workspace,
            account: &account,
            options: Options::default(),
            stopped: &|| false,
            hooks: &NoHooks,
            observer: &winners::NoObserver,
        })
        .unwrap();
        Self {
            scratch,
            prepared,
            done,
            counter,
            workspace,
            handoff,
            spooled,
        }
    }
    pub fn outcomes(&self, name: &str, fs: &dyn FileSystem) -> unityrs_batch::outcomes::Outcomes {
        let root = self.scratch.join(name);
        std::fs::create_dir(&root).unwrap();
        unityrs_batch::outcomes::Outcomes::create(
            &root,
            self.handoff.counts.winners,
            self.prepared.spec.archives.len() as u64,
            &self.counter,
            fs,
            &[0; 128],
            &|| false,
        )
        .unwrap()
    }
}
fn records<'a>(
    owner: &Directory,
    account: &Account,
    prepared: &'a phase1::PreparedRun,
    index: usize,
    keys: &[&str],
    archives: bool,
) -> Summary<'a> {
    let id = prepared.spec.bundle(index).unwrap().id();
    let mut work = Work::new(u64::MAX);
    let mut writer = records::Writer::new(owner, account.meter()).unwrap();
    candidates(
        &mut writer,
        owner,
        prepared,
        index,
        keys,
        archives,
        &mut work,
    );
    writer
        .append(
            Record::Diagnostic(records::Diagnostic {
                kind: records::DiagnosticKind::UndeclaredDependency,
                input: id,
                bundle: None,
                name: records::Text::Raw(&[0xff; 1024]),
                cut: true,
            }),
            &mut work,
        )
        .unwrap();
    writer
        .append(
            Record::Skip(records::Skip {
                input: id,
                serialized_file: records::Text::Raw(&[0xff; 1024]),
                path_id: 3,
                class: "Texture2D",
                output: unityrs_export::ngui::SkipOutput::Texture,
                sprite: None,
                reason: records::Reason::Text("empty_image".into()),
                detail: None,
                cut: true,
            }),
            &mut work,
        )
        .unwrap();
    writer
        .finish(
            Summary {
                input: id,
                status: Status::CompleteWithSkips,
                reason: None,
                detail: None,
                figures: Some(records::Figures {
                    skips: 1,
                    ..records::Figures::default()
                }),
                spooled_part: 0,
                claimed_bytes: 0,
                cut: false,
            },
            &mut work,
        )
        .unwrap()
}
fn candidates(
    writer: &mut records::Writer,
    owner: &Directory,
    prepared: &phase1::PreparedRun,
    index: usize,
    keys: &[&str],
    archives: bool,
    work: &mut Work,
) {
    let id = prepared.spec.bundle(index).unwrap().id();
    if !keys.is_empty() {
        for (ordinal, key) in keys
            .iter()
            .take(if index == 0 { keys.len() } else { 1 })
            .enumerate()
        {
            let ordinal = u64::try_from(ordinal).unwrap();
            let hashes = records::copy_output(owner, ordinal, b"payload", work).unwrap();
            writer
                .append(
                    Record::Candidate(records::Candidate {
                        key,
                        unit: None,
                        input: id,
                        serialized_file: records::Text::Prepared("CAB", false),
                        kind: unityrs_export::ngui::OutputKind::AtlasJson,
                        path_id: 7,
                        hashes,
                        picture: None,
                        size: 7,
                        plan_index: ordinal,
                        cut: false,
                    }),
                    work,
                )
                .unwrap();
            if archives {
                for archive in 0..2 {
                    writer
                        .member(
                            &prepared.spec.archives[archive].key,
                            records::Member {
                                archive: u64::try_from(archive).unwrap(),
                                path: records::Text::Prepared(key, false),
                                key,
                                input: id,
                                md5: &hashes.md5,
                                reason: None,
                                cut: false,
                            },
                            work,
                        )
                        .unwrap();
                }
            }
        }
    }
}
/// No allocating trace in counted children. Exercises the descriptor fallback
/// on every host; native resolver coverage belongs to the Linux gates.
pub struct Portable;
impl unityrs_batch::sink::fs::FileSystem for Portable {
    fn syncfs(&self, _: &std::fs::File) -> std::io::Result<()> {
        Ok(())
    }
    fn release(&self) -> std::io::Result<(u32, u32)> {
        Ok((5, 8))
    }
    fn probe(&self, _: &std::fs::File) -> std::io::Result<()> {
        Err(std::io::Error::from_raw_os_error(libc::ENOSYS))
    }
    fn entry(
        &self,
        parent: &std::fs::File,
        name: &std::ffi::CStr,
    ) -> std::io::Result<unityrs_batch::sys::Entry> {
        let mut entry = unityrs_batch::sys::rooted_entry(parent, name)?;
        entry.mount = self.mount(parent)?;
        Ok(entry)
    }
    fn mount(&self, file: &std::fs::File) -> std::io::Result<Option<u64>> {
        use std::os::unix::fs::MetadataExt;
        Ok(Some(file.metadata()?.dev()))
    }
    fn writeback(&self, _: &std::fs::File) -> std::io::Result<()> {
        Ok(())
    }
}
pub struct OutcomeFaults(pub Arc<std::sync::atomic::AtomicBool>);
struct FailedRandom {
    file: Box<dyn unityrs_batch::spool::random::Random>,
    armed: Arc<std::sync::atomic::AtomicBool>,
}
impl unityrs_batch::spool::random::Random for FailedRandom {
    fn available(&self) -> std::io::Result<u64> {
        self.file.available()
    }
    fn read_at(&self, b: &mut [u8], offset: u64) -> std::io::Result<usize> {
        self.file.read_at(b, offset)
    }
    fn write_at(&self, b: &[u8], offset: u64) -> std::io::Result<usize> {
        if self.armed.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(std::io::Error::from_raw_os_error(libc::ENOSPC));
        }
        self.file.write_at(b, offset)
    }
}
impl FileSystem for OutcomeFaults {
    fn available(&self, path: &std::path::Path) -> std::io::Result<u64> {
        Host.available(path)
    }
    fn create_random(
        &self,
        path: &std::path::Path,
    ) -> std::io::Result<Box<dyn unityrs_batch::spool::random::Random>> {
        Ok(Box::new(FailedRandom {
            file: Host.create_random(path)?,
            armed: self.0.clone(),
        }))
    }
}
