#![allow(dead_code)]
use serde_json::Value as J;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};
use unityrs_batch::{
    attempt::*,
    spool::{Counter, Directory, Host},
};
use unityrs_core::{
    FileContext, RawStr,
    budget::{Reservation, Unbounded},
    limits::Meter,
};
use unityrs_export::ngui::{self, Options, Template, TemplateKind};
use unityrs_testkit::{
    ngui::{AtlasScene, SceneBundle},
    scratch::Scratch,
};

pub fn options(scene: &AtlasScene) -> Options {
    let o = &scene.options;
    Options {
        sprite_key: Template::parse(&o.sprite_key, TemplateKind::AtlasSprite).unwrap(),
        json_key: Template::parse(&o.json_key, TemplateKind::AtlasJson).unwrap(),
        texture_key: o
            .texture_key
            .as_ref()
            .map(|s| Template::parse(s, TemplateKind::Texture).unwrap()),
        sprites_key: o
            .sprites_key
            .as_ref()
            .map(|s| Template::parse(s, TemplateKind::Sprite).unwrap()),
        script_classes: o
            .script_classes
            .iter()
            .map(|s| s.as_bytes().to_vec())
            .collect(),
        skip_name_suffixes: o
            .skip_name_suffixes
            .iter()
            .map(|s| s.as_bytes().to_vec())
            .collect(),
        atlases: true,
        formats: unityrs_export::ngui::Formats::V1,
    }
}
pub fn limits() -> AttemptLimits {
    AttemptLimits {
        m: 64 << 20,
        d: 64 << 20,
        max_attempt_evictions: 10000,
        max_export_work: 1 << 30,
    }
}
pub struct Harness {
    pub scratch: Scratch,
    pub directory: Directory,
    pub counter: Arc<Counter>,
    pub stop: AtomicBool,
    pub bundles: Vec<SceneBundle>,
    pub names: Vec<RawStr>,
    pub paths: Vec<PathBuf>,
    pub peeks: Vec<(u32, Option<PathBuf>)>,
    pub input: usize,
    pub options: Options,
}
impl Harness {
    pub fn new(scene: &AtlasScene, input: &str) -> Self {
        let scratch = Scratch::new("attempt");
        fs::create_dir(scratch.join("in")).unwrap();
        fs::create_dir(scratch.join("attempts")).unwrap();
        fs::create_dir(scratch.join("fail")).unwrap();
        let mut seen = BTreeSet::new();
        let mut pending = vec![input.to_owned()];
        while let Some(id) = pending.pop() {
            if seen.insert(id.clone()) {
                pending.extend(
                    scene
                        .bundles
                        .iter()
                        .find(|b| b.id == id)
                        .unwrap()
                        .deps
                        .iter()
                        .cloned(),
                );
            }
        }
        let mut bundles: Vec<_> = scene
            .bundles
            .iter()
            .filter(|b| seen.contains(&b.id))
            .cloned()
            .collect();
        bundles.sort_by(|a, b| a.id.cmp(&b.id));
        let input = bundles.iter().position(|b| b.id == input).unwrap();
        let mut paths = Vec::new();
        let mut names = Vec::new();
        let mut peeks = Vec::new();
        for (i, b) in bundles.iter().enumerate() {
            let path = scratch.join(format!("in/{}{i}", if i == input { "i" } else { "d" }));
            fs::write(&path, &b.data).unwrap();
            let mut permissions = fs::metadata(&path).unwrap().permissions();
            permissions.set_readonly(true);
            fs::set_permissions(&path, permissions).unwrap();
            let name = RawStr::from(path.to_str().unwrap());
            let ctx = Arc::new(FileContext::unitypy());
            let meter = Meter::for_input(&ctx.limits, Arc::new(Unbounded), Reservation::Input);
            let bits = ngui::peek(&name, ngui::Data::Path(&path), &ctx, &meter, &ngui::NoHooks);
            // Failure tests also exercise attempts whose caller supplied clear
            // metadata: real dispatch screens failed peeks before this API.
            let (rows, peek) = match bits {
                Ok(bits) => {
                    let file = bits.bytes().iter().any(|b| *b != 0).then(|| {
                        let p = scratch.join(format!("in/p{i}"));
                        fs::write(&p, bits.bytes()).unwrap();
                        p
                    });
                    (bits.rows(), file)
                }
                Err(_) => (0, None),
            };
            paths.push(path);
            names.push(name);
            peeks.push((rows, peek));
        }
        let counter = Counter::new(u64::MAX, 0, 4096).unwrap();
        let directory =
            Directory::create(scratch.join("attempts/0"), counter.clone(), Arc::new(Host)).unwrap();
        Self {
            scratch,
            directory,
            counter,
            stop: AtomicBool::new(false),
            bundles,
            paths,
            names,
            peeks,
            input,
            options: options(scene),
        }
    }
    pub fn run(&self, limits: AttemptLimits, hooks: &dyn Hooks) -> AttemptEnd {
        self.run_members(limits, hooks, &[], &[])
    }
    pub fn run_members(
        &self,
        limits: AttemptLimits,
        hooks: &dyn Hooks,
        templates: &[ngui::MemberTemplate],
        archives: &[&str],
    ) -> AttemptEnd {
        let staged: Vec<_> = self
            .bundles
            .iter()
            .enumerate()
            .map(|(i, b)| StagedBundle {
                id: &b.id,
                name: &self.names[i],
                path: &self.paths[i],
            })
            .collect();
        let members: Vec<_> = staged
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != self.input)
            .map(|(_, s)| *s)
            .collect();
        let entries: Vec<_> = self
            .bundles
            .iter()
            .zip(&self.peeks)
            .map(|(b, (rows, file))| PeekEntry {
                id: &b.id,
                rows: *rows,
                file: file.as_deref(),
            })
            .collect();
        run_attempt(AttemptArgs {
            view: AttemptView {
                input_index: 17,
                input: staged[self.input],
                members: &members,
            },
            options: &self.options,
            prefix: "",
            members: templates,
            archive_keys: archives,
            limits,
            peeks: &PeekTable::new(&entries).unwrap(),
            directory: &self.directory,
            stop: &self.stop,
            hooks,
        })
    }
    pub fn rows(&self) -> Vec<J> {
        rows(self.directory.path())
    }
    pub fn output(&self, row: &J) -> Vec<u8> {
        fs::read(self.directory.path().join(row["file"].as_str().unwrap())).unwrap()
    }
}
pub fn rows(dir: &Path) -> Vec<J> {
    let path = dir.join("records.ndjson");
    if !path.exists() {
        return Vec::new();
    }
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}
pub fn simple(objects: &[unityrs_testkit::ngui::Object]) -> AtlasScene {
    let file = unityrs_testkit::ngui::file(&[], objects, true).unwrap();
    AtlasScene {
        bundles: vec![SceneBundle {
            id: "x".into(),
            data: unityrs_testkit::env::bundle(&[("CAB-x", file)]),
            deps: Vec::new(),
        }],
        inputs: vec!["x".into()],
        options: unityrs_testkit::ngui::CutOptions::textures("{name}.png"),
    }
}
pub fn fixture(name: &str) -> J {
    serde_json::from_str(
        &fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(name)).unwrap(),
    )
    .unwrap()
}
pub fn check_files(scratch: &Scratch, jobs: &[J]) {
    let path = scratch.join("checks.json");
    fs::write(&path, serde_json::to_vec(jobs).unwrap()).unwrap();
    let result = unityrs_testkit::child::output(
        std::process::Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/attempt_support/check.py"))
            .arg(path),
    )
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
pub fn clear(end: &AttemptEnd) {
    assert!(end.accounts.is_clear(), "{end:?}");
    assert!(end.slot_releasable, "{end:?}");
    assert!(std::mem::size_of::<AttemptEnd>() < 4096);
}
