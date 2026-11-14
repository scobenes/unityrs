//! Charged memory output and streaming I/O. The fallible byte method keeps
//! allocation/work refusals typed, without boxing an error inside `io::Error`.
use crate::MeshError;
use std::{
    fmt,
    io::{self, Write},
    sync::Arc,
};
use unityrs_core::limits::{Meter, MeteredVec};

/// A sink must charge every input-sized allocation before fallible growth.
/// Implementations run on the caller's thread. Every byte counts as work.
pub trait MeteredWrite: Write + Send + Sync {
    fn meter(&self) -> &Arc<Meter>;
    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), MeshError>;
    /// Stack-only formatting for bounded numbers and punctuation.
    fn write_args(&mut self, args: fmt::Arguments<'_>) -> Result<(), MeshError> {
        struct Adapter<'a, T: MeteredWrite + ?Sized>(&'a mut T, Option<MeshError>);
        impl<T: MeteredWrite + ?Sized> fmt::Write for Adapter<'_, T> {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                self.0.write_bytes(s.as_bytes()).map_err(|e| {
                    self.1 = Some(e);
                    fmt::Error
                })
            }
        }
        let mut adapter = Adapter(self, None);
        let result = fmt::write(&mut adapter, args);
        match adapter.1 {
            Some(e) => Err(e),
            None if result.is_err() => Err(MeshError::Field("format")),
            None => Ok(()),
        }
    }
}
const _: () = crate::send_sync::<dyn MeteredWrite>();
/// Owned output. Moving it keeps the allocation charged.
#[derive(Debug)]
pub struct ChargedBytes(MeteredVec<u8>);
const _: () = crate::send_sync::<ChargedBytes>();
impl ChargedBytes {
    #[must_use]
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self(MeteredVec::new(meter))
    }
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }
}
impl MeteredWrite for ChargedBytes {
    fn meter(&self) -> &Arc<Meter> {
        self.0.charge().meter()
    }
    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), MeshError> {
        self.0.reserve(bytes.len())?;
        self.meter().count_decode(bytes.len() as u64)?;
        self.0.extend_from_slice(bytes)?;
        Ok(())
    }
}
/// Adapter for an `io::Write` that streams to a file or another bounded sink.
/// Any internal buffer of W must itself be charged; File needs none.
pub struct Writer<W> {
    inner: W,
    meter: Arc<Meter>,
}
const _: () = crate::send_sync::<Writer<std::fs::File>>();
impl<W: Write + Send + Sync> Writer<W> {
    pub fn new(inner: W, meter: &Arc<Meter>) -> Self {
        Self {
            inner,
            meter: meter.clone(),
        }
    }
    pub fn finish(mut self) -> Result<W, MeshError> {
        self.inner.flush()?;
        Ok(self.inner)
    }
}
impl<W: Write + Send + Sync> MeteredWrite for Writer<W> {
    fn meter(&self) -> &Arc<Meter> {
        &self.meter
    }
    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), MeshError> {
        self.meter.count_decode(bytes.len() as u64)?;
        self.inner.write_all(bytes)?;
        Ok(())
    }
}
/// A renderer writes one file at a time; no boxed writers or whole-output
/// staging. Names are raw stems and extensions, never joined paths.
pub trait ObjSink: Send + Sync {
    fn file(
        &mut self,
        stem: &[u8],
        extension: &'static str,
        write: &mut dyn FnMut(&mut dyn MeteredWrite) -> Result<(), MeshError>,
    ) -> Result<(), MeshError>;
}
const _: () = crate::send_sync::<dyn ObjSink>();

// io::Write cannot retain a typed ChargeError without a boxed allocation.
// Exporters use write_bytes; direct io callers receive a static error kind.
fn io_error(error: MeshError) -> io::Error {
    match error {
        MeshError::Io(e) => e,
        _ => io::ErrorKind::OutOfMemory.into(),
    }
}
impl Write for ChargedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write_bytes(bytes).map_err(io_error)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl<W: Write + Send + Sync> Write for Writer<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write_bytes(bytes).map_err(io_error)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Python's strict UTF-8 text-file encoding, applied only to OBJ/MTL.
/// Exporters pass complete raw names or valid Rust formatting fragments.
pub(super) struct Utf8<'a>(pub &'a mut dyn MeteredWrite);
impl MeteredWrite for Utf8<'_> {
    fn meter(&self) -> &Arc<Meter> {
        self.0.meter()
    }
    fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), MeshError> {
        self.meter().count_decode(bytes.len() as u64)?;
        std::str::from_utf8(bytes).map_err(|_| MeshError::Field("UTF-8 export text"))?;
        self.0.write_bytes(bytes)
    }
}
impl Write for Utf8<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.write_bytes(bytes).map_err(io_error)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
