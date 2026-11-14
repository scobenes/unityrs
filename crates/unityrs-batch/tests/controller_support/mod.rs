#![allow(dead_code)]
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex, atomic::AtomicBool},
};
use unityrs_batch::{
    controller::{Controller, Event, Hooks, PreparedSpool},
    execution::Execution,
    phase1::{self, PreparedRun},
    spool::{Counter, Host},
};
use unityrs_testkit::{
    bundle::{self, BundleSpec, EntrySpec, TestCodec},
    scratch::Scratch,
};

pub fn clear(size: usize) -> Vec<u8> {
    bundle::build(&BundleSpec {
        data_codec: TestCodec::None,
        info_codec: TestCodec::None,
        entries: vec![EntrySpec {
            name: b"plain".to_vec(),
            flags: 0,
            data: vec![19; size + 32],
        }],
        ..BundleSpec::default()
    })
}
pub fn marked() -> Vec<u8> {
    bundle::build(&BundleSpec {
        data_codec: TestCodec::None,
        info_codec: TestCodec::None,
        entries: (0..11)
            .map(|i| EntrySpec {
                name: b"same".to_vec(),
                flags: 0,
                data: if [0, 7, 8, 10].contains(&i) {
                    clear(1)
                } else {
                    vec![4; 23]
                },
            })
            .collect(),
        ..BundleSpec::default()
    })
}
#[derive(Default)]
pub struct Observe {
    pub events: Mutex<Vec<Event>>,
}
impl unityrs_export::ngui::Hooks for Observe {}
impl Hooks for Observe {
    fn event(&self, event: Event) {
        self.events.lock().unwrap().push(event);
    }
}

pub struct Harness {
    pub scratch: Scratch,
    pub prepared: Option<PreparedRun>,
    pub spool: PreparedSpool,
    pub control: Arc<AtomicBool>,
    pub execution: Option<Execution>,
}
impl Harness {
    pub fn new(
        inputs: &[(&str, Vec<u8>, Vec<&str>)],
        deps: &[(&str, Vec<u8>, Vec<&str>)],
        threads: u32,
        slots: u32,
    ) -> Self {
        let scratch = Scratch::new("controller");
        for directory in ["sources", "in", "attempts", "fail", "done"] {
            fs::create_dir(scratch.join(directory)).unwrap();
        }
        let bundles: Vec<_> = inputs
            .iter()
            .chain(deps)
            .enumerate()
            .map(|(i, (id, bytes, deps))| {
                let path = scratch.join(format!("sources/{i}"));
                fs::write(&path, bytes).unwrap();
                json!({"id": id, "path": path, "deps": deps})
            })
            .collect();
        let mut validated =
            super::stage_support::from_bundles(&bundles[..inputs.len()], &bundles[inputs.len()..]);
        validated.spec.threads = threads;
        validated.spec.max_decoded_bytes = 32 << 20;
        validated.spec.dep_cache_bytes = 32 << 20;
        validated.spec.max_inflight_uncompressed_bytes = u64::from(slots) * (64 << 20);
        let prepared = phase1::prepare(validated).unwrap();
        let spool = PreparedSpool::new(
            scratch.path().to_path_buf(),
            Counter::new(1 << 30, 0, 4_096).unwrap(),
            Arc::new(Host),
        )
        .unwrap();
        Self {
            scratch,
            prepared: Some(prepared),
            spool,
            control: Arc::new(AtomicBool::new(false)),
            execution: None,
        }
    }
    pub fn configure(&mut self, f: impl FnOnce(&mut unityrs_batch::spec::model::Spec)) {
        f(Arc::get_mut(&mut self.prepared.as_mut().unwrap().spec).unwrap());
        let p = self.prepared.as_mut().unwrap();
        p.extraction = Arc::new(unityrs_batch::attempt::Prepared::from_spec(&p.spec).unwrap());
    }
    pub fn controller(&mut self, hooks: Arc<dyn Hooks>) -> Controller {
        let c = Controller::new(
            self.prepared.take().unwrap(),
            self.spool.clone(),
            self.control.clone(),
            hooks,
        )
        .unwrap();
        self.execution = Some(Execution::new(&c).unwrap());
        c
    }
    pub fn source(&self, index: usize) -> PathBuf {
        self.scratch.join(format!("sources/{index}"))
    }
    pub fn records(&self, input: usize) -> Vec<Value> {
        fs::read_to_string(self.scratch.join(format!("done/{input}/records.ndjson")))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }
}

pub struct Tap {
    pub event: Box<dyn Fn(Event) + Send + Sync>,
    pub point: Box<dyn Fn(unityrs_batch::controller::Point) + Send + Sync>,
}
impl Default for Tap {
    fn default() -> Self {
        Self {
            event: Box::new(|_| {}),
            point: Box::new(|_| {}),
        }
    }
}
impl unityrs_export::ngui::Hooks for Tap {}
impl Hooks for Tap {
    fn event(&self, event: Event) {
        (self.event)(event);
    }
    fn point(&self, point: unityrs_batch::controller::Point) {
        (self.point)(point);
    }
}

pub fn scene_harness(
    scene: &unityrs_testkit::ngui::AtlasScene,
    threads: u32,
    slots: u32,
) -> Harness {
    let parts = |input: bool| {
        scene
            .bundles
            .iter()
            .filter(|b| scene.inputs.contains(&b.id) == input)
            .map(|b| {
                (
                    b.id.as_str(),
                    b.data.clone(),
                    b.deps.iter().map(String::as_str).collect(),
                )
            })
            .collect::<Vec<_>>()
    };
    let mut h = Harness::new(&parts(true), &parts(false), threads, slots);
    h.configure(|s| {
        s.outputs = unityrs_batch::spec::model::Outputs {
            textures: scene.options.texture_key.clone(),
            sprites: scene.options.sprites_key.clone(),
            atlases: Some(unityrs_batch::spec::model::Atlases {
                sprite_key: scene.options.sprite_key.clone(),
                json_key: scene.options.json_key.clone(),
                script_classes: scene.options.script_classes.clone(),
                skip_name_suffixes: scene.options.skip_name_suffixes.clone(),
            }),
            ..s.outputs.clone()
        }
    });
    h
}
