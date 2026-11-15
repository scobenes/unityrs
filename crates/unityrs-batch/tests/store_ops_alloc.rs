//! Count engine allocations without retaining a test peer's copy of the upload.
mod store_support;
use hyper::{Request, body::Body};
use std::{
    io,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use unityrs_batch::{
    alloc::{self, Counting},
    http::{
        self, Payload, Response,
        permit::{OperationToken, PermitPool, RequestPermit},
    },
    manifest::{Family, chunks::Destination as _},
    phase::{Account, Buffer},
    store::{
        Kind,
        attempt::{Tries, TrySlot},
        client::{self, Settings},
        connector::{Connector, Transport},
        rows,
    },
};
use url::Url;
#[global_allocator]
static ALLOCATOR: Counting = Counting(alloc::NoHook);
#[derive(Clone)]
struct Drain {
    reply: Arc<[u8]>,
    calls: Arc<AtomicU64>,
}
impl Transport for Drain {
    type Io = Discard;
    async fn send(
        &self,
        permit: &mut RequestPermit,
        _: &Url,
        request: Request<Payload>,
        _: Duration,
        token: OperationToken,
        connected: &AtomicBool,
    ) -> Result<Response<Discard>, http::Error> {
        drop(token);
        connected.store(true, Ordering::Release);
        self.calls.fetch_add(1, Ordering::AcqRel);
        let length = request.body().size_hint().exact().unwrap();
        http::exchange(
            Discard {
                reply: self.reply.clone(),
                at: 0,
                head: 0,
                body: 0,
                length,
                reader: None,
            },
            permit,
            request,
        )
        .await
    }
}
struct Discard {
    reply: Arc<[u8]>,
    at: usize,
    head: usize,
    body: u64,
    length: u64,
    reader: Option<Waker>,
}
impl AsyncWrite for Discard {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        for &byte in bytes {
            if self.head == 4 {
                self.body += 1;
            } else if b"\r\n\r\n"[self.head] == byte {
                self.head += 1;
            } else {
                self.head = usize::from(byte == b'\r');
            }
        }
        if let Some(wake) = self.reader.take() {
            wake.wake();
        }
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
impl AsyncRead for Discard {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.head != 4 || self.body < self.length {
            self.reader = Some(cx.waker().clone());
            return Poll::Pending;
        }
        let count = (self.reply.len() - self.at).min(buffer.remaining());
        buffer.put_slice(&self.reply[self.at..self.at + count]);
        self.at += count;
        Poll::Ready(Ok(()))
    }
}
fn setup(body: &[u8]) -> (client::Store, Arc<AtomicU64>) {
    let url = store_support::endpoint();
    let settings = Settings {
        bucket: "bucket",
        region: "us-east-1",
        endpoint: Some(&url),
        path_style: true,
    };
    let head = format!(
        "HTTP/1.1 200 OK\r\nETag: \"tag\"\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let mut reply = head.into_bytes();
    reply.extend_from_slice(body);
    let calls = Arc::new(AtomicU64::new(0));
    let connector = Connector::new(
        Drain {
            reply: reply.into(),
            calls: calls.clone(),
        },
        client::origin(&settings).unwrap(),
        Duration::from_secs(2),
    );
    let provider =
        store_support::credentials(&"K".repeat(128), &"S".repeat(128), Some(&"T".repeat(16384)));
    (
        client::build(&settings, None, &provider, &connector).unwrap(),
        calls,
    )
}
#[test]
fn finisher_rows_larger_than_one_chunk_stay_inside_twenty_four_mib() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (store, calls) = setup(b"");
    let pool = PermitPool::new(1).unwrap();
    let slot = TrySlot::new(pool.acquire().unwrap(), Kind::Put);
    let stop = AtomicBool::new(false);
    let unsettled = AtomicU64::new(0);
    let tries = Tries {
        policy: store_support::policy(1),
        control: &stop,
        unsettled: &unsettled,
    };
    let account = Account::new(24 << 20);
    let (result, measured) = alloc::measured(|| {
        let mut destination = rows::Destination {
            runtime: &runtime,
            request: unityrs_batch::store::put::Request::new(
                &store,
                &tries,
                &slot,
                tokio::time::Instant::now() + Duration::from_secs(5),
            ),
            account: &account,
            prefix: "",
            run_id: "run",
            attempt: 1,
        };
        for (chunk, size) in [16 * 1024 * 1024, 3].into_iter().enumerate() {
            let mut bytes = Buffer::new(account.meter(), size).unwrap();
            bytes.fill(b'\n');
            assert!(
                destination
                    .publish_owned(Family::Outputs, chunk as u64, bytes)?
                    .is_none()
            );
        }
        Ok::<_, unityrs_batch::manifest::WriteError>(())
    });
    result.unwrap();
    assert_eq!(calls.load(Ordering::Acquire), 2);
    assert_eq!(account.snapshot().input.used, 0);
    assert!(measured.peak < 24 * 1024 * 1024, "{}", measured.peak);
    eprintln!("row chunks counted peak {}", measured.peak);
}
#[test]
#[cfg(feature = "test-hooks")]
fn maximum_escaped_complete_at_key_token_and_part_id_caps_fits_the_reservation() {
    use object_store::path::Path;
    use unityrs_batch::{
        spool::journal::Journal,
        store::{
            etag::Etag,
            multipart::{PartBudget, Target, Transfer},
            put::{PART_SIZE, Request},
        },
    };
    use unityrs_testkit::scratch::Scratch;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let body=b"<CompleteMultipartUploadResult><Bucket>bucket</Bucket><Key>k</Key><ETag>\"tag\"</ETag></CompleteMultipartUploadResult>";
    let (store, calls) = setup(body);
    let pool = PermitPool::new(1).unwrap();
    let slot = TrySlot::new(pool.acquire().unwrap(), Kind::Complete);
    let stop = AtomicBool::new(false);
    let unsettled = AtomicU64::new(0);
    let tries = Tries {
        policy: store_support::policy(1),
        control: &stop,
        unsettled: &unsettled,
    };
    let scratch = Scratch::new("complete-alloc");
    let journal = Journal::new(scratch.path()).unwrap();
    let path = Path::parse("k".repeat(1024)).unwrap();
    let id = "u".repeat(1024);
    let created = format!(
        "<InitiateMultipartUploadResult><Bucket>bucket</Bucket><Key>k</Key><UploadId>{id}</UploadId></InitiateMultipartUploadResult>"
    );
    let (create_store, create_calls) = setup(created.as_bytes());
    let account = Account::new(64 << 20);
    let part_budget = PartBudget::new();
    let endpoint = store_support::endpoint();
    let request = Request::new(
        &store,
        &tries,
        &slot,
        tokio::time::Instant::now() + Duration::from_secs(5),
    );
    let mut transfer = runtime
        .block_on(Transfer::start(
            &Request::new(&create_store, &tries, &slot, request.deadline),
            &account,
            &part_budget,
            &journal,
            Target {
                key: &path,
                endpoint: Some(endpoint.as_str()),
                bucket: "bucket",
            },
            10000 * PART_SIZE as u64,
        ))
        .unwrap();
    // Each 100-byte ETag requires the longest partial-XML escape (&amp;).
    transfer.preload_parts_for_test(Etag::new(&[b'&'; 100]).unwrap());
    assert_eq!(create_calls.load(Ordering::Acquire), 1);
    // The part table uses its separate C_net budget; this account records
    // only the charge taken by production's completion path below.
    assert_eq!(account.snapshot().input.peak, 0);
    let (done, measured) =
        alloc::measured(|| runtime.block_on(transfer.complete(&request, b"tag")));
    let done = done.unwrap();
    assert!(!done.verify_by_size);
    assert_eq!(calls.load(Ordering::Acquire), 1);
    assert!(journal.reader().unwrap().next().unwrap().is_none());
    let charged = account.snapshot().input.peak;
    assert_eq!(account.snapshot().input.used, 0);
    assert_eq!(part_budget.used(), 0);
    assert!(
        measured.peak <= charged,
        "completion allocation peak {} exceeds production charge {charged}",
        measured.peak,
    );
    eprintln!(
        "maximum escaped complete counted peak {}, production charge {charged}",
        measured.peak
    );
}
