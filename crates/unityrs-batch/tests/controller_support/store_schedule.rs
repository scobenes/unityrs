//! Store sweep schedules: no clock decides where payload work resumes or
//! which thread drops the final store owner and its finisher reactor.
use std::{
    os::unix::net::UnixStream,
    task::{Context, Waker},
    time::Instant,
};
use tokio::io::unix::AsyncFd;
use unityrs_batch::{sink::store::Store, sys::Shared};
use unityrs_testkit::guard::DEADLOCK_GUARD;

pub struct Retire {
    store: Shared<Store>,
    registration: AsyncFd<UnixStream>,
    peer: UnixStream,
}
impl Retire {
    /// Called by the exempt observer, before the finisher. A live, idle
    /// registration makes Tokio's shutdown collection nonempty even if its
    /// last real socket was already purged by an IO-driver turn.
    pub fn new(store: &Shared<Store>) -> Self {
        let (socket, peer) = UnixStream::pair().unwrap();
        socket.set_nonblocking(true).unwrap();
        let _entered = store.test_finisher_handle().unwrap().enter();
        // SAFETY: the owned UnixStream's descriptor is never exposed,
        // replaced or closed until this AsyncFd is dropped.
        let registration =
            unsafe { AsyncFd::register_with_interest(socket, tokio::io::Interest::READABLE) }
                .unwrap();
        // Prime this infrastructure registration's lazy waiters mutex on
        // Darwin, before the counted runtime shutdown can lock it.
        let mut cx = Context::from_waker(Waker::noop());
        assert!(registration.poll_read_ready(&mut cx).is_pending());
        Self {
            store: store.clone(),
            registration,
            peer,
        }
    }

    pub fn finish(self) {
        let Self {
            store,
            registration,
            peer,
        } = self;
        // Production teardown detaches retiring workers. Retain the owner
        // until they release it, so all children count its drop here rather
        // than racing a worker's Context destructor or the allocator disarm.
        let deadline = Instant::now() + DEADLOCK_GUARD;
        while Shared::strong_count(&store) != 1 {
            assert!(Instant::now() < deadline, "worker retained the store");
            std::thread::yield_now();
        }
        drop(store);
        super::ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
        // Both descriptors belong to the test, outside the counted span.
        drop(registration);
        drop(peer);
    }
}
