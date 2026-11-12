//! Immutable successful peek metadata supplied by dispatch.
use super::{classify::internal, hooks::probe};
use std::{fs::File, io::Read, path::Path, sync::Arc};
use unityrs_core::limits::Meter;
use unityrs_export::ngui::{Bits, Failure, Hooks, PeekBits};

#[derive(Clone, Copy, Debug)]
pub struct PeekEntry<'a> {
    pub id: &'a str,
    pub rows: u32,
    /// None is the all-clear word; Some is exactly ceil(rows/8) bytes.
    pub file: Option<&'a Path>,
}
#[derive(Clone, Copy, Debug)]
pub struct PeekTable<'a> {
    entries: &'a [PeekEntry<'a>],
}
impl<'a> PeekTable<'a> {
    /// Validate the immutable id index once, outside worker attempts. Entries
    /// are strictly ascending bytewise ids; metadata may cover the whole run.
    pub fn new(entries: &'a [PeekEntry<'a>]) -> Result<Self, Failure> {
        if !entries
            .windows(2)
            .all(|pair| matches!(pair, [a,b] if a.id.as_bytes() < b.id.as_bytes()))
        {
            return Err(internal());
        }
        Ok(Self { entries })
    }
}
impl PeekBits for PeekTable<'_> {
    fn load(&self, id: &str, meter: &Arc<Meter>, hooks: &dyn Hooks) -> Result<Bits, Failure> {
        let index = self
            .entries
            .binary_search_by(|e| e.id.as_bytes().cmp(id.as_bytes()))
            .map_err(|_| internal())?;
        let e = self.entries.get(index).ok_or_else(internal)?;
        let mut bits = probe(hooks, || Bits::charged(e.rows, meter))?;
        if let Some(path) = e.file {
            let io = || Failure::Run(unityrs_export::ngui::Reason::plain("host_io"));
            let mut file = File::open(path).map_err(|_| io())?;
            if file.metadata().map_err(|_| io())?.len() != bits.bytes().len() as u64 {
                return Err(internal());
            }
            file.read_exact(bits.bytes_mut()).map_err(|_| io())?;
            let mut extra = [0u8; 1];
            if file.read(&mut extra).map_err(|_| io())? != 0 {
                return Err(internal());
            }
            let used = e.rows % 8;
            if used != 0 && bits.bytes().last().is_some_and(|b| b >> used != 0) {
                return Err(internal());
            }
        }
        Ok(bits)
    }
}
