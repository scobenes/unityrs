// Ported from UnityPy 1.25.2 (MIT): UnityPy/files/File.py, UnityPy/files/SerializedFile.py. See THIRD_PARTY_LICENSES.
//! Writable resources (`files/File.py:92-121`, `files/SerializedFile.py:362-387`).
//! Creations join the container's save dictionary. They do not register in
//! Environment.cabs; reloading the saved parent registers its resource readers.
//! Creation does not mark a file changed.

use std::sync::{Arc, Mutex, MutexGuard};

use bytes::Bytes;

use super::save::{SaveError, Writer};
use super::{BundleFile, External, ParentKind, SerializedFile, WebFile};
use crate::limits::{Charge, Meter, MeteredVec, heap_cost};
use crate::raw::RawStr;
use crate::stream::{Endian, ShortArray};

const DEFAULT_NAME: &[u8] = b"CAB-UnityPy_Mod.resS";

/// A shared, appendable resource writer. Clones refer to the same bytes.
#[derive(Clone)]
pub struct WriteableCab(triomphe::Arc<CabState>);
const _: () = crate::send_sync::<WriteableCab>();

struct CabState {
    name: RawStr,
    flags: u32,
    endian: Endian,
    path: Mutex<Option<RawStr>>,
    data: Mutex<Writer>,
    charge: Charge,
}

impl std::fmt::Debug for WriteableCab {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WriteableCab")
            .field("name", self.name())
            .finish_non_exhaustive()
    }
}

impl WriteableCab {
    /// Create an empty writer owned by a caller-managed file mapping.
    /// The caller supplies resource metadata from its current mapping.
    pub fn new(
        name: RawStr,
        flags: u32,
        endian: Endian,
        meter: &Arc<Meter>,
    ) -> Result<Self, SaveError> {
        let size = heap_cost((size_of::<CabState>() + 16) as u64);
        let charge = meter.charge(size)?;
        let state = CabState {
            name,
            flags,
            endian,
            path: Mutex::new(None),
            data: Mutex::new(Writer::new(meter, endian)?),
            charge,
        };
        Ok(Self(triomphe::Arc::try_new(state).map_err(|_| {
            crate::limits::ChargeError::Allocator(meter.allocator_refused(size))
        })?))
    }

    /// The dictionary key; the default is `CAB-UnityPy_Mod.resS`.
    #[must_use]
    pub fn name(&self) -> &RawStr {
        &self.0.name
    }
    /// Directory flags copied from the first existing `.resS`, or zero.
    #[must_use]
    pub fn flags(&self) -> u32 {
        self.0.flags
    }
    /// Endianness copied from that resource, or big endian.
    #[must_use]
    pub fn endian(&self) -> Endian {
        self.0.endian
    }
    /// The archive path installed by the last SerializedFile caller.
    #[must_use]
    pub fn path(&self) -> Option<RawStr> {
        lock(&self.0.path).clone()
    }
    pub(crate) fn validate_name(
        &self,
        layout: super::SaveLayout,
        web: bool,
    ) -> Result<(), SaveError> {
        if layout == super::SaveLayout::Semantic {
            self.0
                .charge
                .meter()
                .count_decode(self.name().len() as u64)?;
            let name = self.name().as_bytes();
            if (!web && name.contains(&0)) || (web && std::str::from_utf8(name).is_err()) {
                return Err(SaveError::Range("CAB name"));
            }
        }
        Ok(())
    }
    /// Append bytes, returning their starting offset. A refusal leaves bytes intact.
    pub fn write_bytes(&self, bytes: &[u8]) -> Result<u64, SaveError> {
        let mut data = lock(&self.0.data);
        let at = super::save::narrow(data.len(), "CAB offset")?;
        data.bytes(bytes)?;
        Ok(at)
    }
    /// Write at a caller-selected cursor, retaining the tail and zero-filling
    /// any gap. A refused growth or work charge leaves the bytes unchanged.
    pub fn write_at(&self, offset: u64, bytes: &[u8]) -> Result<(), SaveError> {
        let offset = super::save::narrow(offset, "CAB offset")?;
        lock(&self.0.data).write_at(offset, bytes)
    }
    /// Current payload length without materializing a snapshot.
    #[must_use]
    pub fn len(&self) -> usize {
        lock(&self.0.data).len()
    }
    /// Whether the resource contains no bytes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Write zero padding at a selected cursor, charging before growth.
    pub fn write_zeroes_at(&self, offset: u64, count: usize) -> Result<(), SaveError> {
        let offset = super::save::narrow(offset, "CAB offset")?;
        lock(&self.0.data).write_zeroes_at(offset, count)
    }
    /// Write a signed integer using the inherited endian.
    pub fn write_i32(&self, value: i32) -> Result<(), SaveError> {
        lock(&self.0.data).i32(value)
    }
    /// A metered snapshot, retaining its charge for as long as any clone lives.
    pub fn save(&self) -> Result<Bytes, SaveError> {
        let data = lock(&self.0.data);
        let mut out = Writer::new(data.meter(), self.endian())?;
        out.bytes(data.slice())?;
        out.finish()
    }
}

pub(crate) struct WritableCabs(Mutex<MeteredVec<WriteableCab>>);

impl WritableCabs {
    pub(crate) fn new(meter: &Arc<Meter>) -> Self {
        Self(Mutex::new(MeteredVec::new(meter)))
    }
    pub(crate) fn lock(&self) -> MutexGuard<'_, MeteredVec<WriteableCab>> {
        lock(&self.0)
    }

    fn get(
        &self,
        name: Option<&RawStr>,
        files: &[super::FileSlot],
        directory: &[super::DirEntry],
        meter: &Arc<Meter>,
    ) -> Result<WriteableCab, SaveError> {
        let name = name
            .filter(|n| !n.is_empty())
            .cloned()
            .unwrap_or_else(|| RawStr::from_static(DEFAULT_NAME));
        let mut cabs = self.lock();
        for cab in cabs.as_slice() {
            meter.count_decode(name.len() as u64)?;
            if cab.name() == &name {
                return Ok(cab.clone());
            }
        }
        let mut flags = None;
        for slot in files {
            meter.count_decode(slot.name.len() as u64 + name.len() as u64)?;
            if slot.name == name {
                return Err(SaveError::CabExists);
            }
            if flags.is_none() && slot.name.as_bytes().ends_with(b".resS") {
                flags = Some(
                    directory
                        .get(slot.dir_index)
                        .and_then(|r| r.flags)
                        .unwrap_or(0),
                );
            }
        }
        // Earlier writable resources participate in the same dict order.
        if flags.is_none() {
            flags = cabs
                .as_slice()
                .iter()
                .find(|c| c.name().as_bytes().ends_with(b".resS"))
                .map(WriteableCab::flags);
        }
        meter.count_decode(name.len() as u64)?;
        cabs.reserve_exact(1)?;
        let cab = WriteableCab::new(
            RawStr::charged(name.as_bytes(), meter)?,
            flags.unwrap_or(0),
            Endian::Big,
            meter,
        )?;
        cabs.push(cab.clone())?;
        Ok(cab)
    }
}

macro_rules! container {
    ($ty:ty) => {
        impl $ty {
            /// Create or reuse a resource writer. Existing reader keys refuse.
            /// Creation alone neither marks changed nor registers an environment CAB.
            pub fn get_writeable_cab(
                &self,
                name: Option<&RawStr>,
            ) -> Result<WriteableCab, SaveError> {
                self.writable
                    .get(name, self.files(), self.directory(), self.meter())
            }
            /// The writers added after the loaded files, in dictionary order.
            pub fn writeable_cabs(&self) -> Result<MeteredVec<WriteableCab>, SaveError> {
                let cabs = self.writable.lock();
                self.meter().count_decode(cabs.len() as u64)?;
                let mut out = MeteredVec::with_capacity(self.meter(), cabs.len())?;
                for cab in cabs.as_slice() {
                    out.push(cab.clone())?;
                }
                Ok(out)
            }
        }
    };
}
container!(BundleFile);
container!(WebFile);

impl SerializedFile {
    /// Create a resource in the parent bundle/WebFile and append its external.
    /// Top-level `SerializedFile`s have no writable CAB, as in UnityPy.
    pub fn get_writeable_cab(
        &self,
        name: Option<&RawStr>,
    ) -> Result<Option<WriteableCab>, SaveError> {
        self.get_writeable_cab_with_guid(name, guid_suffix())
    }

    /// The same operation with the last sixteen ASCII bytes of a UUID URN
    /// supplied by the caller (the Python binding can pass `uuid.uuid1()`).
    /// This also makes the otherwise nondeterministic GUID replayable.
    pub fn get_writeable_cab_with_guid(
        &self,
        name: Option<&RawStr>,
        guid: [u8; 16],
    ) -> Result<Option<WriteableCab>, SaveError> {
        let Some(parent) = self.parent() else {
            return Ok(None);
        };
        let name = name
            .cloned()
            .unwrap_or_else(|| RawStr::from_static(DEFAULT_NAME));
        self.meter().count_decode(
            10u64
                .saturating_add(self.name().len() as u64)
                .saturating_add(name.len() as u64),
        )?;
        // File.name is a POSIX basename in UnityPy, whereas our stored name
        // also serves as the container dictionary key.
        let basename = self
            .name()
            .as_bytes()
            .rsplit(|&b| b == b'/')
            .next()
            .unwrap_or_default();
        let path = RawStr::charged_concat(
            &[b"archive:/", basename, b"/", name.as_bytes()],
            self.meter(),
        )?;
        self.meter().count_decode(path.len() as u64)?;
        if path.as_bytes().contains(&0) || path.len() > crate::stream::STRING_TO_NULL_WINDOW {
            return Err(SaveError::Range("CAB external path"));
        }
        let mut pending = lock(&self.pending_externals);
        self.meter().count_decode(
            ((self.externals().len() + pending.len()) as u64).saturating_mul(path.len() as u64),
        )?;
        let present = self
            .externals()
            .iter()
            .chain(pending.as_slice())
            .any(|e| e.path == path);
        if !present {
            pending.reserve_exact(1)?;
        }
        let cab = match &parent.kind {
            ParentKind::Bundle { bundle, .. } => {
                bundle.upgrade().map(|b| b.get_writeable_cab(Some(&name)))
            }
            ParentKind::Web { web } => web.upgrade().map(|w| w.get_writeable_cab(Some(&name))),
        }
        .transpose()?;
        if let Some(cab) = &cab {
            if !present {
                pending.push(External {
                    temp_empty: Some(RawStr::default()),
                    guid: Some(ShortArray::from_slice(&guid)),
                    kind: Some(0),
                    path: path.clone(),
                })?;
            }
            *lock(&cab.0.path) = Some(path);
        }
        Ok(cab)
    }

    /// Pending metadata additions. Loaded metadata remains the immutable
    /// `externals()` view, as original object bytes do after `set_raw_data`.
    pub fn writeable_externals(&self) -> Result<MeteredVec<External>, SaveError> {
        let pending = lock(&self.pending_externals);
        self.meter().count_decode(pending.len() as u64)?;
        let mut out = MeteredVec::with_capacity(self.meter(), pending.len())?;
        for external in pending.as_slice() {
            out.push(external.clone())?;
        }
        Ok(out)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn guid_suffix() -> [u8; 16] {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NODE: AtomicU64 = AtomicU64::new(0);
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let node = NODE.load(Ordering::Relaxed);
    let node = if node == 0 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u8(1);
        let proposed = h.finish() | (1 << 40);
        match NODE.compare_exchange(0, proposed, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => proposed,
            Err(current) => current,
        }
    } else {
        node
    };
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(SEQUENCE.fetch_add(1, Ordering::Relaxed));
    let value = (h.finish() & 0xfff) << 48 | node & 0xffff_ffff_ffff;
    let mut out = [b'0'; 16];
    out[3] = b'-';
    let mut shift = 56u32;
    for (i, b) in out.iter_mut().enumerate() {
        if i == 3 {
            continue;
        }
        *b = b"0123456789abcdef"
            .get(((value >> shift) & 15) as usize)
            .copied()
            .unwrap_or(b'0');
        shift = shift.saturating_sub(4);
    }
    out
}
