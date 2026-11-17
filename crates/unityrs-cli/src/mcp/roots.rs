//! Open each component against its held parent; never canonicalize then reopen.
use std::{
    ffi::{CString, OsStr},
    fs::File,
    io::{self, Read},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::ffi::OsStrExt,
    },
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use unityrs_core::{
    Bytes, Error, LimitKind, RawStr,
    budget::{DecodeBuf, MemoryBudget, Reservation},
    env::{Fetch, Source},
    limits::{Limits, Meter},
    stream::ReaderKind,
};
#[derive(Debug)]
pub struct Roots(Vec<(PathBuf, File)>);
fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "path must be beneath a configured root without symlinks or parent components",
    )
}
fn open(parent: &File, name: &OsStr, directory: bool) -> io::Result<File> {
    let name = CString::new(name.as_bytes()).map_err(|_| denied())?;
    let flags = libc::O_RDONLY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if directory { libc::O_DIRECTORY } else { 0 };
    // SAFETY: parent is held, name is terminated, and success returns a new fd.
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: this is the sole owner of the successful openat result.
    let file = unsafe { File::from_raw_fd(fd) };
    if !directory && !file.metadata()?.is_file() {
        return Err(denied());
    }
    Ok(file)
}
// Match the core loader's terminal split suffix and Unicode decimal digits.
fn split_base(raw: &[u8]) -> Option<&[u8]> {
    let at = raw.windows(6).rposition(|w| w == b".split")?;
    let prefix = raw.get(..at)?;
    if prefix.is_empty() || prefix.last().is_some_and(|b| matches!(b, b'/' | b'\\')) {
        return None;
    }
    if let Some(newline) = prefix.iter().position(|&b| b == b'\n')
        && prefix
            .iter()
            .rposition(|b| matches!(b, b'/' | b'\\'))
            .is_some_and(|sep| newline < sep)
    {
        return None;
    }
    let suffix = std::str::from_utf8(raw.get(at + 6..)?).ok()?;
    (!suffix.is_empty()
        && suffix
            .chars()
            .all(|c| unityrs_core::tables::nd_digit(c).is_some()))
    .then_some(prefix)
}
impl Roots {
    pub fn new(paths: &[PathBuf]) -> io::Result<Self> {
        let mut roots = Vec::new();
        for path in paths {
            let absolute = std::path::absolute(path)?;
            if absolute.components().any(|c| c == Component::ParentDir) {
                return Err(denied());
            }
            let mut held = File::open("/")?;
            for c in absolute.components() {
                if let Component::Normal(name) = c {
                    held = open(&held, name, true)?;
                }
            }
            roots.push((absolute, held));
        }
        Ok(Self(roots))
    }
    pub fn open(&self, path: &Path) -> io::Result<File> {
        if path.components().any(|c| c == Component::ParentDir) {
            return Err(denied());
        }
        let absolute = std::path::absolute(path)?;
        let (relative, root) = self
            .0
            .iter()
            .find_map(|(p, f)| absolute.strip_prefix(p).ok().map(|r| (r, f)))
            .ok_or_else(denied)?;
        let parts: Vec<_> = relative
            .components()
            .filter_map(|c| {
                if let Component::Normal(s) = c {
                    Some(s)
                } else {
                    None
                }
            })
            .collect();
        let mut held = root.try_clone()?;
        let mut iter = parts.iter().peekable();
        while let Some(name) = iter.next() {
            held = open(&held, name, iter.peek().is_some())?;
        }
        if parts.is_empty() {
            return Err(denied());
        }
        Ok(held)
    }
    pub fn read(&self, path: &Path, limits: &Limits, meter: &Arc<Meter>) -> Result<Bytes, Error> {
        let mut out = DecodeBuf::new(meter, 0, limits.max_input_bytes, LimitKind::InputBytes)?;
        read_file(self.open(path)?, &mut out)?;
        Ok(out.into_bytes()?)
    }
    pub fn source(
        self: &Arc<Self>,
        path: &Path,
        limits: Limits,
        budget: Arc<dyn MemoryBudget>,
    ) -> Source {
        Source::Fetch {
            name: RawStr::new(
                split_base(path.as_os_str().as_bytes())
                    .unwrap_or(path.as_os_str().as_bytes())
                    .to_vec(),
            ),
            fetch: Arc::new(Input {
                roots: self.clone(),
                path: path.to_owned(),
                limits,
                budget,
            }),
        }
    }
}
fn read_file(mut file: File, out: &mut DecodeBuf) -> Result<(), Error> {
    let mut buf = [0; 8192];
    loop {
        match file.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => out.extend_from_slice(buf.get(..n).unwrap_or_default())?,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
}
#[derive(Debug)]
struct Input {
    roots: Arc<Roots>,
    path: PathBuf,
    limits: Limits,
    budget: Arc<dyn MemoryBudget>,
}
impl Fetch for Input {
    fn fetch(&self) -> Result<Bytes, Error> {
        self.fetch_with_reader().map(|(bytes, _)| bytes)
    }
    fn fetch_with_reader(&self) -> Result<(Bytes, ReaderKind), Error> {
        let meter = Meter::for_input(&self.limits, self.budget.clone(), Reservation::Input);
        let raw = self.path.as_os_str().as_bytes();
        let base = split_base(raw);
        if base.is_none() {
            match self.roots.open(&self.path) {
                Ok(file) => {
                    let mut out = DecodeBuf::new(
                        &meter,
                        0,
                        self.limits.max_input_bytes,
                        LimitKind::InputBytes,
                    )?;
                    read_file(file, &mut out)?;
                    return Ok((out.into_bytes()?, ReaderKind::Stream));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    let mut first = raw.to_vec();
                    first.extend_from_slice(b".split0");
                    match self.roots.open(Path::new(OsStr::from_bytes(&first))) {
                        Ok(_) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => {
                            return Err(e.into());
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
        let base = base.unwrap_or(raw);
        let mut out = DecodeBuf::new(
            &meter,
            0,
            self.limits.max_input_bytes,
            LimitKind::InputBytes,
        )?;
        for index in 0..999 {
            let mut part = base.to_vec();
            part.extend_from_slice(format!(".split{index}").as_bytes());
            match self.roots.open(Path::new(OsStr::from_bytes(&part))) {
                Ok(file) => {
                    read_file(file, &mut out)?;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    if index != 0 {
                        break;
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok((out.into_bytes()?, ReaderKind::Memory))
    }
}
