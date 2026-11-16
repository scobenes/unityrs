// Ported from UnityPy 1.25.2 (MIT): UnityPy/streams/EndianBinaryWriter.py. See THIRD_PARTY_LICENSES.
//! Metered primitives for file and tree writers, following
//! `streams/EndianBinaryWriter.py:41-97`. Every growth charges memory;
//! every byte copied or zeroed charges the reader's cumulative work first.

use crate::budget::DecodeBuf;
use crate::error::{Error, LimitKind};
use crate::limits::{ChargeError, Meter};
use crate::stream::Endian;
use bytes::Bytes;
use std::sync::Arc;

/// File layout and the blob writer's common-string offset-zero rule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SaveLayout {
    /// Reloadable and idempotent: common offset zero and sequential metadata
    /// strings at the reader window boundary use corrected spellings (Tier C).
    /// Emitted GUIDs and hashes must contain exactly 16 bytes.
    #[default]
    Semantic,
    /// UnityPy's layout, including its local spelling of common offset zero.
    /// Tier C old trees and pre-v9 headers still use the corrected layout.
    UnityPy,
}

/// Options for `SerializedFile::save`. Packers belong to the enclosing bundle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SaveOptions {
    /// The layout to emit.
    pub layout: SaveLayout,
}

/// A writer refusal. No assertion, narrowing cast or allocation may panic.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SaveError {
    /// A writable CAB key already names a loaded reader.
    #[error("this CAB already exists and is not a writer")]
    CabExists,
    /// Serialized formats above 26 and bundle formats above 9 are outside
    /// the pinned HEAD writer's supported formats.
    #[error("HEAD-tier writes are not implemented")]
    HeadTierWritesNotImplemented,
    /// A file kind or layout whose writer is outside this implementation.
    #[error("save is not implemented for {0}")]
    Unsupported(&'static str),
    /// A codec without a UnityPy compression function.
    #[error("compression is not implemented for {0:?}")]
    UnsupportedCodec(crate::codec::Codec),
    /// The encoder refused the input or could not finish its stream.
    #[error("{codec:?} compression failed: {detail}")]
    Compress {
        /// The selected compression algorithm.
        codec: crate::codec::Codec,
        /// A fixed diagnostic (no unmetered error allocation).
        detail: &'static str,
    },
    /// The input's meter refused work, or its meter, budget or allocator
    /// refused storage.
    #[error(transparent)]
    Charge(#[from] ChargeError),
    /// An original object's reader refused its byte range.
    #[error(transparent)]
    Read(#[from] Error),
    /// Reserializing the object's typetree failed before installing data.
    #[error(transparent)]
    Object(#[from] crate::walk::WriteError),
    /// A length or integer cannot be represented by the selected format.
    #[error("save field {0} is out of range")]
    Range(&'static str),
    /// A required field is absent (UnityPy's assertion or struct error).
    #[error("save field {0} is missing")]
    Missing(&'static str),
}

pub(crate) fn required<T>(value: Option<T>, name: &'static str) -> Result<T, SaveError> {
    value.ok_or(SaveError::Missing(name))
}

pub(crate) fn narrow<T: TryFrom<U>, U>(value: U, name: &'static str) -> Result<T, SaveError> {
    T::try_from(value).map_err(|_| SaveError::Range(name))
}

pub(crate) struct Writer {
    buf: DecodeBuf,
    endian: Endian,
    sequential_window: bool,
    window_tail: bool,
}

impl Writer {
    pub(crate) fn new(meter: &Arc<Meter>, endian: Endian) -> Result<Self, SaveError> {
        Ok(Self {
            buf: DecodeBuf::counted(meter, u64::MAX, LimitKind::InputMemory)?,
            endian,
            sequential_window: false,
            window_tail: false,
        })
    }
    pub(crate) fn metadata(
        meter: &Arc<Meter>,
        endian: Endian,
        layout: SaveLayout,
    ) -> Result<Self, SaveError> {
        let mut out = Self::new(meter, endian)?;
        out.sequential_window = layout == SaveLayout::Semantic;
        Ok(out)
    }
    pub(crate) fn meter(&self) -> &Arc<Meter> {
        self.buf.meter()
    }
    pub(crate) fn len(&self) -> usize {
        self.buf.len()
    }
    pub(crate) fn bytes(&mut self, b: &[u8]) -> Result<(), SaveError> {
        self.buf.extend_from_slice(b)?;
        if !b.is_empty() {
            self.window_tail = false;
        }
        Ok(())
    }
    pub(crate) fn write_at(&mut self, offset: usize, bytes: &[u8]) -> Result<(), SaveError> {
        self.buf.write_at(offset, bytes)?;
        Ok(())
    }
    pub(crate) fn slice(&self) -> &[u8] {
        self.buf.as_slice()
    }
    pub(crate) fn finish(self) -> Result<Bytes, SaveError> {
        Ok(self.buf.into_bytes()?)
    }
    pub(crate) fn align(&mut self, n: usize) -> Result<(), SaveError> {
        let padding = (n - self.len() % n) % n;
        self.buf.grow_zeroed(padding)?;
        if padding != 0 {
            self.window_tail = false;
        }
        Ok(())
    }
    pub(crate) fn cstr(&mut self, b: &[u8]) -> Result<(), SaveError> {
        // A sequential reader consumes exactly its window when no NUL is
        // found. Appending one at that boundary shifts the following field.
        // Blob string buffers have explicit offsets and retain their NULs.
        if self.sequential_window && b.len() > crate::stream::STRING_TO_NULL_WINDOW {
            return Err(SaveError::Range("metadata string window"));
        }
        self.bytes(b)?;
        if self.sequential_window && b.len() == crate::stream::STRING_TO_NULL_WINDOW {
            self.window_tail = true;
            return Ok(());
        }
        self.bytes(&[0])
    }
    /// A memory reader requires at least one byte beyond an unterminated
    /// full window. Retain a sentinel when that window ends the metadata;
    /// the next save reconstructs it, so it is idempotent even at EOF.
    pub(crate) fn end_metadata(&mut self) -> Result<(), SaveError> {
        if self.window_tail {
            self.bytes(&[0])?;
        }
        Ok(())
    }
    pub(crate) fn fixed16(&mut self, b: &[u8], field: &'static str) -> Result<(), SaveError> {
        // A negative memory-reader position can produce a short GUID/hash
        // and still let subsequent fields parse. Such a record has no valid
        // sequential spelling; layout mode retains UnityPy's short write.
        if self.sequential_window && b.len() != 16 {
            return Err(SaveError::Range(field));
        }
        self.bytes(b)
    }
    pub(crate) fn boolean(&mut self, v: bool) -> Result<(), SaveError> {
        self.bytes(&[u8::from(v)])
    }
    pub(crate) fn count(&mut self, n: usize) -> Result<(), SaveError> {
        self.i32(narrow(n, "count")?)
    }
    pub(crate) fn number<const N: usize>(&mut self, mut little: [u8; N]) -> Result<(), SaveError> {
        if self.endian == Endian::Big {
            little.reverse();
        }
        self.bytes(&little)
    }
    pub(crate) fn i16(&mut self, n: i16) -> Result<(), SaveError> {
        self.number(n.to_le_bytes())
    }
    pub(crate) fn u16(&mut self, n: u16) -> Result<(), SaveError> {
        self.number(n.to_le_bytes())
    }
    pub(crate) fn i32(&mut self, n: i32) -> Result<(), SaveError> {
        self.number(n.to_le_bytes())
    }
    pub(crate) fn u32(&mut self, n: u32) -> Result<(), SaveError> {
        self.number(n.to_le_bytes())
    }
    pub(crate) fn i64(&mut self, n: i64) -> Result<(), SaveError> {
        self.number(n.to_le_bytes())
    }
    pub(crate) fn u64(&mut self, n: u64) -> Result<(), SaveError> {
        self.number(n.to_le_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::{Reservation, Unbounded};

    #[test]
    fn positioned_writes_preserve_tails_and_refusals_preserve_bytes() {
        let meter = Meter::with_decode_work(1 << 20, 2048, Arc::new(Unbounded), Reservation::Input);
        let mut writer = Writer::new(&meter, Endian::Little).unwrap();
        writer.bytes(b"abc").unwrap();
        writer.write_at(0, b"Z").unwrap();
        writer.write_at(5, b"Q").unwrap();
        assert_eq!(writer.slice(), b"Zbc\0\0Q");
        writer.write_at(100, b"").unwrap();
        assert_eq!(writer.slice(), b"Zbc\0\0Q");
        let before = meter.used();
        let work = meter.decoded();
        let held = meter.charge(meter.limit() - before).unwrap();
        let error = writer.write_at(1, &[b'x'; 1024]).unwrap_err();
        assert!(matches!(error, SaveError::Charge(ChargeError::Limit(_))));
        assert_eq!(writer.slice(), b"Zbc\0\0Q");
        assert_eq!(meter.decoded(), work);
        drop(held);
        writer.write_at(1, &[b'x'; 1024]).unwrap();
        assert_eq!(writer.len(), 1025);
        assert_eq!(writer.slice().first(), Some(&b'Z'));
        meter.count_decode(2048 - meter.decoded()).unwrap();
        let error = writer.write_at(0, b"W").unwrap_err();
        assert!(matches!(error, SaveError::Charge(ChargeError::Limit(_))));
        assert_eq!(writer.slice().first(), Some(&b'Z'));
    }

    #[test]
    fn memory_refused_alignment_can_retry_with_its_exact_work_allowance() {
        let meter = Meter::with_decode_work(1 << 20, 16, Arc::new(Unbounded), Reservation::Input);
        let mut writer = Writer::new(&meter, Endian::Little).unwrap();
        writer.bytes(&[7; 8]).unwrap();
        let before = meter.used();
        let held = meter.charge(meter.limit() - before).unwrap();
        let SaveError::Charge(ChargeError::Limit(error)) = writer.align(16).unwrap_err() else {
            panic!("alignment must refuse memory")
        };
        assert_eq!(error.kind(), LimitKind::InputMemory);
        assert_eq!(writer.slice(), &[7; 8]);
        assert_eq!(meter.decoded(), 8, "refused padding writes nothing");
        drop(held);
        assert_eq!(meter.used(), before);
        writer.align(16).unwrap();
        assert_eq!(
            writer.slice(),
            &[7, 7, 7, 7, 7, 7, 7, 7, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(meter.decoded(), 16);
        drop(writer);
        assert_eq!(meter.used(), 0);
    }
}
