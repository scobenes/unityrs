#![allow(dead_code)]
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};
use unityrs_batch::{
    peek::{self, PeekArgs, PeekEnd, word::Word},
    spool::{Counter, FileSystem, Host, OwnedFile},
};
use unityrs_core::RawStr;
use unityrs_testkit::{
    bundle::{self, BundleSpec, EntrySpec},
    scratch::Scratch,
};
pub fn bytes(marked: bool) -> Vec<u8> {
    let nested = bundle::build(&BundleSpec::default());
    bundle::build(&BundleSpec {
        entries: (0..11)
            .map(|i| EntrySpec {
                // The marked first row is dropped by later same-key rows. The peek
                // must still inspect it; it keeps no key/shadowing table.
                name: b"same".to_vec(),
                flags: 0,
                data: if marked && [0, 7, 8, 10].contains(&i) {
                    nested.clone()
                } else {
                    b"plain data".to_vec()
                },
            })
            .collect(),
        ..BundleSpec::default()
    })
}
pub struct Harness {
    pub scratch: Scratch,
    pub staged: PathBuf,
    pub directory: PathBuf,
    pub counter: Arc<Counter>,
    pub word: Word,
    pub file: Option<OwnedFile>,
    pub stop: AtomicBool,
    pub fs: Arc<dyn FileSystem>,
}
impl Harness {
    pub fn new(bytes: &[u8]) -> Self {
        let scratch = Scratch::new("sealed-peek");
        let directory = scratch.join("in");
        fs::create_dir(&directory).unwrap();
        let staged = directory.join("i0");
        fs::write(&staged, bytes).unwrap();
        Self {
            scratch,
            staged,
            directory,
            counter: Counter::new(1 << 20, 0, 4096).unwrap(),
            word: Word::default(),
            file: None,
            stop: AtomicBool::new(false),
            fs: Arc::new(Host),
        }
    }
    pub fn run(&mut self, bundle: u32, inputs: usize, m: u64, hooks: &dyn peek::Hooks) -> PeekEnd {
        peek::run_peek(PeekArgs {
            bundle,
            inputs,
            staged: &self.staged,
            name: &RawStr::from_static(b"i0"),
            m,
            word: &self.word,
            file: &mut self.file,
            directory: &self.directory,
            counter: self.counter.clone(),
            fs: self.fs.clone(),
            stop: &self.stop,
            hooks,
        })
    }
}
