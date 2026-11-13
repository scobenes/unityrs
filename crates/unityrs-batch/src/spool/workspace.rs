//! Aggregate ownership with disk-backed sealed descriptors, never an owner
//! vector. Dropping any part retains claims. Only successful unlink releases.
use super::{Counter, Error, FileSystem, OwnedFile};
use std::{
    io::Read,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

static IDENTITIES: AtomicU64 = AtomicU64::new(1);

/// Move-only capability. Its private encoding is used only by disk catalogs.
#[derive(Debug)]
pub struct Sealed {
    owner: u64,
    id: u64,
    length: u64,
    claimed: u64,
}
impl Sealed {
    pub(crate) const BYTES: usize = 32;
    #[must_use]
    pub fn length(&self) -> u64 {
        self.length
    }
    #[must_use]
    pub fn claimed(&self) -> u64 {
        self.claimed
    }
    pub(crate) fn encode(&self) -> [u8; Self::BYTES] {
        let mut out = [0; Self::BYTES];
        for (slot, n) in out.as_chunks_mut::<8>().0.iter_mut().zip([
            self.owner,
            self.id,
            self.length,
            self.claimed,
        ]) {
            slot.copy_from_slice(&n.to_le_bytes());
        }
        out
    }
    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != Self::BYTES {
            return Err(Error::Internal);
        }
        let mut fields = [0; 4];
        for (slot, chunk) in fields.iter_mut().zip(bytes.as_chunks::<8>().0.iter()) {
            *slot = u64::from_le_bytes(*chunk);
        }
        let [owner, id, length, claimed] = fields;
        Ok(Self {
            owner,
            id,
            length,
            claimed,
        })
    }
}

pub struct Workspace {
    path: PathBuf,
    identity: u64,
    next: AtomicU64,
    counter: Arc<Counter>,
    claimed: Arc<AtomicU64>,
    fs: Arc<dyn FileSystem>,
}
impl Workspace {
    pub fn create(
        path: PathBuf,
        counter: Arc<Counter>,
        fs: Arc<dyn FileSystem>,
    ) -> Result<Self, Error> {
        let identity = IDENTITIES
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
            .map_err(|_| Error::Internal)?;
        fs.mkdir(&path)?;
        Ok(Self {
            path,
            identity,
            next: AtomicU64::new(0),
            counter,
            claimed: Arc::new(AtomicU64::new(0)),
            fs,
        })
    }
    #[must_use]
    pub fn claimed(&self) -> u64 {
        self.claimed.load(Ordering::Acquire)
    }
    /// Two generated paths can coexist while opening a list or scratch reader;
    /// boxed file adapters and fixed ownership state fit the additional 8 KiB.
    pub(crate) fn adapter_charge(&self) -> Result<u64, crate::phase::Error> {
        let capacity = self
            .path
            .as_os_str()
            .len()
            .checked_add(22)
            .ok_or_else(crate::phase::Error::internal)?;
        let bytes = u64::try_from(capacity).map_err(|_| crate::phase::Error::internal())?;
        unityrs_core::limits::heap_cost(bytes)
            .checked_mul(2)
            .and_then(|n| n.checked_add(8192))
            .ok_or_else(crate::phase::Error::internal)
    }
    fn path(&self, mut id: u64) -> Result<PathBuf, Error> {
        // Reserve the directory, separator, `f`, and all twenty u64 digits
        // before either push. The sort's fixed overhead charge covers this
        // transient path; the owner holds it only until sealing/retirement.
        let capacity = self
            .path
            .as_os_str()
            .len()
            .checked_add(22)
            .ok_or(Error::Internal)?;
        let mut path = PathBuf::new();
        path.try_reserve_exact(capacity)
            .map_err(|_| Error::HostMemory)?;
        let mut name = [b'0'; 21];
        let mut start = name.len();
        loop {
            start -= 1;
            *name.get_mut(start).ok_or(Error::Internal)? =
                b'0' + u8::try_from(id % 10).map_err(|_| Error::Internal)?;
            id /= 10;
            if id == 0 {
                break;
            }
        }
        start -= 1;
        *name.get_mut(start).ok_or(Error::Internal)? = b'f';
        let name = std::str::from_utf8(name.get(start..).ok_or(Error::Internal)?)
            .map_err(|_| Error::Internal)?;
        path.push(&self.path);
        path.push(name);
        Ok(path)
    }
    pub fn create_file(&mut self, stopped: &dyn Fn() -> bool) -> Result<File, Error> {
        stop(stopped)?;
        let id = self
            .next
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
            .map_err(|_| Error::Internal)?;
        let owned = OwnedFile::create(self.path(id)?, self.counter.clone(), self.fs.clone())?;
        Ok(File {
            owned,
            id,
            owner: self.identity,
            ledger: self.claimed.clone(),
        })
    }
    /// Independent scratch ownership for concurrent archive passes. Numeric
    /// IDs share this workspace's namespace, including across regenerated passes.
    /// No workspace borrow survives creation and no ownership token is cloned.
    pub fn create_independent(&self, stopped: &dyn Fn() -> bool) -> Result<Independent, Error> {
        stop(stopped)?;
        let id = self
            .next
            .try_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
            .map_err(|_| Error::Internal)?;
        let owned = OwnedFile::create(self.path(id)?, self.counter.clone(), self.fs.clone())?;
        Ok(Independent {
            file: File {
                owned,
                id,
                owner: self.identity,
                ledger: self.claimed.clone(),
            },
            fs: self.fs.clone(),
        })
    }
    fn validate(&self, file: &Sealed) -> Result<PathBuf, Error> {
        if file.owner != self.identity
            || file.id >= self.next.load(Ordering::Acquire)
            || crate::work::rounded(file.length, self.counter.block).ok_or(Error::Internal)?
                != file.claimed
        {
            return Err(Error::Internal);
        }
        let path = self.path(file.id)?;
        if self.fs.length(&path).map_err(read_error)? != file.length {
            return Err(Error::Internal);
        }
        Ok(path)
    }
    pub fn open(
        &self,
        file: &Sealed,
        stopped: &dyn Fn() -> bool,
    ) -> Result<Box<dyn Read + Send>, Error> {
        stop(stopped)?;
        self.fs.open(&self.validate(file)?).map_err(read_error)
    }
    /// Caller closes every reader before retirement. A failed delete retains
    /// the entire descriptor's claim, even if the filesystem removed bytes.
    #[allow(clippy::needless_pass_by_value)] // Consumes the deletion capability, including on failure.
    pub fn retire(&mut self, file: Sealed, stopped: &dyn Fn() -> bool) -> Result<(), Error> {
        stop(stopped)?;
        let path = self.validate(&file)?;
        stop(stopped)?;
        OwnedFile::retire(
            path,
            self.counter.clone(),
            self.fs.clone(),
            file.length,
            file.claimed,
        )?;
        self.claimed.fetch_sub(file.claimed, Ordering::AcqRel);
        Ok(())
    }
}
pub struct File {
    owned: OwnedFile,
    id: u64,
    owner: u64,
    ledger: Arc<AtomicU64>,
}
impl File {
    pub fn append(&mut self, bytes: &[u8], stopped: &dyn Fn() -> bool) -> Result<(), Error> {
        let before = self.owned.claimed();
        let update = ClaimUpdate {
            owned: &mut self.owned,
            ledger: &self.ledger,
            before,
        };
        update.owned.append(bytes, stopped)
    }
    pub fn seal(mut self, stopped: &dyn Fn() -> bool) -> Result<Sealed, Error> {
        self.owned.seal(stopped)?;
        let (length, claimed) = self.owned.detach()?;
        Ok(Sealed {
            owner: self.owner,
            id: self.id,
            length,
            claimed,
        })
    }
}
// A filesystem adapter can unwind after claiming. Keep aggregate ownership
// accurate even when append never returns to the blocking task's catch.
struct ClaimUpdate<'a> {
    owned: &'a mut OwnedFile,
    ledger: &'a AtomicU64,
    before: u64,
}
impl Drop for ClaimUpdate<'_> {
    fn drop(&mut self) {
        self.ledger
            .fetch_add(self.owned.claimed() - self.before, Ordering::AcqRel);
    }
}
fn stop(stopped: &dyn Fn() -> bool) -> Result<(), Error> {
    if stopped() {
        Err(Error::Stopped)
    } else {
        Ok(())
    }
}

// Capacity errnos are spool-full only on mutations. Reads and metadata keep
// their host-I/O class, while fallible path/adapter refusals remain memory.
fn read_error(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::OutOfMemory {
        Error::HostMemory
    } else {
        Error::HostIo(error)
    }
}

/// Individually owned central scratch, on the same counter and aggregate
/// ledger as framed workspace files. Drop closes descriptors and retains claims.
pub struct Independent {
    file: File,
    fs: Arc<dyn FileSystem>,
}
impl Independent {
    pub fn append(&mut self, bytes: &[u8], stopped: &dyn Fn() -> bool) -> Result<(), Error> {
        self.file.append(bytes, stopped)
    }
    pub fn seal(&mut self, stopped: &dyn Fn() -> bool) -> Result<(), Error> {
        self.file.owned.seal(stopped)
    }
    #[must_use]
    pub fn length(&self) -> u64 {
        self.file.owned.length()
    }
    pub fn open(&self, stopped: &dyn Fn() -> bool) -> Result<Box<dyn Read + Send>, Error> {
        stop(stopped)?;
        if !self.file.owned.is_sealed()
            || self.fs.length(self.file.owned.path()).map_err(read_error)? != self.length()
        {
            return Err(Error::Internal);
        }
        self.fs.open(self.file.owned.path()).map_err(read_error)
    }
    /// Consume this pass's owner only after all readers have closed. No claim
    /// is released if deletion fails or a stop precedes it.
    pub fn retire(mut self, stopped: &dyn Fn() -> bool) -> Result<(), Error> {
        stop(stopped)?;
        let claimed = self.file.owned.claimed();
        self.file.owned.remove()?;
        self.file.ledger.fetch_sub(claimed, Ordering::AcqRel);
        Ok(())
    }
}
