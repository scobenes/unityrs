//! One standalone sealed peek. No scheduling, input commits or retained bits.
mod disk;
mod run;
pub mod word;
use crate::{
    budget::Snapshot,
    job::RunFailure,
    spool::{Counter, FileSystem, OwnedFile},
};
pub use disk::{PeekBits, load_bits};
pub use run::run_peek;
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};
use unityrs_core::RawStr;
use unityrs_export::ngui;
use word::{Outcome, Word};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Done,
    Stopped,
    RunFailure,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeekEnd {
    pub bundle: u32,
    pub state: State,
    /// A data-failed peek is Done, with a Failed outcome. A stopped/run-failed
    /// job publishes no outcome in its end, even if its word is already set.
    pub outcome: Option<Outcome>,
    pub failure: Option<RunFailure>,
    pub input_peak: u64,
    pub decode_work: u64,
    pub claimed_bytes: u64,
    pub accounts: Snapshot,
    pub slot_releasable: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Point {
    Open,
    BeforeSeal,
    BeforeWord,
    AfterWord,
}
pub trait Hooks: ngui::Hooks {
    fn point(&self, _point: Point) {}
    fn request_stop(&self, _end: &PeekEnd) {}
}
impl Hooks for ngui::NoHooks {}

pub struct PeekArgs<'a> {
    pub bundle: u32,
    pub inputs: usize,
    pub staged: &'a Path,
    pub name: &'a RawStr,
    pub m: u64,
    pub word: &'a Word,
    /// Controller-retained ownership, initialized to None. A marked peek
    /// creates it exclusively; all-clear and data-failed peeks create nothing.
    pub file: &'a mut Option<OwnedFile>,
    pub directory: &'a Path,
    pub counter: Arc<Counter>,
    pub fs: Arc<dyn FileSystem>,
    pub stop: &'a AtomicBool,
    pub hooks: &'a dyn Hooks,
}
