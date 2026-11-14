#![allow(dead_code)]
use crate::{
    http_support::{MemoryIo, run},
    stage_support,
};
use serde_json::json;
use std::{
    collections::VecDeque,
    future::{Future, pending},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use unityrs_batch::{
    control::{Control, Culprit},
    http::{
        self, Data, Detail, Error, Payload,
        permit::{PermitPool, RequestPermit},
    },
    job::RunFailure,
    spec::model::Http,
    spool::{Counter, OwnedFile},
    stage::{
        StageEnd,
        timers::{Clock, Context},
        url::{Body, Stage, Transport},
    },
};
use unityrs_testkit::scratch::Scratch;

pub const ORIGINAL: &str = "http://source.invalid/start?original_secret=hidden#fragment";
mod time;
pub use time::Time;
#[derive(Default)]
pub struct Stop {
    pub stopped: AtomicBool,
    pub failures: Mutex<Vec<(RunFailure, Culprit)>>,
}
impl Control for Stop {
    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
    fn request_stop(&self, failure: RunFailure, culprit: Culprit) {
        self.failures.lock().unwrap().push((failure, culprit));
        self.stopped.store(true, Ordering::Release);
    }
}
#[derive(Clone)]
pub struct Reply {
    pub raw: Arc<[u8]>,
    pub head_at: u64,
    pub connect_at: u64,
    pub reset_at: Option<usize>,
    pub body_at: Vec<u64>,
    pub fragment: usize,
    pub read_at: Vec<(usize, u64)>,
    pub failure: Option<Error>,
    pub stall: bool,
}
impl Reply {
    pub fn raw(raw: impl Into<Vec<u8>>) -> Self {
        Self {
            raw: raw.into().into(),
            head_at: 0,
            connect_at: 0,
            reset_at: None,
            body_at: vec![],
            fragment: usize::MAX,
            read_at: vec![],
            failure: None,
            stall: false,
        }
    }
    pub fn status(status: u16, fields: &str) -> Self {
        Self::raw(format!(
            "HTTP/1.1 {status} Test\r\n{fields}Content-Length: 0\r\n\r\n"
        ))
    }
    pub fn ok(bytes: &[u8]) -> Self {
        let mut raw =
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", bytes.len()).into_bytes();
        raw.extend_from_slice(bytes);
        Self::raw(raw)
    }
    pub fn fail(detail: Detail) -> Self {
        let mut reply = Self::raw(vec![]);
        reply.failure = Some(Error::Transport(http::Failure {
            detail,
            errno: Some(123),
        }));
        reply
    }
}
pub struct Owner(pub Arc<AtomicU64>);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
pub struct ReplyBody {
    _owner: Owner,
    response: http::Response<MemoryIo>,
    time: Arc<Time>,
    at: VecDeque<u64>,
    stall: bool,
}
impl Body for ReplyBody {
    fn status(&self) -> u16 {
        self.response.status()
    }
    fn headers(&self) -> &hyper::HeaderMap {
        self.response.headers()
    }
    async fn data(
        &mut self,
        credit: &mut impl FnMut(usize) -> Result<(), Error>,
    ) -> Result<Option<Data>, Error> {
        if let Some(&at) = self.at.front() {
            self.time.sleep_until(Duration::from_millis(at)).await;
            self.at.pop_front();
        }
        if self.stall {
            pending::<()>().await;
        }
        self.response.data_with_credit(credit).await
    }
}
#[derive(Debug)]
pub struct Seen {
    pub uri: String,
    pub headers: hyper::HeaderMap,
    pub at: u64,
    pub claimed: u64,
}
pub struct Peer {
    pub live: Arc<AtomicU64>,
    pub phase: AtomicU64,
    replies: Mutex<VecDeque<Reply>>,
    pub seen: Mutex<Vec<Seen>>,
    pub time: Arc<Time>,
    pub counter: Arc<Counter>,
}
impl Transport for Peer {
    type Response = ReplyBody;
    async fn request(
        &self,
        permit: &mut RequestPermit,
        _: &url::Url,
        request: hyper::Request<Payload>,
        _: Duration,
        token: http::permit::OperationToken,
    ) -> Result<ReplyBody, Error> {
        self.seen.lock().unwrap().push(Seen {
            uri: request.uri().to_string(),
            headers: request.headers().clone(),
            at: self.time.now().as_millis().try_into().unwrap(),
            claimed: self.counter.used(),
        });
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected request");
        self.live.fetch_add(1, Ordering::AcqRel);
        let owner = Owner(self.live.clone());
        // There is no resolver pool task in this adapter. Network availability
        // is scheduled by Time, not classified as a busy blocking write.
        drop(token);
        self.phase.store(1, Ordering::Release);
        self.time
            .sleep_until(Duration::from_millis(reply.connect_at))
            .await;
        self.phase.store(2, Ordering::Release);
        self.time
            .sleep_until(Duration::from_millis(reply.head_at))
            .await;
        if let Some(error) = reply.failure {
            return Err(error);
        }
        let mut io = MemoryIo::shared(reply.raw, reply.fragment);
        if !reply.read_at.is_empty() {
            let time = self.time.clone();
            let mut schedule: VecDeque<_> = reply.read_at.into();
            let mut receipt = None;
            io = io.on_read(move |offset, cx| {
                if let Some(&(start, at)) = schedule.front() {
                    if offset >= start {
                        let wait = receipt.get_or_insert_with(|| time.receipt(at));
                        if Pin::new(wait).poll(cx).is_pending() {
                            return std::task::Poll::Pending;
                        }
                        receipt = None;
                        schedule.pop_front();
                    } else {
                        return std::task::Poll::Ready(start - offset);
                    }
                }
                std::task::Poll::Ready(
                    schedule
                        .front()
                        .map_or(usize::MAX, |(start, _)| start - offset),
                )
            });
        }
        if let Some(offset) = reply.reset_at {
            io = io.failing_read(offset);
        }
        let response = http::exchange(io, permit, request).await?;
        self.phase.store(3, Ordering::Release);
        Ok(ReplyBody {
            _owner: owner,
            response,
            time: self.time.clone(),
            at: reply.body_at.into(),
            stall: reply.stall,
        })
    }
}
pub struct Scene {
    pub scratch: Scratch,
    pub counter: Arc<Counter>,
    pub time: Arc<Time>,
    pub stop: Arc<Stop>,
    pub peer: Peer,
    pub policy: Http,
    pub bundle: serde_json::Value,
    pub m: u64,
    pub cutoff: Option<Duration>,
}
impl Scene {
    pub fn new(replies: Vec<Reply>) -> Self {
        let counter = Counter::new(1024 * 1024 * 1024, 0, 4096).unwrap();
        let time = Arc::new(Time::default());
        Self {
            scratch: Scratch::new("url-stage"),
            counter: counter.clone(),
            time: time.clone(),
            stop: Arc::new(Stop::default()),
            peer: Peer {
                live: Arc::new(AtomicU64::new(0)),
                phase: AtomicU64::new(0),
                replies: Mutex::new(replies.into()),
                seen: Mutex::new(vec![]),
                time,
                counter,
            },
            policy: Http {
                attempts: 1,
                ..Http::default()
            },
            bundle: json!({"id":"x","url":ORIGINAL}),
            m: 16 * 1024 * 1024,
            cutoff: None,
        }
    }
    pub fn execute(&self) -> (StageEnd, Stage<ReplyBody>) {
        self.with_fs(Arc::new(unityrs_batch::spool::Host))
    }
    pub fn with_fs(
        &self,
        fs: Arc<dyn unityrs_batch::spool::FileSystem>,
    ) -> (StageEnd, Stage<ReplyBody>) {
        let validated = stage_support::from_bundles(std::slice::from_ref(&self.bundle), &[]);
        let pool = PermitPool::new(1).unwrap();
        let mut permit = pool.acquire().unwrap();
        let file = OwnedFile::create(self.scratch.join("i0"), self.counter.clone(), fs).unwrap();
        let mut stage = Stage::new(file, &permit).unwrap();
        let cx = Context {
            clock: self.time.clone(),
            control: self.stop.clone(),
            cutoff: self.cutoff,
        };
        let end = run(self.time.drive(
            async {
                stage
                    .run(
                        0,
                        validated.spec.bundle(0).unwrap(),
                        self.m,
                        &self.policy,
                        &(),
                        &self.peer,
                        &mut permit,
                        &tokio::runtime::Handle::current(),
                        &cx,
                    )
                    .await
            },
            || pool.snapshot().outstanding,
        ));
        let _ = run(stage.quiesce());
        assert_eq!(pool.snapshot().outstanding, 0);
        assert_eq!(permit.charged(), unityrs_batch::stage::url::STAGE_ENVELOPE);
        (end, stage)
    }
    pub fn requests(&self) -> usize {
        self.peer.seen.lock().unwrap().len()
    }
}
#[allow(clippy::unnecessary_wraps)]
pub fn download(detail: Detail) -> Option<unityrs_batch::stage::Failure> {
    Some(unityrs_batch::stage::Failure::Data(
        unityrs_batch::stage::DataFailure::DownloadFailed(detail),
    ))
}
pub mod disk;
