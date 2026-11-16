// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/File.py, UnityPy/files/ObjectReader.py. See THIRD_PARTY_LICENSES.
//! `files/File.py:145-148`, `files/ObjectReader.py:181-184`: replacements
//! belong to save, while reads continue using the original reader.
use super::SerializedFile;
use crate::files::save::{SaveError, Writer};
use crate::stream::Endian;
use std::sync::atomic::Ordering;

impl SerializedFile {
    /// Whether explicitly marked or an object replacement has been installed.
    #[must_use]
    pub fn is_changed(&self) -> bool {
        self.changed.load(Ordering::Acquire)
    }

    /// Mark this file and all live parent containers changed.
    pub fn mark_changed(&self) {
        if let Some(parent) = &self.parent {
            parent.mark_changed();
        }
        self.changed.store(true, Ordering::Release);
    }

    pub(crate) fn clear_raw_data(&self, index: u32) {
        let mut edits = self
            .edits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Ok(at) = edits.as_slice().binary_search_by_key(&index, |(i, _)| *i)
            && let Some(tail) = edits.as_mut_slice().get_mut(at..)
        {
            tail.rotate_left(1);
            edits.pop();
        }
        drop(edits);
        self.mark_changed();
    }

    pub(crate) fn set_raw_data(&self, index: u32, data: &[u8]) -> Result<(), SaveError> {
        if self.object_infos.get(index as usize).is_none() {
            return Err(SaveError::Missing("object row"));
        }
        let _: u32 = crate::files::save::narrow(data.len(), "byte_size")?;
        let mut out = Writer::new(self.meter(), Endian::Big)?;
        out.bytes(data)?;
        let data = out.finish()?;
        let mut edits = self
            .edits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match edits.as_slice().binary_search_by_key(&index, |(i, _)| *i) {
            Ok(at) => {
                if let Some((_, old)) = edits.as_mut_slice().get_mut(at) {
                    *old = data;
                }
            }
            Err(at) => {
                edits.push((index, data))?;
                if let Some(tail) = edits.as_mut_slice().get_mut(at..) {
                    tail.rotate_right(1);
                }
            }
        }
        drop(edits);
        self.mark_changed();
        Ok(())
    }
}
