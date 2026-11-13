//! Injectable destination operations, separate from trusted spool paths.
use std::{ffi::CStr, fs::File, io};

pub trait FileSystem: Send + Sync {
    fn release(&self) -> io::Result<(u32, u32)> {
        crate::sys::kernel_release()
    }
    fn probe(&self, root: &File) -> io::Result<()> {
        crate::sys::beneath_probe(root)
    }
    fn mount(&self, file: &File) -> io::Result<Option<u64>> {
        crate::sys::mount_id(file)
    }
    fn open(&self, parent: &File, name: &CStr, flags: i32, beneath: bool) -> io::Result<File> {
        crate::sys::rooted_open(parent, name, flags, beneath)
    }
    fn mkdir(&self, parent: &File, name: &CStr) -> io::Result<()> {
        crate::sys::rooted_mkdir(parent, name)
    }
    fn unlink(&self, parent: &File, name: &CStr) -> io::Result<()> {
        crate::sys::rooted_unlink(parent, name)
    }
    fn rename(&self, parent: &File, from: &CStr, to: &CStr) -> io::Result<()> {
        crate::sys::rooted_rename(parent, from, to)
    }
    fn lock(&self, file: &File) -> io::Result<()> {
        crate::sys::lock_exclusive(file)
    }
    fn write(&self, mut file: &File, bytes: &[u8]) -> io::Result<usize> {
        std::io::Write::write(&mut file, bytes)
    }
    fn read(&self, mut file: &File, bytes: &mut [u8]) -> io::Result<usize> {
        std::io::Read::read(&mut file, bytes)
    }
    fn sync_file(&self, file: &File) -> io::Result<()> {
        file.sync_all()
    }
    fn sync_directory(&self, file: &File) -> io::Result<()> {
        file.sync_all()
    }
    fn writeback(&self, file: &File) -> io::Result<()> {
        crate::sys::writeback(file)
    }
    fn syncfs(&self, lock: &File) -> io::Result<()> {
        crate::sys::sync_filesystem(lock)
    }
}
pub struct Host;
impl FileSystem for Host {}
