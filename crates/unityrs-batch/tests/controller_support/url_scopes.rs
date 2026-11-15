//! The CR10 sweep's networked scopes (`controller_alloc.rs`): whole library
//! runs staging two URLs and a local copy, armed at the first dispatch
//! through staging's end. Each scope adds what a try can meet: custom
//! headers (literal and from the environment, in every scope), a retried
//! status with a non-canonical reason, a two-hop redirect, a dripped body,
//! a stop, `https`, a peer that cuts a body or closes without a reply
//! (plain and over TLS), and an input that fails, armed through the run's
//! teardown. A refused run must also keep every connection whose response
//! the origin had not finished open until after its finisher.
use super::{ACTIVE, EXEMPT, SEEN, SITE, Space, publication_support, run_support};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::{SocketAddr, TcpListener},
    os::fd::{AsRawFd, RawFd},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
    },
    time::Duration,
};
use unityrs_batch::{
    controller::{Event, Hooks, Point},
    finish::Finished,
    http::resolve::{Addresses, Resolver},
    lifetime::Stop,
    progress::{Snapshot, live::Service},
    reason::{Code, StopReason},
    run::{Lifetime, Network, Observer, Options},
};

pub const SCOPES: &[&str] = &[
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
/// Every url scope's allocation count follows the peer's timing and the
/// machine's load (how a head or a body splits across reads, a drip's
/// pieces, a TLS record's split, where the stop lands, which read meets a
/// cut), so its sites are keyed by call stack and the scope settles
/// (`sites.rs`): an occurrence a child never meets is unreached there, not
/// a failure, and an entry need only be reached in some scope it names.
pub fn timed(scope: &str) -> bool {
    SCOPES.contains(&scope)
}

#[derive(Clone)]
enum Reply {
    /// The body in chunks of this size, a millisecond apart.
    Ok(usize),
    /// A status line's code and reason, `Retry-After: 0`, no body.
    Status(&'static str),
    Redirect(String),
    /// The head and this many body bytes, then a reset (true) or a FIN.
    Cut(usize, bool),
    /// No reply to the request: a reset (true) or a FIN (false).
    Drop(bool),
}
/// One connection as the origin saw it: whether it wrote its whole reply,
/// and whether the client closed it.
#[derive(Default)]
struct Connection {
    complete: AtomicBool,
    closed: AtomicBool,
}
#[derive(Default)]
struct Origin {
    bodies: Mutex<Vec<(String, Arc<[u8]>)>>,
    scripts: Mutex<Vec<(String, VecDeque<Reply>)>>,
    defaults: Mutex<Vec<(String, Reply)>>,
    connections: Mutex<Vec<Arc<Connection>>>,
}
impl Origin {
    /// Connections the client closed before the origin wrote all of its
    /// reply.
    fn early(&self) -> usize {
        let connections = self.connections.lock().unwrap();
        connections
            .iter()
            .filter(|c| c.closed.load(SeqCst) && !c.complete.load(SeqCst))
            .count()
    }
    fn reply(&self, route: &str) -> (Reply, Arc<[u8]>) {
        let scripted = self
            .scripts
            .lock()
            .unwrap()
            .iter_mut()
            .find(|(r, _)| r == route)
            .and_then(|(_, s)| s.pop_front());
        let reply = scripted
            .or_else(|| {
                let defaults = self.defaults.lock().unwrap();
                defaults
                    .iter()
                    .find(|(r, _)| r == route)
                    .map(|(_, d)| d.clone())
            })
            .unwrap_or(Reply::Ok(1 << 20));
        let bodies = self.bodies.lock().unwrap();
        let body = bodies
            .iter()
            .find(|(r, _)| r == route)
            .map(|(_, b)| b.clone());
        (reply, body.unwrap_or_else(|| Arc::from([])))
    }
    /// One connection, plain or TLS: the request's head, then its route's
    /// reply. A reset sets `socket`'s zero linger: the caller's close
    /// sends it.
    fn serve(&self, stream: &mut impl ReadWrite, connection: &Connection, socket: RawFd) {
        let mut head = Vec::new();
        let mut buffer = [0; 4096];
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(n) => head.extend_from_slice(&buffer[..n]),
            }
        }
        let text = String::from_utf8_lossy(&head).into_owned();
        let path = text.split(' ').nth(1).unwrap_or("");
        let route = path.split('?').next().unwrap_or("");
        let written = match self.reply(route) {
            (Reply::Ok(chunk), body) => write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .and_then(|()| {
                for piece in body.chunks(chunk) {
                    stream.write_all(piece)?;
                    stream.flush()?;
                    if chunk < body.len() {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
                Ok(())
            }),
            (Reply::Status(status), _) => write!(
                stream,
                "HTTP/1.1 {status}\r\nRetry-After: 0\r\nContent-Length: 0\r\n\r\n"
            ),
            (Reply::Redirect(location), _) => write!(
                stream,
                "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n"
            ),
            // The origin's own ends: no client close of these is early.
            (Reply::Cut(count, reset), body) => {
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                )
                .and_then(|()| stream.write_all(&body[..count.min(body.len())]))
                .and_then(|()| stream.flush());
                std::thread::sleep(Duration::from_millis(5));
                connection.complete.store(true, SeqCst);
                if reset {
                    let _ = unityrs_batch::sys::test_set_zero_linger(socket);
                }
                return;
            }
            (Reply::Drop(reset), _) => {
                connection.complete.store(true, SeqCst);
                if reset {
                    let _ = unityrs_batch::sys::test_set_zero_linger(socket);
                }
                return;
            }
        };
        if written.is_ok() {
            connection.complete.store(true, SeqCst);
            let _ = stream.read(&mut buffer);
        }
    }
}
trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}
/// A loopback origin on threads the refusals exempt, one request a
/// connection; with `tls`, the same routes over TLS (the localhost leaf).
/// Its connection records count either kind's closes.
fn listen(origin: &Arc<Origin>, tls: bool) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let state = origin.clone();
    std::thread::spawn(move || {
        EXEMPT.with(|e| e.set(true));
        let config = tls.then(super::http_support::tls::server);
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let (state, config) = (state.clone(), config.clone());
            std::thread::spawn(move || {
                EXEMPT.with(|e| e.set(true));
                let _ = stream.set_read_timeout(Some(unityrs_testkit::guard::DEADLOCK_GUARD));
                let connection = Arc::new(Connection::default());
                state.connections.lock().unwrap().push(connection.clone());
                let socket = stream.as_raw_fd();
                if let Some(config) = config {
                    let session = rustls::ServerConnection::new(config).unwrap();
                    let mut stream = rustls::StreamOwned::new(session, stream);
                    state.serve(&mut stream, &connection, socket);
                } else {
                    state.serve(&mut stream, &connection, socket);
                }
                // The client closed it, or the origin ends it now (a cut's
                // or a dropped reply's reset, by its zero linger).
                connection.closed.store(true, SeqCst);
            });
        }
    });
    address
}
/// `localhost` is the TLS peer; every other host, the plain origin. No
/// allocation: it runs on the engine's blocking pool.
struct Resolve {
    origin: SocketAddr,
    tls: Option<SocketAddr>,
}
impl Resolver for Resolve {
    fn resolve(&self, host: &str, _: u16) -> std::io::Result<Addresses> {
        Ok(Addresses::first([match self.tls {
            Some(tls) if host == "localhost" => tls,
            _ => self.origin,
        }]))
    }
}
struct Environment;
impl unityrs_batch::stage::env::Environment for Environment {
    fn var(&self, name: &str) -> Option<std::ffi::OsString> {
        (name == "UNITYRS_SWEEP_TOKEN").then(|| "from-the-environment".into())
    }
}
/// The stage poll the stop scope's SIGTERM comes at: mid-body (its drip),
/// as a rule.
const STOP_AT: usize = 12;
/// Armed at the first dispatch; off at staging's end (the first peek or a
/// stop's hand-over), unless `through` (until `drive_with` returns).
struct Arm {
    stop: bool,
    through: bool,
    polls: AtomicUsize,
    lifetime: Arc<Lifetime>,
}
impl unityrs_export::ngui::Hooks for Arm {}
impl Hooks for Arm {
    fn event(&self, event: Event) {
        match event {
            Event::StageDispatched(_) => ACTIVE.store(true, SeqCst),
            Event::PeekDispatched(_) | Event::HandOver if !self.through => {
                ACTIVE.store(false, SeqCst);
            }
            _ => {}
        }
    }
    fn point(&self, point: Point) {
        if self.stop
            && matches!(point, Point::StagePoll(_))
            && self.polls.fetch_add(1, SeqCst) == STOP_AT
        {
            self.lifetime.request(Stop::external(StopReason::Sigterm));
        }
    }
}
/// The origin's early closes at the finisher, while the parked owners
/// (a refused or frozen stage among them) are alive.
struct Finish(Arc<Origin>);
static EARLY: OnceLock<usize> = OnceLock::new();
impl Observer for Finish {
    fn finished(&self, _: &Finished) {
        EXEMPT.with(|e| e.set(true));
        let _ = EARLY.set(self.0.early());
        EXEMPT.with(|e| e.set(false));
    }
}
/// The origin (and for `https`, the TLS peer) serving `scope`'s replies,
/// with its `http` policy and the two bundles' URLs.
fn serve(
    scope: &str,
    bodies: &[Vec<u8>; 2],
) -> (
    Arc<Origin>,
    SocketAddr,
    serde_json::Value,
    [String; 2],
    Option<SocketAddr>,
) {
    let origin = Arc::new(Origin::default());
    let address = listen(&origin, false);
    let mut http = serde_json::json!({"attempts": 1});
    let mut tls = None;
    let retries =
        || serde_json::json!({"attempts": 3, "backoff_initial_ms": 0, "backoff_max_ms": 0});
    {
        let mut routes = origin.bodies.lock().unwrap();
        routes.push(("/b0".into(), bodies[0].clone().into()));
        routes.push(("/b1".into(), bodies[1].clone().into()));
        routes.push(("/final".into(), bodies[0].clone().into()));
    }
    let script = |route: &str, replies: Vec<Reply>| {
        origin
            .scripts
            .lock()
            .unwrap()
            .push((route.into(), replies.into()));
    };
    let default = |route: &str, reply: Reply| {
        origin.defaults.lock().unwrap().push((route.into(), reply));
    };
    let mut urls = [0, 1].map(|n| format!("http://{address}/b{n}?sig=a"));
    match scope {
        "url" => {}
        "url-retry" => {
            http = retries();
            script("/b0", vec![Reply::Status("503 X")]);
            script("/b1", vec![Reply::Status("500 X"), Reply::Status("503 X")]);
        }
        "url-redirect" => {
            script("/b0", vec![Reply::Redirect("/hop?q=1".into())]);
            default(
                "/hop",
                Reply::Redirect(format!("http://{address}/final?second=hop")),
            );
        }
        "url-drip" | "url-stop" => {
            default("/b0", Reply::Ok(700));
            default("/b1", Reply::Ok(300));
        }
        "url-https" | "url-https-cut" => {
            let secure = listen(&origin, true);
            urls[0] = format!("https://localhost:{}/b0", secure.port());
            tls = Some(secure);
            if scope == "url-https-cut" {
                http = retries();
                script(
                    "/b0",
                    vec![Reply::Cut(bodies[0].len() / 3, true), Reply::Drop(false)],
                );
            }
        }
        // A third of the input, then a reset; the dependency's head and a
        // little, then a FIN (an incomplete body), then a retried status.
        "url-cut" => {
            http = retries();
            script("/b0", vec![Reply::Cut(bodies[0].len() / 3, true)]);
            script("/b1", vec![Reply::Cut(100, false), Reply::Status("503 X")]);
        }
        "url-noreply" => {
            http = retries();
            script("/b0", vec![Reply::Drop(true), Reply::Drop(false)]);
            script("/b1", vec![Reply::Drop(false)]);
        }
        // The input's one try fails: no peek, and the arm stays on through
        // the finisher and the run's teardown. The dependency stays local,
        // so no body's reads (whose split follows the peer's timing) move
        // this scope's sites.
        "url-fail" => {
            default("/b0", Reply::Status("404 X"));
            urls[1].clear();
        }
        _ => panic!("unknown url scope: {scope}"),
    }
    (origin, address, http, urls, tls)
}
/// The input and the dependency moved onto `urls` (but for an empty one),
/// with custom headers (literal and from the environment) and the scope's
/// `http` policy.
fn onto(spec: &mut serde_json::Value, urls: &[String; 2], http: serde_json::Value) {
    let headers = serde_json::json!({
        "User-Agent": "sweep",
        "X-Token": "literal-token",
        "X-From-Env": {"env": "UNITYRS_SWEEP_TOKEN"},
    });
    for (list, url) in [("inputs", &urls[0]), ("dependencies", &urls[1])] {
        if url.is_empty() {
            continue;
        }
        let bundle = spec[list][0].as_object_mut().unwrap();
        bundle.remove("path");
        bundle.insert("url".into(), url.clone().into());
        bundle.insert("headers".into(), headers.clone());
    }
    // One thread, one permit and one try keep allocation ordinals fixed.
    spec["threads"] = 1.into();
    spec["download_concurrency"] = 1.into();
    spec["http"] = http;
}
/// Every allocation of a networked run's staging, from its first dispatch,
/// either succeeds or reports `host_memory`/74, but for the listed
/// third-party exclusions; and a refused run resets no connection whose
/// response was still arriving before its finisher.
pub fn child(scope: &str, site: usize) {
    let case = run_support::cases()
        .into_iter()
        .find(|c| c["name"] == "ngui-material_in_dependency")
        .unwrap();
    let scene = run_support::scene(&case);
    let scratch = unityrs_testkit::scratch::Scratch::new("url-refusals");
    std::fs::create_dir(scratch.join("spool")).unwrap();
    std::fs::create_dir(scratch.join("sink")).unwrap();
    let (mut spec, _) = run_support::instantiate(&case, &scene, &scratch);
    // A local copy beside the two URLs.
    let mut copy = spec["dependencies"][0].clone();
    copy["id"] = "local-copy".into();
    spec["dependencies"].as_array_mut().unwrap().push(copy);
    let bodies = [&spec["inputs"][0], &spec["dependencies"][0]]
        .map(|b| std::fs::read(b["path"].as_str().unwrap()).unwrap());
    let (origin, address, http, urls, tls) = serve(scope, &bodies);
    onto(&mut spec, &urls, http);
    let prepared = unityrs_batch::phase1::prepare(
        unityrs_batch::spec::parse(serde_json::to_vec(&spec).unwrap()).unwrap(),
    )
    .unwrap();
    let service = Service::start(
        std::fs::File::create(scratch.join("progress")).unwrap(),
        Snapshot::validate(0),
    )
    .unwrap();
    let lifetime = Arc::new(Lifetime::default());
    let roots = tls
        .as_ref()
        .map(|_| {
            vec![rustls::pki_types::CertificateDer::from(
                super::http_support::tls::ROOT.to_vec(),
            )]
        })
        .unwrap_or_default();
    println!("CONTROLLER_REFUSAL kind={scope} site={site}");
    SEEN.store(0, SeqCst);
    SITE.store(site, SeqCst);
    let result = unityrs_batch::run::drive_with(
        prepared,
        0,
        &service.sender(),
        (&Space, Arc::new(publication_support::Portable)),
        &Options {
            lifetime: lifetime.clone(),
            controller_hooks: Arc::new(Arm {
                stop: scope == "url-stop",
                through: scope == "url-fail",
                polls: AtomicUsize::new(0),
                lifetime,
            }),
            observer: Arc::new(Finish(origin.clone())),
            network: Some(Network {
                environment: Arc::new(Environment),
                hooks: Some(unityrs_batch::run::Hooks {
                    resolver: Arc::new(Resolve {
                        origin: address,
                        tls,
                    }),
                    extra_roots: roots,
                }),
                ..Network::system()
            }),
            ..Options::default()
        },
    );
    ACTIVE.store(false, SeqCst);
    let seen = SEEN.load(SeqCst);
    let end = result.unwrap();
    // A run that refused nothing: the census's, or a child's that never
    // met its aim (a settled one's among them).
    if site == 0 || !super::sites::met() {
        let expected = match scope {
            "url-stop" => Code::Stopped,
            "url-fail" => Code::Incomplete,
            _ => Code::Complete,
        };
        assert_eq!(end.code, expected, "{end:?}");
    }
    if site == 0 {
        assert!(seen > 0);
        println!("CONTROLLER_SITES kind={scope} sites={seen}");
        super::sites::report(scope);
        return;
    }
    if !super::sites::met() {
        println!("CONTROLLER_UNREACHED kind={scope} site={site}");
        return;
    }
    assert_eq!(
        (end.code, end.reason),
        (Code::Host, Some("host_memory")),
        "{site}"
    );
    assert_eq!(EARLY.get().copied(), Some(0), "reset before the finisher");
}
