//! One checked loader for B4's slice table and B5's compact run words.
use super::word::{Outcome, Word};
use crate::{
    attempt::{classify::internal, hooks::probe},
    spec::model::Spec,
    view::Graph,
};
use std::{fs::OpenOptions, io::Read, os::unix::fs::OpenOptionsExt, path::Path, sync::Arc};
use unityrs_core::limits::Meter;
use unityrs_export::ngui::{self, Bits, Failure, Hooks};

/// Allocate only the current member's bits on the attempt's meter; validate
/// exact disk length, EOF and padding before exposing any bit to registration.
pub fn load_bits(
    rows: u32,
    path: Option<&Path>,
    meter: &Arc<Meter>,
    hooks: &dyn Hooks,
) -> Result<Bits, Failure> {
    if hooks.stopped() {
        return Err(Failure::Stopped);
    }
    let mut bits = probe(hooks, || Bits::charged(rows, meter))?;
    if let Some(path) = path {
        let io = || Failure::Run(ngui::Reason::plain("host_io"));
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY)
            .open(path)
            .map_err(|_| io())?;
        let meta = file.metadata().map_err(|_| io())?;
        if !meta.is_file() || meta.len() != bits.bytes().len() as u64 {
            return Err(internal());
        }
        file.read_exact(bits.bytes_mut()).map_err(|_| io())?;
        let mut extra = [0u8; 1];
        if file.read(&mut extra).map_err(|_| io())? != 0 {
            return Err(internal());
        }
        let used = rows % 8;
        if used != 0 && bits.bytes().last().is_some_and(|b| b >> used != 0) {
            return Err(internal());
        }
    }
    Ok(bits)
}

/// Borrow phase-1 state. IDs resolve through the existing graph permutation;
/// paths are derived on demand, and no decoded bit vector is retained here.
pub struct PeekBits<'a> {
    spec: &'a Spec,
    graph: &'a Graph,
    words: &'a [Word],
    directory: &'a Path,
}
impl<'a> PeekBits<'a> {
    pub fn new(
        spec: &'a Spec,
        graph: &'a Graph,
        words: &'a [Word],
        directory: &'a Path,
    ) -> Result<Self, Failure> {
        if words.len() != graph.len() || words.len() != spec.bundle_count() {
            return Err(internal());
        }
        Ok(Self {
            spec,
            graph,
            words,
            directory,
        })
    }
}
impl ngui::PeekBits for PeekBits<'_> {
    fn load(&self, id: &str, meter: &Arc<Meter>, hooks: &dyn Hooks) -> Result<Bits, Failure> {
        let index = self.graph.find(self.spec, id).ok_or_else(internal)?;
        let word = self
            .words
            .get(index as usize)
            .ok_or_else(internal)?
            .load()
            .map_err(|_| internal())?;
        match word {
            Outcome::Clear(rows) => load_bits(rows, None, meter, hooks),
            Outcome::Disk(rows) => {
                let path =
                    crate::stage::try_path(self.directory, index, self.spec.input_count(), true)
                        .map_err(|error| {
                            crate::attempt::classify::record(crate::records::Error::Spool(error))
                        })?;
                load_bits(rows, Some(&path), meter, hooks)
            }
            Outcome::Pending | Outcome::Failed(_) => Err(internal()),
        }
    }
}
