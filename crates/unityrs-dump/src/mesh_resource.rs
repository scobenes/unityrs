//! Exact local resource candidates beside a corpus input. Paths and bytes
//! use fallible charged storage. No per-resource fetcher allocation is kept.
use std::{
    ffi::OsStr,
    os::unix::ffi::OsStrExt,
    path::Path,
    sync::{Arc, Mutex},
};
use unityrs_core::{
    Bytes, Error, FileContext, RawStr,
    env::{CabResolver, Fetch, Source, read_path},
    limits::{Charge, ChargeError, Meter, heap_cost},
};

#[derive(Debug)]
pub(super) struct Directory {
    root: RawStr,
    meter: Arc<Meter>,
    context: Arc<FileContext>,
    failure: Arc<Failure>,
    _charge: Charge,
}
impl Directory {
    pub(super) fn new(
        path: &Path,
        meter: &Arc<Meter>,
        context: &Arc<FileContext>,
    ) -> Result<Self, ChargeError> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        // Two fixed-size Arc owners, once per input, follow the core's
        // existing Arc allocation boundary; all variable storage is fallible.
        let charge = meter.charge(heap_cost((std::mem::size_of::<Self>() + 16) as u64))?;
        let failure_charge =
            meter.charge(heap_cost((std::mem::size_of::<Failure>() + 16) as u64))?;
        Ok(Self {
            root: RawStr::charged(parent.as_os_str().as_bytes(), meter)?,
            meter: meter.clone(),
            context: context.clone(),
            failure: Arc::new(Failure {
                error: Mutex::new(None),
                _charge: failure_charge,
            }),
            _charge: charge,
        })
    }
    fn candidate(&self, name: &[u8]) -> Result<Option<RawStr>, ChargeError> {
        self.meter.count_decode(name.len() as u64)?;
        let path = Path::new(OsStr::from_bytes(name));
        if path.is_file() {
            return RawStr::charged(name, &self.meter).map(Some);
        }
        if path.is_absolute() {
            return Ok(None);
        }
        self.meter
            .count_decode((self.root.as_bytes().len() as u64).saturating_add(name.len() as u64))?;
        let joined = RawStr::charged_concat(&[self.root.as_bytes(), b"/", name], &self.meter)?;
        Ok(Path::new(OsStr::from_bytes(joined.as_bytes()))
            .is_file()
            .then_some(joined))
    }
    fn source(&self, name: &[u8]) -> Result<Option<Source>, Error> {
        let Some(path) = self.candidate(name)? else {
            return Ok(None);
        };
        let data = read_path(
            Path::new(OsStr::from_bytes(path.as_bytes())),
            &self.context.limits,
            &self.meter,
        )?;
        Ok(Some(Source::Bytes { name: path, data }))
    }
}
impl CabResolver for Directory {
    fn resolve(&self, name: &[u8]) -> Option<Source> {
        match self.source(name) {
            Ok(source) => source,
            Err(error) => {
                // A single precharged failure owner crosses Option's error
                // boundary. Every use immediately fails its input, so it is
                // never retained as a successful dependency's mutable source.
                *self
                    .failure
                    .error
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
                Some(Source::Fetch {
                    name: RawStr::default(),
                    fetch: self.failure.clone(),
                })
            }
        }
    }
    fn find_file(&self, name: &[u8]) -> Result<Option<Source>, Error> {
        self.source(name)
    }
}
#[derive(Debug)]
struct Failure {
    error: Mutex<Option<Error>>,
    _charge: Charge,
}
impl Fetch for Failure {
    fn fetch(&self) -> Result<Bytes, Error> {
        match self
            .error
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            Some(error) => Err(error.clone()),
            None => Err(std::io::Error::from(std::io::ErrorKind::Other).into()),
        }
    }
}
