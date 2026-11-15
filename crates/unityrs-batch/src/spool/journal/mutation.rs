//! Private mutation IO boundary; production has no adapter allocation or state.
use std::fs::File;
use std::io::{self, Read, Seek, Write};
use std::path::Path;

pub(super) trait FileIo: Read + Write + Seek {
    fn length(&self) -> io::Result<u64>;
    fn truncate(&self) -> io::Result<()>;
    fn sync_data(&self) -> io::Result<()>;
}
impl FileIo for File {
    fn length(&self) -> io::Result<u64> {
        self.metadata().map(|metadata| metadata.len())
    }
    fn truncate(&self) -> io::Result<()> {
        self.set_len(0)
    }
    fn sync_data(&self) -> io::Result<()> {
        File::sync_data(self)
    }
}
pub(super) trait JournalIo {
    type File: FileIo;
    fn open(&self, path: &Path, create: bool) -> io::Result<Self::File>;
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    fn sync_directory(&self, path: &Path) -> io::Result<()>;
}
pub(super) struct Host;
impl JournalIo for Host {
    type File = File;
    fn open(&self, path: &Path, create: bool) -> io::Result<File> {
        crate::sys::journal_file(path, create)
    }
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        crate::sys::spool_rename(from, to)
    }
    fn sync_directory(&self, path: &Path) -> io::Result<()> {
        crate::sys::open_directory(path)?.sync_all()
    }
}
