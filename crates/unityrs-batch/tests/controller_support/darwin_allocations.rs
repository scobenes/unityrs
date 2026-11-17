//! Private mutexes created per connection/response by the pinned dependencies.
//! Retained reactor and scheduler locks are primed at startup instead.
use super::{Exclusion, Platform};

const NOTIFY: &str = "<tokio::loom::std::mutex::Mutex<tokio::util::linked_list::LinkedList<tokio::sync::notify::Waiter>>>::lock";
const DISPATCH_DROP: &str = "<tokio::sync::mpsc::chan::Rx<hyper::client::dispatch::Envelope<http::request::Request<unityrs_batch::http::request::Payload>, http::response::Response<hyper::body::incoming::Incoming>>, tokio::sync::mpsc::unbounded::Semaphore> as core::ops::drop::Drop>::drop";

pub const INVENTORY: &[Exclusion] = &[
    // Each fresh ScheduledIo owns its waiters mutex. Tokio 1.53.3 exposes
    // neither reusable registrations nor caller-supplied mutex storage.
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::sys::reset::RegisteredSocket::connected"],
        platform: Platform::Darwin,
        frame: Some(
            "<tokio::loom::std::mutex::Mutex<tokio::runtime::io::scheduled_io::Waiters>>::lock",
        ),
        via: &["<tokio::io::async_fd::AsyncFd<unityrs_batch::sys::reset::Socket>>::async_io"],
        name: "Darwin per-registration Tokio socket waiters mutex",
    },
    // Hyper creates a new unbounded dispatch channel for each handshake.
    // Its private notify_rx_closed is first locked at receiver close/drop;
    // SendRequest exposes neither the Notify nor channel storage injection.
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::store::put::Request::bytes"],
        platform: Platform::Darwin,
        frame: Some(NOTIFY),
        via: &["<object_store::aws::AmazonS3 as object_store::ObjectStore>::put_opts"],
        name: "Darwin dispatch close Notify mutex during store PUT",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::store::head::Write::publish"],
        platform: Platform::Darwin,
        frame: Some(NOTIFY),
        via: &["<object_store::aws::AmazonS3 as object_store::ObjectStore>::put_opts"],
        name: "Darwin dispatch close Notify mutex during store head PUT",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::store::abort::upload"],
        platform: Platform::Darwin,
        frame: Some(NOTIFY),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::abort_multipart",
        ],
        name: "Darwin dispatch close Notify mutex during store abort",
    },
    Exclusion {
        scopes: &["store-multipart"],
        functions: &["unityrs_batch::store::multipart::Transfer::part"],
        platform: Platform::Darwin,
        frame: Some(NOTIFY),
        via: &[
            "<object_store::aws::AmazonS3 as object_store::multipart::MultipartStore>::put_part",
        ],
        name: "Darwin dispatch close Notify mutex during store multipart part",
    },
    Exclusion {
        scopes: super::STORE,
        functions: &["unityrs_batch::http::response::Response::poll_frame"],
        platform: Platform::Darwin,
        frame: Some(NOTIFY),
        via: &[DISPATCH_DROP],
        name: "Darwin dispatch close Notify mutex at response completion",
    },
    // Hyper 1.12.0 allocates a fresh private body channel for each nonempty
    // Incoming response. Its State mutex and channel APIs are all private.
    Exclusion {
        scopes: super::STORE,
        functions: &[super::EXCHANGE],
        platform: Platform::Darwin,
        frame: Some("<hyper::body::chan::Sender>::poll_ready"),
        via: &[super::DRIVER],
        name: "Darwin response body channel State mutex",
    },
];
