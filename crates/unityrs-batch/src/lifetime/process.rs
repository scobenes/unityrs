use super::{State, Stop};
use crate::{
    control::{Control, Culprit},
    job::RunFailure,
    reason::{Code, StopReason},
    sys::{lifecycle::Pipe, reset::SocketTable},
};

/// Process-lived storage is required before handlers become reachable. The
/// host retains it until _exit; installation never borrows a stack pointer.
pub struct Process {
    pub state: State,
    pub sockets: SocketTable,
    pub pipe: Pipe,
}
impl Process {
    pub fn prepare() -> std::io::Result<Self> {
        Ok(Self {
            state: State::new(),
            sockets: SocketTable::new(),
            pipe: Pipe::new()?,
        })
    }
    pub fn request(&self, stop: Stop) {
        self.state.request(stop);
        self.pipe.notify();
    }
    pub fn signal(&self, reason: StopReason) {
        if self.state.second_signal() {
            self.exit(self.state.end_code(Some(reason)));
        }
        self.request(Stop::external(reason));
    }
    pub fn exit(&self, code: Code) -> ! {
        crate::sys::lifecycle::exit(code, &self.sockets)
    }
}
impl Control for Process {
    fn stopped(&self) -> bool {
        self.state.live().is_some()
    }
    fn request_stop(&self, failure: RunFailure, culprit: Culprit) {
        self.request((failure, culprit).into());
    }
}
