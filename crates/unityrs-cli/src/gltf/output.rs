//! Exclusive sibling staging. A failure only removes this invocation's temps.
use super::{Result, args::path};
use std::{
    fs::{self, File},
    io::Write,
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
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Pending {
    temporary: RawStr,
    destination: RawStr,
    renamed: bool,
}
impl Drop for Pending {
    fn drop(&mut self) {
        if !self.renamed {
            let _ = fs::remove_file(path(self.temporary.as_bytes()));
        }
    }
}
pub(super) struct Outputs {
    pending: MeteredVec<Pending>,
}
impl Outputs {
    pub fn new(meter: &Arc<Meter>) -> Self {
        Self {
            pending: MeteredVec::new(meter),
        }
    }
    pub fn write(
        &mut self,
        destination: &Path,
        write: impl FnOnce(&mut File) -> Result<()>,
    ) -> Result<()> {
        // Reserve before the writer/report observes peak charges. A refusal
        // creates no temporary file and never publishes a partial result.
        self.pending.reserve(1)?;
        let meter = self.pending.charge().meter();
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let destination =
            RawStr::charged_concat(&[destination.as_os_str().as_encoded_bytes()], meter)?;
        let (pending, mut file) = loop {
            // Constant-sized formatting; the input-sized parent is charged first.
            let suffix = format!(
                "/.unityrs-gltf-{}-{}.tmp",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let temporary = RawStr::charged_concat(
                &[parent.as_os_str().as_encoded_bytes(), suffix.as_bytes()],
                meter,
            )?;
            match File::options()
                .write(true)
                .create_new(true)
                .open(path(temporary.as_bytes()))
            {
                Ok(file) => {
                    break (
                        Pending {
                            temporary,
                            destination,
                            renamed: false,
                        },
                        file,
                    );
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e.into()),
            }
        };
        write(&mut file)?;
        file.flush()?;
        drop(file);
        self.pending.push(pending)?;
        Ok(())
    }
    pub fn finish(mut self) -> Result<()> {
        for pending in self.pending.as_slice() {
            let destination = path(pending.destination.as_bytes());
            match fs::symlink_metadata(destination) {
                Ok(m) if m.is_dir() => {
                    return Err(format!("output is a directory: {}", destination.display()).into());
                }
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        for pending in self.pending.as_mut_slice() {
            fs::rename(
                path(pending.temporary.as_bytes()),
                path(pending.destination.as_bytes()),
            )
            .map_err(|e| {
                std::io::Error::new(
                    e.kind(),
                    format!(
                        "cannot publish output {}: {e}",
                        path(pending.destination.as_bytes()).display()
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
    fn failed_publish_keeps_earlier_complete_outputs_and_only_cleans_own_temps() {
        let tmp = Scratch::new("gltf-publish-failure");
        let mut outputs = Outputs::new(&Meter::unlimited());
        for name in ["scene.glb", "report.json"] {
            fs::write(tmp.join(name), b"old").unwrap();
            outputs
                .write(&tmp.join(name), |out| {
                    out.write_all(b"new complete")?;
                    Ok(())
                })
                .unwrap();
        }
        fs::remove_file(path(outputs.pending.as_slice()[1].temporary.as_bytes())).unwrap();
        fs::write(tmp.join(".unityrs-gltf-foreign.tmp"), b"kept").unwrap();
        let error = outputs.finish().unwrap_err().to_string();
        assert!(error.contains("report.json"), "{error}");
        assert_eq!(fs::read(tmp.join("scene.glb")).unwrap(), b"new complete");
        assert_eq!(fs::read(tmp.join("report.json")).unwrap(), b"old");
        assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 3);
    }
}
