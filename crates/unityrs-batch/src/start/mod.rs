//! Reusable startup components. B9 owns their final process-level ordering.
pub mod host;
pub mod key;
pub mod spool;
use crate::spec::model::{Sink, Spec};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsupported {
    ObjectStore,
}
/// Validate the execution subset before any host inspection or mutation.
/// Full schema validation and `--check` remain independent of this guard.
/// Local files and HTTP(S) URL bundles are supported. The lifetime
/// fields (`deadline`, `stop_on_stdin_eof`, `parent_pid`) are the process
/// host's (`run::process`); a library run ignores them.
pub fn supported(spec: &Spec) -> Result<Local<'_>, Unsupported> {
    let Sink::Directory { root } = &spec.sink else {
        return Err(Unsupported::ObjectStore);
    };
    Ok(Local { spec, root })
}
/// An execution-subset proof, obtainable only via [`supported`].
pub struct Local<'a> {
    pub(crate) spec: &'a Spec,
    pub(crate) root: &'a str,
}
impl Local<'_> {
    #[must_use]
    pub fn spec(&self) -> &Spec {
        self.spec
    }
    #[must_use]
    pub fn root(&self) -> &std::path::Path {
        std::path::Path::new(self.root)
    }
}
