//! Streamed run inventory. A generation owns two catalogs at most, never a
//! `Vec` of descriptors or paths. Catalog bytes use the shared spool counter.
use super::{Control, Point, format};
use crate::{
    phase::{Error, check},
    spool::workspace::{File, Sealed, Workspace},
};
use std::io::Read;

pub(crate) const TAG: u32 = u32::MAX;
pub struct Catalog {
    pub(crate) file: Sealed,
    pub count: u64,
}
pub(crate) struct Writer {
    file: File,
    count: u64,
}
impl Writer {
    pub fn new(workspace: &mut Workspace, control: &Control<'_>) -> Result<Self, Error> {
        let mut file = workspace.create_file(control.stopped)?;
        control.at(Point::BeforeWrite)?;
        file.append(&format::header(TAG), control.stopped)?;
        Ok(Self { file, count: 0 })
    }
    #[allow(clippy::needless_pass_by_value)] // Transfers the move-only capability to disk.
    pub fn push(&mut self, file: Sealed, control: &Control<'_>) -> Result<(), Error> {
        control.at(Point::BeforeWrite)?;
        self.file.append(&file.encode(), control.stopped)?;
        self.count = self.count.checked_add(1).ok_or_else(Error::internal)?;
        Ok(())
    }
    pub fn finish(self, control: &Control<'_>) -> Result<Catalog, Error> {
        Ok(Catalog {
            file: self.file.seal(control.stopped)?,
            count: self.count,
        })
    }
}
pub(crate) struct Reader {
    read: Box<dyn Read + Send>,
    remaining: u64,
}
impl Reader {
    pub fn open(
        workspace: &Workspace,
        catalog: &Catalog,
        control: &Control<'_>,
    ) -> Result<Self, Error> {
        if catalog
            .count
            .checked_mul(32)
            .and_then(|n| n.checked_add(format::HEADER))
            != Some(catalog.file.length())
        {
            return Err(Error::internal());
        }
        let mut read = workspace.open(&catalog.file, control.stopped)?;
        format::check_header(&mut read, TAG, control.stopped)?;
        Ok(Self {
            read,
            remaining: catalog.count,
        })
    }
    pub fn next(&mut self, control: &Control<'_>) -> Result<Option<Sealed>, Error> {
        control.at(Point::BeforeRead)?;
        if self.remaining == 0 {
            return Ok(None);
        }
        let mut bytes = [0; Sealed::BYTES];
        format::read_exact(&mut self.read, &mut bytes, control.stopped)?;
        self.remaining -= 1;
        Ok(Some(Sealed::decode(&bytes)?))
    }
}
pub(crate) fn retire(
    workspace: &mut Workspace,
    file: Sealed,
    control: &Control<'_>,
) -> Result<(), Error> {
    control.at(Point::BeforeRetire)?;
    check(control.stopped)?;
    Ok(workspace.retire(file, control.stopped)?)
}
