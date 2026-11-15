//! Reusable startup components. B9 owns their final process-level ordering.
pub mod host;
pub mod key;
pub mod spool;
use crate::spec::model::{Sink, Spec};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unsupported {
    UrlSource,
    ObjectStore,
    Deadline,
    StdinWatch,
    ParentWatch,
}
/// Validate the execution subset before any host inspection or mutation.
/// Full schema validation and `--check` remain independent of this guard.
/// A URL is refused here until the CLI activates URL staging; a library run
/// given a network admits it through [`supported_with`].
pub fn supported(spec: &Spec) -> Result<Local<'_>, Unsupported> {
    supported_with(spec, false)
}
/// [`supported`], admitting URL bundles when `urls` (a networked run).
pub(crate) fn supported_with(spec: &Spec, urls: bool) -> Result<Local<'_>, Unsupported> {
    if !urls && spec.bundles().any(|b| b.url().is_some()) {
        return Err(Unsupported::UrlSource);
    }
    let Sink::Directory { root } = &spec.sink else {
        return Err(Unsupported::ObjectStore);
    };
    if spec.deadline.is_some() {
        return Err(Unsupported::Deadline);
    }
    if spec.stop_on_stdin_eof {
        return Err(Unsupported::StdinWatch);
    }
    if spec.parent_pid.is_some() {
        return Err(Unsupported::ParentWatch);
    }
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
