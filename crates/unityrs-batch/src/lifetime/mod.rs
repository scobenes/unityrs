//! Process lifetime policy. These primitives do not activate CLI lifetime
//! support: the engine hand-over and stopped finisher own that integration.
mod clock;
mod process;
mod runtimes;
mod stdin;
mod watch;
mod word;
pub use clock::{Boot, Policy, TimingError};
pub use process::Process;
pub use runtimes::Runtimes;
pub use stdin::StdinWatch;
pub use watch::Watch;
pub use word::{SpoolDetail, State, Stop};
