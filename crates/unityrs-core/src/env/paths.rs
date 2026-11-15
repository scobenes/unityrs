// Ported from UnityPy 1.25.2 (MIT): UnityPy/environment.py. See THIRD_PARTY_LICENSES.
//! Core path and archive entry points for `load_assets`
//! (`environment.py:282-308`) and `load_zip_file` (`environment.py:162-174`).
//! ZIP None assignments retain dict positions internally; public file
//! iterators expose actual files only.

use super::{Environment, Source, split, within_input_bytes};
use crate::budget::{DecodeBuf, Reservation};
use crate::detect::{DetectError, FileType, check_entry_type_with_reader};
use crate::error::{Error, LimitError, LimitKind};
use crate::files::File;
use crate::limits::{Meter, MeteredVec, heap_cost};
use crate::raw::RawStr;
use crate::stream::ReaderKind;
use crate::zip::ZipArchive;
use bytes::Bytes;
use std::path::Path;
use std::sync::Arc;

mod local;
pub(super) use local::confined_path;
pub(super) use local::local_path;
mod state;
use state::Seen;

/// UnityPy joins each yielded root and basename with fs.sep, even for `/`
/// (`environment.py:79-81`); a filesystem path join would lose that slash.
fn walk_file(root: &[u8], basename: &[u8], meter: &Arc<Meter>) -> Result<RawStr, Error> {
    Ok(RawStr::charged_concat(&[root, b"/", basename], meter)?)
}

impl Environment {
    fn input_meter(&self) -> Arc<Meter> {
        Meter::for_input(
            &self.ctx().limits,
            self.ctx().budget.clone(),
            Reservation::Input,
        )
    }

    /// Detect and load one source. ZIPs load their members and return None;
    /// other inputs return their File. This is the container-capable companion
    /// to the original single-File `load`/`load_dependency` interface.
    #[allow(
        clippy::needless_pass_by_value,
        reason = "consistent with Environment::load"
    )]
    pub fn load_file(&self, source: Source) -> Result<Option<File>, Error> {
        let meter = self.input_meter();
        // The str-path gate uses literal os.path.exists, including split0
        // fallback (environment.py:110-137). Only the eventual fs.open and
        // split joining normalise; the caller's spelling remains the key.
        if let Source::Path(p) | Source::ConfinedPath { path: p, .. } = &source
            && split::base(p.as_os_str().as_encoded_bytes()).is_none()
            && !p.exists()
        {
            let first = split::part(p.as_os_str().as_encoded_bytes(), 0, &meter)?;
            if !split::path(first.as_bytes())?.exists() {
                return Ok(None);
            }
        }
        let raw = match &source {
            Source::Path(p) | Source::ConfinedPath { path: p, .. } => {
                p.as_os_str().as_encoded_bytes()
            }
            Source::Bytes { name, .. } | Source::Fetch { name, .. } => name.as_bytes(),
        };
        let raw = if matches!(source, Source::Path(_) | Source::ConfinedPath { .. }) {
            split::base(raw).unwrap_or(raw)
        } else {
            raw
        };
        let name = RawStr::charged(raw, &meter)?;
        let (data, reader) = super::registry::read_source(&source, &meter, self.ctx())?;
        self.load_data(name, data, reader, &meter)
    }

    /// Explicit ZIP/APK loading, independent of filename or byte detection.
    /// Members retain their own names (`environment.py:162-174`).
    #[allow(
        clippy::needless_pass_by_value,
        reason = "consistent with Environment::load"
    )]
    pub fn load_zip_file(&self, source: Source) -> Result<(), Error> {
        let meter = self.input_meter();
        let data = if let Source::Path(path) | Source::ConfinedPath { path, .. } = &source {
            // Both fs.exists and fs.open normalise (environment.py:163-164).
            // This explicit entry point never joins split-looking paths.
            let local = local_path(path, &meter)?;
            let physical = local.path()?;
            let confined;
            let physical = if let Source::ConfinedPath { root, .. } = &source {
                confined = local::confined_path(physical, root)?;
                confined.as_path()
            } else {
                physical
            };
            super::read_path(physical, &self.ctx().limits, &meter)?
        } else {
            super::registry::read_source(&source, &meter, self.ctx())?.0
        };
        self.load_zip_data(data, &meter)
    }

    /// Constructor-style local path dispatch: case-sensitive `.zip`/`.apk`
    /// names explicitly open an archive; every other file uses detection.
    /// A path that is neither a file nor a folder is ignored.
    pub fn load_path(&self, path: &Path) -> Result<(), Error> {
        let meter = self.input_meter();
        // fs.isfile/isdir normalise, but dispatch receives the original arg
        // (environment.py:54-68), including load_file's literal OS gate.
        let local = local_path(path, &meter)?;
        let physical = local.path()?;
        let _path_storage =
            meter.charge(heap_cost(path.as_os_str().as_encoded_bytes().len() as u64))?;
        if physical.is_dir() {
            return self.load_folder(path);
        }
        let bytes = path.as_os_str().as_encoded_bytes();
        if !physical.is_file() {
            return Ok(());
        }
        let (_, ext) = crate::pystr::ntpath_splitext(bytes);
        if matches!(ext, b".zip" | b".apk") {
            self.load_zip_file(Source::Path(path.to_path_buf()))
        } else {
            self.load_file(Source::Path(path.to_path_buf())).map(|_| ())
        }
    }

    /// `load_assets` for filesystem paths, merging each split base once per
    /// call (`environment.py:294-308`). A normal path is always read as given.
    pub fn load_files<P: AsRef<Path>>(
        &self,
        paths: impl IntoIterator<Item = P>,
    ) -> Result<(), Error> {
        let meter = self.input_meter();
        let mut seen = Seen::new(&meter);
        for p in paths {
            // load_files supplies plain open (environment.py:75-77).
            // De-duplication and keys use the supplied split base spelling
            // (environment.py:294-308), before any fsspec operation.
            let p = p.as_ref();
            if let Some(base) = split::base(p.as_os_str().as_encoded_bytes())
                && seen.contains_or_insert(base)?
            {
                continue;
            }
            let _path_storage =
                meter.charge(heap_cost(p.as_os_str().as_encoded_bytes().len() as u64))?;
            self.load_asset_path(p)?;
        }
        Ok(())
    }

    fn load_asset_path(&self, p: &Path) -> Result<(), Error> {
        // load_assets uses its opener literally for non-split names, even
        // on a miss (environment.py:306-308); split joining alone uses fs.
        if split::base(p.as_os_str().as_encoded_bytes()).is_some() {
            self.load_file(Source::Path(p.to_path_buf()))?;
        } else {
            // Each source owns its meter; the caller's meter pays only for
            // traversal, temporary paths and split de-duplication.
            let meter = self.input_meter();
            let name = RawStr::charged(p.as_os_str().as_encoded_bytes(), &meter)?;
            let data = super::read_path(p, &self.ctx().limits, &meter)?;
            self.load_data(name, data, ReaderKind::Stream, &meter)?;
        }
        Ok(())
    }

    /// Recursively load a local folder, files before child folders, keeping
    /// the filesystem's order. Walk state and path buffers are metered.
    /// Symlink directories inside the walk are opened as files, as fsspec's
    /// walk does; a symlink passed as the root follows its target.
    pub fn load_folder(&self, path: &Path) -> Result<(), Error> {
        let meter = self.input_meter();
        // fs.walk normalises the root lexically, preserving embedded '.'
        // and repeated separators in keys (fsspec make_path_posix).
        // It returns physical names
        // for load_files' plain open (environment.py:79-81). Empty roots
        // denote cwd, so normalisation precedes existence checks.
        // The cwd and directory entry OS allocations are reserved first.
        let _os = meter.charge(heap_cost(32_768))?;
        let root = local::walk_root(path, &meter)?;
        if !split::path(root.as_bytes())?.exists() {
            return Ok(());
        }
        let mut pending = MeteredVec::new(&meter);
        pending.push(root)?;
        let mut seen = Seen::new(&meter);
        while let Some(dir) = pending.pop() {
            let mut children = MeteredVec::new(&meter);
            let walk = match std::fs::read_dir(split::path(dir.as_bytes())?) {
                Ok(w) => w,
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => continue,
                Err(e) => return Err(e.into()),
            };
            for entry in walk {
                let entry = entry?;
                // OS directory entry/path temporaries are bounded by the OS
                // path maximum, covered by the walk's reservation above.
                if entry.file_type()?.is_dir() {
                    // fsspec normalises listing names and recursive roots
                    // separately from UnityPy's file keys:
                    // https://github.com/fsspec/filesystem_spec/blob/2025.5.1/fsspec/spec.py#L384-L434
                    // https://github.com/fsspec/filesystem_spec/blob/2025.5.1/fsspec/implementations/local.py#L54-L116
                    children.push(local::walk_root(&entry.path(), &meter)?)?;
                } else {
                    let name =
                        walk_file(dir.as_bytes(), entry.file_name().as_encoded_bytes(), &meter)?;
                    if let Some(base) = split::base(name.as_bytes())
                        && seen.contains_or_insert(base)?
                    {
                        continue;
                    }
                    self.load_asset_path(split::path(name.as_bytes())?)?;
                }
            }
            for child in children.as_mut_slice().iter().rev() {
                pending.push(child.clone())?;
            }
        }
        Ok(())
    }

    pub(super) fn load_zip_data(&self, data: Bytes, meter: &Arc<Meter>) -> Result<(), Error> {
        let mut stack = MeteredVec::new(meter);
        stack.push(Frame::new(data, None, meter)?)?;
        while !stack.is_empty() {
            let depth = stack.len().saturating_sub(1) as u64;
            if depth > u64::from(self.ctx().limits.max_container_depth) {
                return Err(Error::Limit(LimitError::refused(
                    LimitKind::ContainerDepth,
                    depth,
                    u64::from(self.ctx().limits.max_container_depth),
                )));
            }
            let Some(frame) = stack.as_mut_slice().last_mut() else {
                break;
            };
            let Some(member) = frame.zip.members().get(frame.at) else {
                if let Some(done) = stack.pop()
                    && let Some(name) = done.name
                {
                    self.list_zip_none(name, meter)?;
                }
                continue;
            };
            let index = frame.at;
            frame.at += 1;
            let member_name = member.name.clone();
            let (name, data, reader) = if let Some(base) = split::base(member_name.as_bytes()) {
                if frame.seen.contains_or_insert(base)? {
                    continue;
                }
                let name = RawStr::charged(base, meter)?;
                let mut out = DecodeBuf::new(
                    meter,
                    0,
                    self.ctx().limits.max_input_bytes,
                    LimitKind::InputBytes,
                )?;
                for i in 0..999 {
                    let part = split::part(base, i, meter)?;
                    if let Some(k) = frame.zip.find(part.as_bytes())? {
                        let bytes = frame.read(k, &self.ctx().limits)?;
                        within_input_bytes(
                            (out.len() as u64).saturating_add(bytes.len() as u64),
                            &self.ctx().limits,
                        )?;
                        out.extend_from_slice(&bytes)?;
                    } else if i != 0 {
                        break;
                    }
                }
                (name, out.into_bytes()?, ReaderKind::Memory)
            } else {
                // ZipFile.open(name) resolves duplicate names to the last row.
                let name = member_name.clone();
                let k = frame.zip.find(name.as_bytes())?.unwrap_or(index);
                (name, frame.read(k, &self.ctx().limits)?, ReaderKind::Stream)
            };
            within_input_bytes(data.len() as u64, &self.ctx().limits)?;
            let ty = check_entry_type_with_reader(&data, &self.ctx().limits, meter, reader)
                .map_err(|e| match e {
                    DetectError::Work(limit) => Error::Limit(limit),
                    e => Error::Detect(e),
                })?;
            if ty == FileType::Zip {
                stack.push(Frame::new(data, Some(name), meter)?)?;
            } else {
                self.load_detected(
                    name,
                    data,
                    reader,
                    ty,
                    meter,
                    u32::try_from(stack.len()).unwrap_or(u32::MAX),
                )?;
            }
        }
        Ok(())
    }
}

struct Cached {
    remaining: usize,
    bytes: Option<Bytes>,
}

struct Frame {
    zip: ZipArchive,
    at: usize,
    seen: Seen,
    cache: MeteredVec<Cached>,
    name: Option<RawStr>,
}
impl Frame {
    fn new(data: Bytes, name: Option<RawStr>, meter: &Arc<Meter>) -> Result<Self, Error> {
        let zip = ZipArchive::open(data, meter)?;
        let mut cache = MeteredVec::new(meter);
        for _ in zip.members() {
            meter.count_decode(1)?;
            cache.push(Cached {
                remaining: 0,
                bytes: None,
            })?;
        }
        // Only ordinary rows resolve by name on every visit. Split parts
        // are consumed once by their base's merge and never need caching.
        for member in zip.members() {
            if split::base(member.name.as_bytes()).is_none() {
                let index = zip
                    .find(member.name.as_bytes())?
                    .ok_or_else(|| crate::zip::bad("cache name"))?;
                let slot = cache
                    .as_mut_slice()
                    .get_mut(index)
                    .ok_or_else(|| crate::zip::bad("cache index"))?;
                slot.remaining = slot
                    .remaining
                    .checked_add(1)
                    .ok_or_else(|| crate::zip::bad("cache references overflow"))?;
            }
        }
        Ok(Self {
            zip,
            at: 0,
            seen: Seen::new(meter),
            cache,
            name,
        })
    }
    fn read(&mut self, index: usize, limits: &crate::limits::Limits) -> Result<Bytes, Error> {
        let slot = self
            .cache
            .as_mut_slice()
            .get_mut(index)
            .ok_or_else(|| crate::zip::bad("cache index"))?;
        slot.remaining = slot.remaining.saturating_sub(1);
        if let Some(bytes) = slot.bytes.take() {
            if slot.remaining != 0 {
                slot.bytes = Some(bytes.clone());
            }
            return Ok(bytes);
        }
        let bytes = self.zip.read_for_load(index, limits)?;
        if slot.remaining != 0 {
            slot.bytes = Some(bytes.clone());
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_file_keys_preserve_the_yielded_root_and_separator() {
        let env = Environment::new(super::super::EnvOptions::unitypy());
        let meter = env.input_meter();
        for (root, expected) in [
            ("/", "//x"),
            ("/a//./b", "/a//./b/x"),
            ("relative", "relative/x"),
        ] {
            assert_eq!(
                walk_file(root.as_bytes(), b"x", &meter).unwrap().as_bytes(),
                expected.as_bytes()
            );
        }
    }
}
