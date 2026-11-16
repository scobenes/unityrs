//! Addendum 3 / CR10: exact allocating functions, independent of allocation
//! ordinals and sizes. Each entry must still abort in every named scope on
//! its platform. Every other refusal must return `host_memory`/74.

#[path = "darwin_allocations.rs"]
mod darwin_allocations;
#[path = "store_allocations.rs"]
mod store_allocations;
#[path = "store_multipart_allocations.rs"]
mod store_multipart_allocations;
#[path = "store_parsing_allocations.rs"]
mod store_parsing_allocations;

pub fn inventory() -> impl Iterator<Item = &'static Exclusion> {
    INVENTORY
        .iter()
        .chain(darwin_allocations::INVENTORY)
        .chain(store_allocations::INVENTORY)
        .chain(store_parsing_allocations::INVENTORY)
        .chain(store_multipart_allocations::INVENTORY)
}

#[derive(Clone, Copy)]
pub enum Platform {
    All,
    Darwin,
}

pub struct Exclusion {
    pub scopes: &'static [&'static str],
    /// The innermost function of ours ([`innermost`]) the refusal is
    /// under: any one of these.
    pub functions: &'static [&'static str],
    pub platform: Platform,
    pub name: &'static str,
    /// A third-party allocation: what its allocating frame (the first above
    /// the allocator outside std's library, [`allocating`]) must name,
    /// since `innermost` keys on our caller and skips the registry's frames.
    pub frame: Option<&'static str>,
    /// For a third-party allocation, the chain from our function: the frame
    /// it calls ([`callee`], std's library skipped) must name one of these,
    /// the third party's entry point. So our own call to a listed allocator
    /// (a `oneshot::channel` of ours) is not admitted as the library's.
    pub via: &'static [&'static str],
}

impl Exclusion {
    pub fn applies(&self, scope: &str) -> bool {
        let applies = match scope {
            // Scheduled variants exercise the same inventoried plain-HTTP
            // sites. Each variant must reach every entry it claims itself;
            // none borrows staleness evidence from another scope.
            "url-head-split" | "url-body-split" => self.scopes.contains(&"url"),
            "url-stop-before" | "url-stop-inside" | "url-stop-after" => {
                self.scopes.contains(&"url")
                    && self.name != "tokio spawn_blocking task cell"
                    && self.name != "tokio deregistration push at a response's socket drop"
            }
            "url-cut-read" => self.scopes.contains(&"url") || self.scopes == CUT,
            _ => self.scopes.contains(&scope),
        };
        applies
            && match self.platform {
                Platform::All => true,
                Platform::Darwin => cfg!(target_vendor = "apple"),
            }
    }
}

pub const INVENTORY: &[Exclusion] = &[
    Exclusion {
        scopes: STORE,
        functions: &["unityrs_batch::publish::store::transfer::run"],
        platform: Platform::All,
        frame: Some("object_store::path"),
        via: &["object_store::path::Path"],
        name: "object-store transfer key parse",
    },
    Exclusion {
        scopes: STORE,
        functions: &["unityrs_batch::sink::store::journal::JournalBackend::abort"],
        platform: Platform::All,
        frame: Some("object_store::path"),
        via: &["object_store::path::Path"],
        name: "object-store finisher abort key parse",
    },
    Exclusion {
        scopes: &["peek", "attempt", "complete"],
        functions: &["unityrs_core::budget::charged_bytes"],
        platform: Platform::All,
        frame: None,
        via: &[],
        name: "Bytes::from_owner owner box",
    },
    Exclusion {
        scopes: &["peek", "attempt", "complete"],
        functions: &["unityrs_core::files::RawFile::with_reader"],
        platform: Platform::All,
        frame: None,
        via: &[],
        name: "RawFile Arc",
    },
    Exclusion {
        scopes: &["peek"],
        functions: &["unityrs_export::ngui::classify::Reason::plain"],
        platform: Platform::All,
        frame: None,
        via: &[],
        name: "owned NGUI reason",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        functions: &["unityrs_core::files::bundle::BundleFile::open_at_depth"],
        platform: Platform::All,
        frame: None,
        via: &[],
        name: "parsed-container cyclic Arc",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        functions: &["unityrs_core::env::Environment::new"],
        platform: Platform::All,
        frame: None,
        via: &[],
        name: "Environment and DepCache Arcs",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        functions: &["unityrs_core::limits::meter::Meter::with_decode_work"],
        platform: Platform::All,
        frame: None,
        via: &[],
        name: "meter and account Arcs",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        functions: &["unityrs_core::env::Environment::try_load_dep"],
        platform: Platform::All,
        frame: None,
        via: &[],
        name: "dependency Arc",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        functions: &["unityrs_core::env::registry::Registry::new_slot"],
        platform: Platform::All,
        frame: None,
        via: &[],
        name: "registry path owner and Slot Arcs in prepare",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        functions: &["unityrs_core::env::registry::Registry::apply"],
        platform: Platform::All,
        frame: None,
        via: &[],
        name: "registry maps",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        functions: &["unityrs_core::files::bundle::index::RunIndex::publish"],
        platform: Platform::All,
        frame: None,
        via: &[],
        name: "run-index BTreeMap",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        functions: &["unityrs_core::env::registry::lru::Lru::use_key"],
        platform: Platform::All,
        frame: None,
        via: &[],
        name: "dependency-cache HashMap in touch_paid",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        functions: &["unityrs_export::ngui::read::head_rows"],
        platform: Platform::All,
        frame: None,
        via: &[],
        name: "Reader head-row Vecs",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        functions: &["unityrs_core::typetree::rows::NodeRow::new"],
        platform: Platform::All,
        frame: None,
        via: &[],
        name: "Reader head-row type and name copies",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        functions: &["unityrs_core::limits::meter::Meter::unlimited"],
        platform: Platform::All,
        frame: None,
        via: &[],
        name: "head-tree meter Arcs through NodeList::trusted",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        functions: &["unityrs_core::typetree::tree::Tree::compile_inner"],
        platform: Platform::All,
        frame: None,
        via: &[],
        name: "compiled Tree Arc",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        functions: &["unityrs_export::ngui::keys::Template::clone"],
        platform: Platform::All,
        frame: None,
        via: &[],
        name: "Attempt::plan template piece Vec clone",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        functions: &["unityrs_export::ngui::keys::Piece::clone"],
        platform: Platform::All,
        frame: None,
        via: &[],
        name: "Attempt::plan template text Vec clones",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        functions: &["unityrs_core::files::bundle::index::RunIndex::mutex"],
        platform: Platform::Darwin,
        frame: None,
        via: &[],
        name: "Darwin lazy mutex box, charged and initialized in RunIndex::mutex",
    },
    Exclusion {
        scopes: &["attempt", "complete"],
        functions: &["unityrs_core::env::held::lock"],
        platform: Platform::Darwin,
        frame: None,
        via: &[],
        name: "Darwin lazy mutex box in DepCache::touch_paid",
    },
    // The one admitted third-party frame: tokio's `spawn_blocking`
    // allocates each task's cell with `Box::new`, which aborts on refusal.
    // There is no fallible submission in tokio's API, and a pool task is how
    // every blocking write is kept off the controller (`docs/plans/batch.md`
    // 2.11), so the refusal is the documented abort (A13: status 134 from
    // step 4). Everything else `Execution::spawn_named` (under `spawn`)
    // allocates is fallible; the prewarm leaves tokio no thread or queue to grow.
    Exclusion {
        scopes: SPAWNING,
        functions: &["unityrs_batch::execution::Execution::spawn_named"],
        platform: Platform::All,
        frame: Some("tokio::runtime::task::core::Cell"),
        via: &[SPAWN_BLOCKING],
        name: "tokio spawn_blocking task cell",
    },
    // A networked run's staging (`docs/plans/batch.md` 2.5, *URL staging in
    // the controller*): a request's lookup and chunk write enter the pool
    // through `execution::blocking`, the same admitted task cell.
    Exclusion {
        scopes: REQUEST,
        functions: &["unityrs_batch::execution::blocking"],
        platform: Platform::All,
        frame: Some("tokio::runtime::task::core::Cell"),
        via: &[SPAWN_BLOCKING],
        name: "tokio spawn_blocking task cell for a lookup or a chunk write",
    },
    // Each request's header map: http's map has no fallible constructor
    // (`try_reserve` reports only its size limit), and hyper consumes the
    // map with its request, so no storage made before the head can serve a
    // later try or hop. Its values are copied into fallible storage first;
    // its names are phase 1's, whose clones count a reference.
    Exclusion {
        scopes: &[
            "url",
            "url-retry",
            "url-redirect",
            "url-drip",
            "url-stop",
            "url-https",
            "url-cut",
            "url-noreply",
            "url-https-cut",
            "url-fail",
        ],
        functions: &["unityrs_batch::stage::headers::prepare"],
        platform: Platform::All,
        frame: Some("http::header::map::HeaderMap"),
        via: &["<http::header::map::HeaderMap>::try_reserve"],
        name: "http HeaderMap storage of a request",
    },
    // A redirect's join: url's parser has no fallible storage.
    Exclusion {
        scopes: &["url-redirect"],
        functions: &["unityrs_batch::stage::url::redirect"],
        platform: Platform::All,
        frame: Some("url::"),
        via: &["<url::Url>::join"],
        name: "url join of a redirect's Location",
    },
    // Tokio's reactor: a socket's registration (`Arc<ScheduledIo>`), and
    // the deregistration's `Vec::push` when a response's socket drops: at
    // a body's end, when a try or a hop drops its response, or when an
    // exchange's connection fails before its head.
    Exclusion {
        scopes: REQUEST,
        functions: &["unityrs_batch::sys::reset::Socket::register"],
        platform: Platform::All,
        frame: Some("RegistrationSet>::allocate"),
        via: &["AsyncFd<unityrs_batch::sys::reset::Socket>>::register"],
        name: "tokio ScheduledIo of a socket's registration",
    },
    Exclusion {
        scopes: URL,
        functions: &[
            EXCHANGE,
            "unityrs_batch::http::response::Response::poll_frame",
            "unityrs_batch::stage::url::Stage::download",
            "unityrs_batch::stage::url::Stage::try_download",
        ],
        platform: Platform::All,
        frame: Some("RegistrationSet>::deregister"),
        via: &["AsyncFd<unityrs_batch::sys::reset::Socket> as core::ops::drop::Drop>::drop"],
        name: "tokio deregistration push at a response's socket drop",
    },
    // hyper's client connection, beside the TLS stack below: its
    // handshake's dispatch channel (a tokio unbounded mpsc and `want`'s
    // `Arc`), write buffer and dispatcher; the oneshot of `send_request`;
    // and, as its driver reads, its read buffer's growth and splits, the
    // body channel's `Arc`, and a non-canonical reason phrase's extension.
    Exclusion {
        scopes: REQUEST,
        functions: &[EXCHANGE],
        platform: Platform::All,
        frame: Some("hyper::client::dispatch::Envelope"),
        via: &[HANDSHAKE],
        name: "hyper dispatch::channel mpsc",
    },
    Exclusion {
        scopes: REQUEST,
        functions: &[EXCHANGE],
        platform: Platform::All,
        frame: Some("want::new"),
        via: &[HANDSHAKE],
        name: "hyper dispatch::channel want Arc",
    },
    Exclusion {
        scopes: REQUEST,
        functions: &[EXCHANGE],
        platform: Platform::All,
        frame: Some("hyper::proto::h1::io::WriteBuf"),
        via: &[HANDSHAKE],
        name: "hyper WriteBuf::new",
    },
    Exclusion {
        scopes: REQUEST,
        functions: &[EXCHANGE],
        platform: Platform::All,
        frame: Some("hyper::proto::h1::dispatch::Dispatcher"),
        via: &[HANDSHAKE],
        name: "hyper http1 handshake's dispatcher box",
    },
    Exclusion {
        scopes: REQUEST,
        functions: &[EXCHANGE],
        platform: Platform::All,
        frame: Some("tokio::sync::oneshot::channel"),
        via: &[
            "hyper::client::conn::http1::SendRequest<unityrs_batch::http::request::Payload>>::send_request",
        ],
        name: "hyper send_request oneshot",
    },
    Exclusion {
        scopes: REQUEST,
        functions: &[EXCHANGE, POLL_FRAME],
        platform: Platform::All,
        frame: Some("bytes::bytes_mut::BytesMut>::reserve_inner"),
        via: &[DRIVER],
        name: "hyper read buffer BytesMut reserve",
    },
    Exclusion {
        scopes: REQUEST,
        functions: &[EXCHANGE, POLL_FRAME],
        platform: Platform::All,
        frame: Some("bytes::bytes_mut::BytesMut>::promote_to_shared"),
        via: &[DRIVER],
        name: "hyper read buffer BytesMut promote_to_shared",
    },
    Exclusion {
        scopes: URL,
        functions: &[EXCHANGE],
        platform: Platform::All,
        frame: Some("hyper::body::chan::channel"),
        via: &[DRIVER],
        name: "hyper body channel Arc",
    },
    Exclusion {
        scopes: STATUS,
        functions: &[EXCHANGE],
        platform: Platform::All,
        frame: Some("hyper::ext::h1_reason_phrase::ReasonPhrase"),
        via: &[DRIVER],
        name: "hyper ReasonPhrase extension insert",
    },
    Exclusion {
        scopes: STATUS,
        functions: &[EXCHANGE],
        platform: Platform::All,
        frame: Some("hashbrown::alloc::inner::do_alloc"),
        via: &[DRIVER],
        name: "hyper ReasonPhrase extension map",
    },
    Exclusion {
        scopes: STATUS,
        functions: &[EXCHANGE],
        platform: Platform::All,
        frame: Some("<bytes::bytes::Bytes>::copy_from_slice"),
        via: &[DRIVER],
        name: "hyper ReasonPhrase copy",
    },
    // A transport failure's own storage, as the driver meets a peer's
    // reset or close mid-reply: hyper's error box, its `io::Error` cause,
    // and the decoder's incomplete-body `io::Error`. hyper builds these
    // with `Box::new` (A13).
    Exclusion {
        scopes: CUT,
        functions: &[EXCHANGE, POLL_FRAME],
        platform: Platform::All,
        frame: Some("<hyper::error::Error>::new"),
        via: &[DRIVER],
        name: "hyper error box of a transport failure",
    },
    Exclusion {
        scopes: CUT,
        functions: &[EXCHANGE, POLL_FRAME],
        platform: Platform::All,
        frame: Some("<hyper::error::Error>::with"),
        via: &[DRIVER],
        name: "hyper error cause of a transport failure",
    },
    Exclusion {
        scopes: CUT,
        functions: &[POLL_FRAME],
        platform: Platform::All,
        frame: Some("<hyper::proto::h1::decode::Decoder>::decode"),
        via: &[DRIVER],
        name: "hyper incomplete-body io::Error",
    },
    // The run's teardown, after its finisher wrote the final head: the
    // parked owners' drop deregisters the wake owner's descriptor, and the
    // runtime's drop shuts its reactor's registrations down. Both are
    // tokio's infallible `Vec` growth; the result is already written, so
    // the abort (A13) changes no outcome.
    Exclusion {
        scopes: &["url-fail"],
        functions: &["unityrs_batch::sys::shared::Shared::drop"],
        platform: Platform::All,
        frame: Some("RegistrationSet>::deregister"),
        via: &["AsyncFd<unityrs_batch::sys::wake::WakeOwner> as core::ops::drop::Drop>::drop"],
        name: "tokio deregistration push at the wake owner's drop",
    },
    Exclusion {
        scopes: &["url-fail", "store", "store-resume", "store-multipart"],
        functions: &[
            "unityrs_batch::execution::Execution::drop",
            "unityrs_batch::sink::store::Runtime::drop",
        ],
        platform: Platform::All,
        frame: Some("RegistrationSet>::shutdown"),
        via: &["<tokio::runtime::runtime::Runtime as core::ops::drop::Drop>::drop"],
        name: "tokio reactor shutdown at the runtime's drop",
    },
    // rustls (and ring under it) over `https`: the client connection's
    // setup, the handshake, and each record read or written after it.
    // rustls's API has no fallible allocation (A13).
    Exclusion {
        scopes: TLS,
        functions: &["unityrs_batch::http::connect::Connector::secure"],
        platform: Platform::All,
        frame: Some("rustls"),
        via: &["<tokio_rustls::client::TlsConnector>::connect"],
        name: "rustls client connection",
    },
    Exclusion {
        scopes: TLS,
        functions: &["unityrs_batch::http::connect::Connector::secure"],
        platform: Platform::All,
        frame: Some("rustls"),
        via: &[TLS_HANDSHAKE],
        name: "rustls handshake",
    },
    Exclusion {
        scopes: TLS,
        functions: &["unityrs_batch::http::connect::Connector::secure"],
        platform: Platform::All,
        frame: Some("<ring::"),
        via: &[TLS_HANDSHAKE],
        name: "ring big-integer limbs of the handshake's signature check",
    },
    Exclusion {
        scopes: TLS,
        functions: &["unityrs_batch::http::io::Stream::poll_read"],
        platform: Platform::All,
        frame: Some("rustls"),
        via: &[
            "<tokio_rustls::client::TlsStream<unityrs_batch::sys::reset::RegisteredSocket> as tokio::io::async_read::AsyncRead>::poll_read",
        ],
        name: "rustls record read",
    },
    Exclusion {
        scopes: TLS,
        functions: &[
            "unityrs_batch::http::io::Stream::poll_write",
            "unityrs_batch::http::io::Stream::poll_write_vectored",
        ],
        platform: Platform::All,
        frame: Some("rustls"),
        via: &[
            "<tokio_rustls::client::TlsStream<unityrs_batch::sys::reset::RegisteredSocket> as tokio::io::async_write::AsyncWrite>::poll_write",
        ],
        name: "rustls record write",
    },
];
/// The scopes whose armed span submits pool tasks.
const SPAWNING: &[&str] = &[
    "stage",
    "peek",
    "attempt",
    "failure",
    "complete",
    "archive",
    "url",
    "url-retry",
    "url-redirect",
    "url-drip",
    "url-stop",
    "url-https",
    "url-cut",
    "url-noreply",
    "url-https-cut",
    "url-fail",
];
const STORE: &[&str] = &["store", "store-resume", "store-multipart"];
const SPAWN_BLOCKING: &str = "<tokio::runtime::handle::Handle>::spawn_blocking";
const EXCHANGE: &str = "unityrs_batch::http::response::exchange_charged";
const POLL_FRAME: &str = "unityrs_batch::http::response::Response::poll_frame";
const HANDSHAKE: &str = "<hyper::client::conn::http1::Builder>::handshake";
/// The driver's poll: hyper's connection, reading.
const DRIVER: &str = "<hyper::client::conn::http1::Connection<hyper_util::rt::tokio::TokioIo<unityrs_batch::http::io::Stream>, unityrs_batch::http::request::Payload> as core::future::future::Future>::poll";
const TLS_HANDSHAKE: &str = "<tokio_rustls::client::Connect<unityrs_batch::sys::reset::RegisteredSocket> as core::future::future::Future>::poll";
/// Every networked scope (`url_scopes.rs`).
const URL: &[&str] = &[
    "store",
    "store-resume",
    "store-multipart",
    "url",
    "url-retry",
    "url-redirect",
    "url-drip",
    "url-stop",
    "url-https",
    "url-cut",
    "url-noreply",
    "url-https-cut",
];
/// The scopes whose request is sent, the failing input's among them.
const REQUEST: &[&str] = &[
    "store",
    "store-resume",
    "store-multipart",
    "url",
    "url-retry",
    "url-redirect",
    "url-drip",
    "url-stop",
    "url-https",
    "url-cut",
    "url-noreply",
    "url-https-cut",
    "url-fail",
];
/// The scopes over `https`.
const TLS: &[&str] = &["url-https", "url-https-cut"];
/// The scopes whose peer answers a try with a non-canonical status line.
const STATUS: &[&str] = &["url-retry", "url-cut", "url-fail"];
/// The scopes whose peer resets or closes a connection mid-reply.
const CUT: &[&str] = &["url-cut", "url-noreply", "url-https-cut"];

impl Exclusion {
    /// Whether a refusal's trace, whose innermost owner is `function`, is
    /// this entry's: ours by name, a third party's by name and frame.
    pub fn admits(&self, function: Option<&str>, trace: &str) -> bool {
        // Darwin dependency mutex entries spell the complete type/method.
        // Preserve generic arguments and reject method-name prefix collisions.
        let exact = |symbol: &str, key: &str| {
            symbol.strip_prefix(key).is_some_and(|tail| {
                tail.is_empty() || tail.starts_with("::<") || tail.starts_with("::{closure")
            })
        };
        let darwin = matches!(self.platform, Platform::Darwin);
        function.is_some_and(|f| self.functions.contains(&f))
            && match self.frame {
                Some(frame) => {
                    allocating(trace).is_some_and(|f| {
                        if darwin {
                            exact(&f, frame)
                        } else {
                            f.contains(frame)
                        }
                    }) && callee(trace).is_some_and(|c| {
                        let key = symbol_key(&c);
                        self.via.iter().any(|v| {
                            if darwin {
                                exact(&c, v)
                            } else {
                                c.contains(v) || key.as_deref() == Some(*v)
                            }
                        })
                    })
                }
                None => self
                    .functions
                    .iter()
                    .all(|f| f.starts_with("unityrs_core::") || f.starts_with("unityrs_export::")),
            }
    }
}

/// Only function symbols count. Generic arguments may mention our types in
/// std/alloc functions; trait implementations supplied by std (e.g. slice's
/// `ConvertVec` for `Piece`) are also foreign, as their source location shows.
/// Cargo's sweep profile `debug = "full"` preserves inline owner/callee frames.
pub fn innermost(stderr: &str) -> Option<String> {
    let trace = stderr.split_once("REFUSED_ALLOCATION ")?.1;
    let (trace, _) = trace.split_once("END_REFUSED_ALLOCATION")?;
    let mut lines = trace.lines().peekable();
    while let Some(line) = lines.next() {
        let Some((index, symbol)) = line.trim().split_once(": ") else {
            continue;
        };
        if index.parse::<usize>().is_err() {
            continue;
        }
        if lines.peek().is_some_and(|source| {
            let source = source.trim();
            source.starts_with("at ")
                && (source.contains("/rustc/") || source.contains("/.cargo/registry/"))
        }) {
            continue;
        }
        let Some(function) = function_key(symbol) else {
            continue;
        };
        // These are the test's allocator plumbing, not the allocation's
        // caller. No other batch/CLI function may be skipped or excluded.
        if function.starts_with("unityrs_batch::alloc::Counting::")
            || function == "unityrs_batch::alloc::no_unwind"
        {
            continue;
        }
        return Some(function);
    }
    None
}

/// The allocation's own frame: the first above the allocator that is not
/// std's library code (`Box::new`, `Arc::new`, `Vec::with_capacity`, ...):
/// a third party's function, or ours.
pub fn allocating(stderr: &str) -> Option<String> {
    let trace = stderr.split_once("REFUSED_ALLOCATION ")?.1;
    let (trace, _) = trace.split_once("END_REFUSED_ALLOCATION")?;
    let mut lines = trace.lines().peekable();
    let mut above = false;
    while let Some(line) = lines.next() {
        let Some((index, symbol)) = line.trim().split_once(": ") else {
            continue;
        };
        if index.parse::<usize>().is_err() {
            continue;
        }
        if !above {
            // The counting allocator's `alloc`, `alloc_zeroed` or `realloc`.
            above = symbol.contains("as core::alloc::global::GlobalAlloc>::")
                && !symbol.contains("{closure");
            continue;
        }
        // std's library code, and the `#[global_allocator]` shim (`__rustc::
        // __rust_alloc`), which its test file's location names.
        if symbol.starts_with("__rustc::")
            || lines.peek().is_some_and(|source| {
                source.trim().starts_with("at ") && source.contains("/rustc/")
            })
        {
            continue;
        }
        return Some(symbol.to_owned());
    }
    None
}

/// The frame [`innermost`]'s function calls: the nearest frame below it
/// (toward the allocator) outside std's library and the allocator's
/// plumbing. `None` when ours allocates directly.
pub fn callee(stderr: &str) -> Option<String> {
    let trace = stderr.split_once("REFUSED_ALLOCATION ")?.1;
    let (trace, _) = trace.split_once("END_REFUSED_ALLOCATION")?;
    let mut lines = trace.lines().peekable();
    let mut above = false;
    let mut last = None;
    while let Some(line) = lines.next() {
        let Some((index, symbol)) = line.trim().split_once(": ") else {
            continue;
        };
        if index.parse::<usize>().is_err() {
            continue;
        }
        if !above {
            above = symbol.contains("as core::alloc::global::GlobalAlloc>::")
                && !symbol.contains("{closure");
            continue;
        }
        let source = lines
            .peek()
            .map(|s| s.trim())
            .filter(|s| s.starts_with("at "));
        if symbol.starts_with("__rustc::") || source.is_some_and(|s| s.contains("/rustc/")) {
            continue;
        }
        let ours = function_key(symbol).is_some()
            && !source.is_some_and(|s| s.contains("/.cargo/registry/"));
        if ours {
            return last;
        }
        last = Some(symbol.to_owned());
    }
    None
}

fn function_key(symbol: &str) -> Option<String> {
    let key = symbol_key(symbol)?;
    [
        "unityrs_core::",
        "unityrs_export::",
        "unityrs_batch::",
        "unityrs_cli::",
        "unityrs::",
    ]
    .iter()
    .any(|prefix| key.starts_with(prefix))
    .then_some(key)
}

fn symbol_key(symbol: &str) -> Option<String> {
    let symbol = if let Some(inner) = symbol.strip_prefix('<') {
        let mut depth = 1;
        let end = inner.char_indices().find_map(|(i, c)| {
            match c {
                '<' => depth += 1,
                '>' => depth -= 1,
                _ => {}
            }
            (depth == 0).then_some(i)
        })?;
        let owner = inner[..end].split(" as ").next()?;
        format!("{owner}{}", &inner[end + 1..])
    } else {
        symbol.to_owned()
    };
    // Monomorphizations and closure numbers are not distinct functions.
    let mut depth = 0;
    let mut key = String::new();
    for c in symbol.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            _ if depth == 0 => key.push(c),
            _ => {}
        }
    }
    Some(
        key.split("::{closure")
            .next()?
            .trim_end_matches(':')
            .to_owned(),
    )
}

#[path = "refusals_tests.rs"]
mod tests;
