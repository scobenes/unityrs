//! Stage sibling files and publish them only after the whole export succeeds.
//! Publication overwrites existing files and symlinks in staging order. A failed
//! rename leaves already published files in place. An interrupted run can leave
//! `.unityrs-obj-*.tmp` siblings, which are safe to delete.
use std::{
    ffi::OsStr,
    fs::{self, File},
    os::unix::ffi::OsStrExt,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use unityrs_core::{
    RawStr,
    limits::{Meter, MeteredVec},
};
use unityrs_mesh::{
    MeshError,
    obj::{MeteredWrite, Writer},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn path(raw: &RawStr) -> &Path {
    Path::new(OsStr::from_bytes(raw.as_bytes()))
}

struct Pending {
    temporary: RawStr,
    destination: RawStr,
    renamed: bool,
}
impl Pending {
    fn create(destination: &Path, meter: &Arc<Meter>) -> Result<(Self, File), MeshError> {
        let destination = RawStr::charged_concat(&[destination.as_os_str().as_bytes()], meter)?;
        let parent = path(&destination)
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        loop {
            let suffix = format!(
                "/.unityrs-obj-{}-{}.tmp",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let temporary =
                RawStr::charged_concat(&[parent.as_os_str().as_bytes(), suffix.as_bytes()], meter)?;
            match File::options()
                .write(true)
                .create_new(true)
                .open(path(&temporary))
            {
                Ok(file) => {
                    return Ok((
                        Self {
                            temporary,
                            destination,
                            renamed: false,
                        },
                        file,
                    ));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        if !self.renamed {
            let _ = fs::remove_file(path(&self.temporary));
        }
    }
}

pub(super) struct Outputs {
    pending: MeteredVec<Pending>,
}
impl Outputs {
    pub(super) fn new(meter: &Arc<Meter>) -> Self {
        Self {
            pending: MeteredVec::new(meter),
        }
    }
    pub(super) fn write(
        &mut self,
        destination: &Path,
        write: &mut dyn FnMut(&mut dyn MeteredWrite) -> Result<(), MeshError>,
    ) -> Result<(), MeshError> {
        let meter = self.pending.charge().meter();
        let (pending, file) = Pending::create(destination, meter)?;
        let mut out = Writer::new(file, meter);
        write(&mut out)?;
        drop(out.finish()?);
        self.pending.push(pending)?;
        Ok(())
    }
    pub(super) fn finish(mut self) -> Result<(), MeshError> {
        for pending in self.pending.as_slice() {
            let destination = path(&pending.destination);
            // Inspect the entry itself: symlinks, including ones to directories,
            // can be replaced by rename without changing their targets.
            match fs::symlink_metadata(destination) {
                Ok(metadata) if metadata.is_dir() => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::IsADirectory,
                        format!("output is a directory: {}", destination.display()),
                    )
                    .into());
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(std::io::Error::new(
                        e.kind(),
                        format!("cannot inspect output {}: {e}", destination.display()),
                    )
                    .into());
                }
            }
        }
        for pending in self.pending.as_mut_slice() {
            fs::rename(path(&pending.temporary), path(&pending.destination)).map_err(|e| {
                std::io::Error::new(
                    e.kind(),
                    format!(
                        "cannot publish output {}: {e}",
                        path(&pending.destination).display()
                    ),
                )
            })?;
            pending.renamed = true;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unityrs_testkit::scratch::Scratch;

    #[test]
    fn failed_rename_preserves_published_files_and_cleans_only_own_temps() {
        let tmp = Scratch::new("obj-publish-failure");
        let mut outputs = Outputs::new(&Meter::unlimited());
        for name in ["tex.png", "m.obj", "m.mtl"] {
            fs::write(tmp.join(name), b"existing").unwrap();
            outputs
                .write(&tmp.join(name), &mut |out| out.write_bytes(b"complete"))
                .unwrap();
        }
        // Lose the second source after staging to force failure after one rename.
        fs::remove_file(path(&outputs.pending.as_slice()[1].temporary)).unwrap();
        let foreign_temp = tmp.join(".unityrs-obj-other.tmp");
        fs::write(&foreign_temp, b"another invocation").unwrap();

        let error = outputs.finish().unwrap_err().to_string();
        assert!(error.contains("m.obj"), "{error}");
        assert_eq!(fs::read(tmp.join("tex.png")).unwrap(), b"complete");
        assert_eq!(fs::read(tmp.join("m.obj")).unwrap(), b"existing");
        assert_eq!(fs::read(tmp.join("m.mtl")).unwrap(), b"existing");
        assert_eq!(fs::read(&foreign_temp).unwrap(), b"another invocation");
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 4);
    }

    #[test]
    fn publication_replaces_symlinks_without_touching_their_targets() {
        let tmp = Scratch::new("obj-publish-symlink");
        let target = tmp.join("target");
        fs::create_dir(&target).unwrap();
        let destination = tmp.join("mesh.obj");
        std::os::unix::fs::symlink(&target, &destination).unwrap();
        let mut outputs = Outputs::new(&Meter::unlimited());
        outputs
            .write(&destination, &mut |out| out.write_bytes(b"complete"))
            .unwrap();
        outputs.finish().unwrap();
        assert!(fs::symlink_metadata(&destination).unwrap().is_file());
        assert_eq!(fs::read(&destination).unwrap(), b"complete");
        assert!(target.is_dir());
        assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 2);
    }
}
