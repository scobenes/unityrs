//! One chronological queue for timers, IO receipts and blocking completions.
use std::{
    collections::BTreeMap,
    future::{Future, poll_fn},
    ops::Deref,
    pin::Pin,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
    thread::ThreadId,
    time::{Duration, SystemTime},
};
use unityrs_batch::stage::timers::Clock;

type Gate = Arc<(Mutex<bool>, Condvar)>;
enum Delivery {
    Wake(Waker),
    Write(Gate),
}
#[derive(Default)]
pub struct Time {
    pub ms: AtomicU64,
    pub waits: Mutex<Vec<u64>>,
    events: Mutex<BTreeMap<u64, (u64, Delivery)>>,
    sequence: AtomicU64,
    driver: Mutex<Option<Waker>>,
    returning: Mutex<Option<Arc<ReturnBarrier>>>,
}
impl Time {
    /// Stop a worker at its completion timestamp, before publishing its join
    /// result. Runtime-thread clock reads remain runnable.
    pub fn hold_return_at(&self, at: u64) -> Arc<ReturnBarrier> {
        let barrier = Arc::new(ReturnBarrier {
            at,
            owner: std::thread::current().id(),
            entered: AtomicBool::new(false),
            released: (Mutex::new(false), Condvar::new()),
        });
        *self.returning.lock().unwrap() = Some(barrier.clone());
        barrier
    }
    fn insert(&self, at: u64, delivery: Delivery) -> u64 {
        let id = self.sequence.fetch_add(1, Ordering::Relaxed);
        self.events.lock().unwrap().insert(id, (at, delivery));
        if let Some(driver) = self.driver.lock().unwrap().as_ref() {
            driver.wake_by_ref();
        }
        id
    }
    /// The sole clock advancement operation. Deliver every event at the
    /// earliest timestamp, preserving all earlier network/timer events.
    fn step(&self) -> bool {
        let mut events = self.events.lock().unwrap();
        let Some(at) = events.values().map(|(at, _)| *at).min() else {
            return false;
        };
        self.ms.fetch_max(at, Ordering::AcqRel);
        self.waits.lock().unwrap().push(at);
        let ids: Vec<_> = events
            .iter()
            .filter(|(_, (deadline, _))| *deadline == at)
            .map(|(id, _)| *id)
            .collect();
        let deliveries: Vec<_> = ids
            .into_iter()
            .map(|id| events.remove(&id).unwrap().1)
            .collect();
        drop(events);
        for delivery in deliveries {
            match delivery {
                Delivery::Wake(waker) => waker.wake(),
                Delivery::Write(gate) => {
                    *gate.0.lock().unwrap() = true;
                    gate.1.notify_all();
                }
            }
        }
        true
    }
    /// Explicit elapsed synchronous work or test setup also traverses the
    /// queue; adapters cannot assign the clock themselves.
    pub fn at(&self, at: u64) {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        self.insert(at, Delivery::Write(gate.clone()));
        while !*gate.0.lock().unwrap() {
            assert!(self.step());
        }
    }
    /// IO receipts register once and yield to `drive`, just like timers. Only
    /// the driver may advance time after all runnable stages have yielded.
    pub fn receipt(self: &Arc<Self>, at: u64) -> Sleep<Arc<Self>> {
        Sleep {
            time: self.clone(),
            deadline: at,
            id: None,
        }
    }
    pub fn complete_write_at(&self, at: u64) {
        if self.now() >= Duration::from_millis(at) {
            return;
        }
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        self.insert(at, Delivery::Write(gate.clone()));
        let (done, _) = gate
            .1
            .wait_timeout_while(
                gate.0.lock().unwrap(),
                unityrs_testkit::guard::DEADLOCK_GUARD,
                |done| !*done,
            )
            .unwrap();
        assert!(*done, "scheduled write completion guard");
    }
    /// Outstanding writes include results not yet received by their stage,
    /// even after the worker has finished its IO and is about to return.
    pub async fn drive<F: Future>(&self, future: F, outstanding: impl Fn() -> usize) -> F::Output {
        let mut future = std::pin::pin!(future);
        let mut wake: Option<Arc<DriverWake>> = None;
        poll_fn(|cx| {
            *self.driver.lock().unwrap() = Some(cx.waker().clone());
            // Keep one wake flag for this whole drive: a task may still hold
            // a waker from an earlier poll when it becomes runnable.
            let wake = wake.get_or_insert_with(|| {
                Arc::new(DriverWake {
                    woke: AtomicBool::new(false),
                    outer: Mutex::new(cx.waker().clone()),
                })
            });
            wake.outer.lock().unwrap().clone_from(cx.waker());
            wake.woke.store(false, Ordering::Release);
            let waker = Waker::from(wake.clone());
            let result = future.as_mut().poll(&mut Context::from_waker(&waker));
            let blocked = self
                .events
                .lock()
                .unwrap()
                .values()
                .filter(|(_, event)| matches!(event, Delivery::Write(_)))
                .count();
            if result.is_pending() && outstanding() <= blocked && !wake.woke.load(Ordering::Acquire)
            {
                self.step();
            }
            result
        })
        .await
    }
}
impl Clock for Time {
    fn now(&self) -> Duration {
        let ms = self.ms.load(Ordering::Acquire);
        let returning = self.returning.lock().unwrap().clone();
        if let Some(barrier) = returning
            && ms == barrier.at
            && std::thread::current().id() != barrier.owner
            && !barrier.entered.swap(true, Ordering::AcqRel)
        {
            if let Some(driver) = self.driver.lock().unwrap().as_ref() {
                driver.wake_by_ref();
            }
            let (released, _) = barrier
                .released
                .1
                .wait_timeout_while(
                    barrier.released.0.lock().unwrap(),
                    unityrs_testkit::guard::DEADLOCK_GUARD,
                    |released| !*released,
                )
                .unwrap();
            assert!(*released, "worker return barrier guard");
        }
        Duration::from_millis(ms)
    }
    fn wall(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(784_111_777) + self.now()
    }
    async fn sleep_until(&self, deadline: Duration) {
        Sleep {
            time: self,
            deadline: deadline.as_millis().try_into().unwrap(),
            id: None,
        }
        .await;
    }
}
pub struct ReturnBarrier {
    at: u64,
    owner: ThreadId,
    pub entered: AtomicBool,
    released: (Mutex<bool>, Condvar),
}
impl ReturnBarrier {
    pub fn release(&self) {
        *self.released.0.lock().unwrap() = true;
        self.released.1.notify_all();
    }
}
pub struct Sleep<T: Deref<Target = Time>> {
    time: T,
    deadline: u64,
    id: Option<u64>,
}
impl<T: Deref<Target = Time> + Unpin> Future for Sleep<T> {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.time.ms.load(Ordering::Acquire) >= self.deadline {
            return Poll::Ready(());
        }
        if let Some(id) = self.id {
            self.time
                .events
                .lock()
                .unwrap()
                .insert(id, (self.deadline, Delivery::Wake(cx.waker().clone())));
        } else {
            self.id = Some(
                self.time
                    .insert(self.deadline, Delivery::Wake(cx.waker().clone())),
            );
        }
        Poll::Pending
    }
}
impl<T: Deref<Target = Time>> Drop for Sleep<T> {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            self.time.events.lock().unwrap().remove(&id);
        }
    }
}
struct DriverWake {
    woke: AtomicBool,
    outer: Mutex<Waker>,
}
impl Wake for DriverWake {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.woke.store(true, Ordering::Release);
        self.outer.lock().unwrap().wake_by_ref();
    }
}
