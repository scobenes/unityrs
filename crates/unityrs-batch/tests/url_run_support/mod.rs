//! A loopback HTTP/1.1 origin for whole library runs, and the spec
//! transform that moves bundles onto it. Plain HTTP on an IP literal needs
//! no resolver hook: the engine's own service and the system resolver serve
//! it. Each connection carries one request; the origin records the request
//! and how the client closed it (FIN, RST, or not yet).
#![allow(dead_code)]
use serde_json::Value;
use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use unityrs_testkit::guard::DEADLOCK_GUARD;

/// One scripted answer; a route answers [`Reply::Ok`] once its script ends.
#[derive(Clone, Debug)]
pub enum Reply {
    Ok,
    /// A status with no body, and extra header lines (each ending `\r\n`).
    Status(u16, String),
    /// A 302 to this path (on this origin), its query included.
    Redirect(String),
    /// The head and this many body bytes, then nothing until the client closes.
    Hold(usize),
    /// Nothing at all, not even the head, until the client closes.
    Silent,
    /// The head, then, once the flag is set, this many body bytes, then
    /// nothing until the client closes.
    HoldAfter(usize, Arc<AtomicBool>),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Close {
    Fin,
    Reset,
}
#[derive(Debug)]
pub struct Seen {
    pub path: String,
    pub head: String,
    /// How the client ended this connection, once it has.
    pub close: Arc<Mutex<Option<Close>>>,
}
#[derive(Default)]
struct State {
    bodies: Mutex<BTreeMap<String, Arc<[u8]>>>,
    scripts: Mutex<BTreeMap<String, VecDeque<Reply>>>,
    seen: Mutex<Vec<Seen>>,
    changed: Condvar,
    closed: AtomicBool,
    open: AtomicUsize,
    most_open: AtomicUsize,
}
pub struct Origin {
    pub address: SocketAddr,
    state: Arc<State>,
}
impl Origin {
    pub fn start() -> Self {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(State::default());
        let accept = state.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if accept.closed.load(Ordering::Acquire) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let state = accept.clone();
                std::thread::spawn(move || {
                    let _ = serve(&state, stream);
                });
            }
        });
        Self { address, state }
    }
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }
    pub fn body(&self, path: &str, bytes: impl Into<Arc<[u8]>>) {
        self.state
            .bodies
            .lock()
            .unwrap()
            .insert(path.into(), bytes.into());
    }
    pub fn script(&self, path: &str, replies: impl IntoIterator<Item = Reply>) {
        self.state
            .scripts
            .lock()
            .unwrap()
            .insert(path.into(), replies.into_iter().collect());
    }
    /// Requests whose path (query excluded) is `path`.
    pub fn requests(&self, path: &str) -> usize {
        self.state
            .seen
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.path.split('?').next() == Some(path))
            .count()
    }
    /// The most connections open at once, a request seen and not yet closed.
    pub fn most_open(&self) -> usize {
        self.state.most_open.load(Ordering::SeqCst)
    }
    pub fn total(&self) -> usize {
        self.state.seen.lock().unwrap().len()
    }
    pub fn heads(&self) -> Vec<String> {
        self.state
            .seen
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.head.clone())
            .collect()
    }
    /// Every connection's close so far, in arrival order.
    pub fn closes(&self) -> Vec<Option<Close>> {
        self.state
            .seen
            .lock()
            .unwrap()
            .iter()
            .map(|s| *s.close.lock().unwrap())
            .collect()
    }
    /// Wait until `count` requests have arrived.
    pub fn wait_requests(&self, count: usize) {
        let deadline = std::time::Instant::now() + DEADLOCK_GUARD;
        let mut seen = self.state.seen.lock().unwrap();
        while seen.len() < count {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(!left.is_zero(), "deadlock guard: {count} requests");
            seen = self.state.changed.wait_timeout(seen, left).unwrap().0;
        }
    }
    /// Wait until every connection has closed.
    pub fn wait_closed(&self) -> Vec<Close> {
        let deadline = std::time::Instant::now() + DEADLOCK_GUARD;
        loop {
            let closes = self.closes();
            if closes.iter().all(Option::is_some) {
                return closes.into_iter().flatten().collect();
            }
            assert!(
                std::time::Instant::now() < deadline,
                "deadlock guard: closes"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Origin {
    fn drop(&mut self) {
        self.state.closed.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.address);
    }
}
fn serve(state: &State, mut stream: TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(DEADLOCK_GUARD))?;
    stream.set_write_timeout(Some(DEADLOCK_GUARD))?;
    let mut head = Vec::new();
    let mut buffer = [0; 4096];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut buffer)?;
        if n == 0 {
            return Ok(());
        }
        head.extend_from_slice(&buffer[..n]);
    }
    let head = String::from_utf8_lossy(&head).into_owned();
    let path = head.split(' ').nth(1).unwrap_or("").to_owned();
    let route = path.split('?').next().unwrap_or("").to_owned();
    let close = Arc::new(Mutex::new(None));
    let reply = state
        .scripts
        .lock()
        .unwrap()
        .get_mut(&route)
        .and_then(VecDeque::pop_front)
        .unwrap_or(Reply::Ok);
    let body = state.bodies.lock().unwrap().get(&route).cloned();
    {
        let mut seen = state.seen.lock().unwrap();
        seen.push(Seen {
            path,
            head,
            close: close.clone(),
        });
        state.changed.notify_all();
    }
    let open = state.open.fetch_add(1, Ordering::SeqCst) + 1;
    state.most_open.fetch_max(open, Ordering::SeqCst);
    let result = answer(&mut stream, reply, body, &close);
    state.open.fetch_sub(1, Ordering::SeqCst);
    result
}
fn answer(
    stream: &mut TcpStream,
    reply: Reply,
    body: Option<Arc<[u8]>>,
    close: &Mutex<Option<Close>>,
) -> io::Result<()> {
    let mut buffer = [0; 4096];
    match (reply, body) {
        (Reply::Ok, Some(body)) => {
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )?;
            stream.write_all(&body)?;
        }
        (Reply::Ok, None) => {
            stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")?;
        }
        (Reply::Status(status, fields), _) => {
            write!(
                stream,
                "HTTP/1.1 {status} Scripted\r\n{fields}Content-Length: 0\r\n\r\n"
            )?;
        }
        (Reply::Redirect(location), _) => {
            write!(
                stream,
                "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n"
            )?;
        }
        (Reply::Hold(count), body) => {
            let body = body.unwrap_or_else(|| Arc::from(vec![0; count + 1]));
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )?;
            stream.write_all(&body[..count.min(body.len())])?;
        }
        (Reply::Silent, _) => {}
        (Reply::HoldAfter(count, flag), body) => {
            let body = body.unwrap_or_else(|| Arc::from(vec![0; count + 1]));
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )?;
            stream.flush()?;
            let deadline = std::time::Instant::now() + DEADLOCK_GUARD;
            while !flag.load(Ordering::SeqCst) {
                assert!(std::time::Instant::now() < deadline, "deadlock guard: flag");
                std::thread::sleep(Duration::from_millis(2));
            }
            stream.write_all(&body[..count.min(body.len())])?;
        }
    }
    stream.flush()?;
    // Observe how the client ends the connection: one request each, so
    // anything it sends now is the close.
    let ended = loop {
        match stream.read(&mut buffer) {
            Ok(0) => break Close::Fin,
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => break Close::Reset,
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                // Still open at the guard: leave the close unset.
                let _ = stream.shutdown(Shutdown::Both);
                return Ok(());
            }
            Err(e) => return Err(e),
        }
    };
    *close.lock().unwrap() = Some(ended);
    Ok(())
}

/// Move bundles onto `origin`: each chosen bundle's `path` becomes a `url`
/// serving the same bytes (`/b<n>`, `n` its position among all bundles).
/// A bundle whose source is missing is served 404.
/// Returns the moved bundles' ids.
pub fn onto(
    origin: &Origin,
    spec: &mut Value,
    mut choose: impl FnMut(usize) -> bool,
) -> Vec<String> {
    let mut moved = Vec::new();
    let mut n = 0;
    for list in ["inputs", "dependencies"] {
        let Some(bundles) = spec[list].as_array_mut() else {
            continue;
        };
        for bundle in bundles {
            let index = n;
            n += 1;
            if !choose(index) {
                continue;
            }
            let fields = bundle.as_object_mut().unwrap();
            moved.push(fields["id"].as_str().unwrap().to_owned());
            let path = fields.remove("path").unwrap();
            let route = format!("/b{index}");
            if let Ok(bytes) = std::fs::read(path.as_str().unwrap()) {
                origin.body(&route, bytes);
            }
            fields.insert("url".into(), origin.url(&route).into());
        }
    }
    moved
}
