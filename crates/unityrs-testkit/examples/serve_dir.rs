//! Loopback HTTP/1.1 static files with repeatable faults and no extra dependencies.
//!
//! `cargo run --release -p unityrs-testkit --example serve_dir -- DIR --port 8080`
//! Add --seed 17 --first-503 1/5 --first-reset 1/7 --slow 1/11 for faults.
//! Fractions select paths by a seeded hash, independently of arrival order.
//! A selected first-try failure happens once per decoded path for this process.
//! Slow paths use --chunk-bytes 1024 --delay-ms 25 (the defaults).
//! Logs identify requests by hash, never by a filename or query string.
#[path = "serve_dir/admission.rs"]
mod admission;
use std::{
    collections::HashSet,
    env, fs,
    io::{self, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

const HELP: &str = "serve_dir DIR [--port 8080] [--seed 0] [--first-503 N/D] \
[--first-reset N/D] [--slow N/D] [--chunk-bytes 1024] [--delay-ms 25]";
#[derive(Clone, Copy, Default)]
struct Fraction {
    numerator: u64,
    denominator: u64,
}
impl Fraction {
    fn parse(text: &str) -> Result<Self, String> {
        let (n, d) = text.split_once('/').ok_or("expected N/D")?;
        let numerator = n.parse().map_err(|_| "invalid numerator")?;
        let denominator = d.parse().map_err(|_| "invalid denominator")?;
        if denominator == 0 || numerator > denominator {
            return Err("fraction must satisfy 0 <= N <= D and D > 0".into());
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }
    fn selects(self, hash: u64) -> bool {
        self.denominator != 0 && hash % self.denominator < self.numerator
    }
}
#[derive(Default)]
struct Options {
    root: PathBuf,
    port: u16,
    seed: u64,
    first_503: Fraction,
    first_reset: Fraction,
    slow: Fraction,
    chunk_bytes: usize,
    delay: Duration,
}
impl Options {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let root = args.next().ok_or(HELP)?;
        let mut options = Self {
            root: fs::canonicalize(root).map_err(|e| format!("directory: {e}"))?,
            port: 8080,
            chunk_bytes: 1024,
            delay: Duration::from_millis(25),
            ..Self::default()
        };
        if !options.root.is_dir() {
            return Err("DIR must be a directory".into());
        }
        while let Some(flag) = args.next() {
            let value = args
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            match flag.as_str() {
                "--port" => options.port = value.parse().map_err(|_| "invalid port")?,
                "--seed" => options.seed = value.parse().map_err(|_| "invalid seed")?,
                "--first-503" => options.first_503 = Fraction::parse(&value)?,
                "--first-reset" => options.first_reset = Fraction::parse(&value)?,
                "--slow" => options.slow = Fraction::parse(&value)?,
                "--chunk-bytes" => {
                    options.chunk_bytes = value.parse().map_err(|_| "invalid chunk size")?;
                    if options.chunk_bytes == 0 || options.chunk_bytes > 65536 {
                        return Err("chunk size must be in 1..=65536".into());
                    }
                }
                "--delay-ms" => {
                    options.delay =
                        Duration::from_millis(value.parse().map_err(|_| "invalid delay")?);
                }
                _ => return Err(format!("unknown option {flag}")),
            }
        }
        Ok(options)
    }
}
struct Server {
    options: Options,
    tried: Mutex<HashSet<PathBuf>>,
}
fn main() -> io::Result<()> {
    if env::args()
        .nth(1)
        .is_some_and(|s| s == "--help" || s == "-h")
    {
        println!("{HELP}");
        return Ok(());
    }
    let options = Options::parse(env::args().skip(1))
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("{e}\n{HELP}")))?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, options.port))?;
    println!("LISTEN http://{}", listener.local_addr()?);
    io::stdout().flush()?;
    let server = Arc::new(Server {
        options,
        tried: Mutex::new(HashSet::new()),
    });
    admission::serve(&server, listener.incoming(), admission::WORKERS, |job| {
        std::thread::Builder::new()
            .name("serve-dir".into())
            .spawn(job)
            .map(drop)
    })
}

fn hash(bytes: &[u8], seed: u64) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325 ^ seed, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3)
    })
}
/// Decode the URL path once. No traversal, NUL, backslashes or empty segments.
fn relative(target: &str) -> Option<PathBuf> {
    let path = target.split('?').next()?.strip_prefix('/')?;
    let mut decoded = Vec::with_capacity(path.len());
    let mut bytes = path.bytes();
    while let Some(byte) = bytes.next() {
        decoded.push(if byte == b'%' {
            let hi = char::from(bytes.next()?).to_digit(16)?;
            let lo = char::from(bytes.next()?).to_digit(16)?;
            u8::try_from(hi * 16 + lo).ok()?
        } else {
            byte
        });
    }
    let decoded = String::from_utf8(decoded).ok()?;
    if decoded.contains(['\0', '\\'])
        || decoded
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == "..")
    {
        return None;
    }
    Some(PathBuf::from(decoded))
}
fn head(stream: &mut TcpStream, status: &str, size: u64) -> io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"
    )
}
impl Server {
    fn serve(&self, mut stream: TcpStream) -> io::Result<()> {
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        let mut request = Vec::new();
        let mut buffer = [0; 4096];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            if request.len() >= 65536 {
                return head(&mut stream, "431 Request Header Fields Too Large", 0);
            }
            let n = stream.read(&mut buffer)?;
            if n == 0 {
                return Ok(());
            }
            request.extend_from_slice(&buffer[..n]);
        }
        let text = std::str::from_utf8(&request).unwrap_or("");
        let mut parts = text.lines().next().unwrap_or("").split_whitespace();
        let method = parts.next().unwrap_or("");
        let target = parts.next().unwrap_or("");
        if !matches!(method, "GET" | "HEAD") {
            return head(&mut stream, "405 Method Not Allowed", 0);
        }
        let Some(relative) = relative(target) else {
            return head(&mut stream, "400 Bad Request", 0);
        };
        let path = match fs::canonicalize(self.options.root.join(&relative)) {
            Ok(path) if path.starts_with(&self.options.root) && path.is_file() => path,
            _ => return head(&mut stream, "404 Not Found", 0),
        };
        let mut file = fs::File::open(path)?;
        let size = file.metadata()?.len();
        let key = hash(relative.as_os_str().as_encoded_bytes(), self.options.seed);
        let status_fault = self.options.first_503.selects(key);
        let reset_fault = size >= 2 && self.options.first_reset.selects(key.rotate_left(21));
        let first = (status_fault || reset_fault) && self.tried.lock().unwrap().insert(relative);
        if first && status_fault {
            println!("REQUEST key={key:016x} mode=first-503 bytes=0");
            return head(&mut stream, "503 Service Unavailable", 0);
        }
        let reset = first && reset_fault;
        let slow = self.options.slow.selects(key.rotate_left(42));
        let mode = if reset {
            "first-reset"
        } else if slow {
            "slow"
        } else {
            "clean"
        };
        println!("REQUEST key={key:016x} mode={mode} bytes={size}");
        head(&mut stream, "200 OK", size)?;
        if method == "HEAD" {
            return Ok(());
        }
        let mut left = if reset { size / 2 } else { size };
        let chunk = if slow {
            self.options.chunk_bytes
        } else {
            buffer.len()
        };
        let mut buffer = vec![0; chunk];
        while left > 0 {
            let cap = usize::try_from(left.min(chunk as u64)).unwrap();
            let n = file.read(&mut buffer[..cap])?;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            stream.write_all(&buffer[..n])?;
            left -= n as u64;
            if slow && left > 0 {
                std::thread::sleep(self.options.delay);
            }
        }
        if reset {
            reset_on_close(&stream)?;
        }
        Ok(())
    }
}
/// Zero linger makes dropping the connection send RST after the partial body.
#[cfg(unix)]
fn reset_on_close(stream: &TcpStream) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let linger = libc::linger {
        l_onoff: 1,
        l_linger: 0,
    };
    let length = libc::socklen_t::try_from(std::mem::size_of_val(&linger)).unwrap();
    // SAFETY: stream owns a live socket; linger and its length describe a
    // valid SO_LINGER value, used only for the duration of setsockopt.
    let result = unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_LINGER,
            (&raw const linger).cast(),
            length,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
#[cfg(not(unix))]
fn reset_on_close(_: &TcpStream) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use unityrs_testkit::scratch::Scratch;

    pub(super) fn server(root: &Path, extra: &[&str]) -> Server {
        let args = std::iter::once(root.to_str().unwrap().to_owned())
            .chain(extra.iter().map(|s| (*s).to_owned()));
        Server {
            options: Options::parse(args).unwrap(),
            tried: Mutex::new(HashSet::new()),
        }
    }
    fn request(server: Arc<Server>, route: &str) -> (Vec<u8>, Option<io::ErrorKind>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        client
            .set_read_timeout(Some(unityrs_testkit::guard::DEADLOCK_GUARD))
            .unwrap();
        let worker = std::thread::spawn(move || server.serve(listener.accept().unwrap().0));
        write!(client, "GET {route} HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        let mut bytes = Vec::new();
        let error = client.read_to_end(&mut bytes).err().map(|e| e.kind());
        worker.join().unwrap().unwrap();
        (bytes, error)
    }
    #[test]
    fn exact_length_binary_body_and_containment() {
        let scratch = Scratch::new("serve-dir");
        fs::create_dir(scratch.join("root")).unwrap();
        fs::write(scratch.join("root/a b"), [0, 1, 255]).unwrap();
        fs::write(scratch.join("outside"), b"private").unwrap();
        let server = Arc::new(server(&scratch.join("root"), &[]));
        let (bytes, error) = request(server.clone(), "/a%20b?ignored=yes");
        assert_eq!(error, None);
        assert_eq!(
            bytes,
            b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\n\0\x01\xff"
        );
        for route in [
            "/%2e%2e/outside",
            "/../outside",
            "/a%00b",
            "/a%ZZ",
            "/missing",
        ] {
            let (bytes, _) = request(server.clone(), route);
            assert!(!bytes.windows(7).any(|w| w == b"private"));
            assert!(!bytes.starts_with(b"HTTP/1.1 200"));
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(scratch.join("outside"), scratch.join("root/link")).unwrap();
            assert!(request(server, "/link").0.starts_with(b"HTTP/1.1 404"));
        }
    }
    #[test]
    fn seeded_selection_is_order_independent_and_503_only_once() {
        let scratch = Scratch::new("serve-dir-503");
        for name in ["a", "b", "c", "d", "e", "f", "g", "h"] {
            fs::write(scratch.join(name), name).unwrap();
        }
        let flags = ["--seed", "17", "--first-503", "1/2"];
        let first = Arc::new(server(scratch.path(), &flags));
        let second = Arc::new(server(scratch.path(), &flags));
        let routes = ["/a", "/b", "/c", "/d", "/e", "/f", "/g", "/h"];
        let statuses: Vec<_> = routes
            .iter()
            .map(|r| request(first.clone(), r).0[..12].to_vec())
            .collect();
        assert!(statuses.iter().any(|s| s == b"HTTP/1.1 503"));
        assert!(statuses.iter().any(|s| s == b"HTTP/1.1 200"));
        for (route, expected) in routes.iter().zip(statuses).rev() {
            assert_eq!(&request(second.clone(), route).0[..12], expected);
            assert!(
                request(first.clone(), &format!("{route}?retry=1"))
                    .0
                    .starts_with(b"HTTP/1.1 200")
            );
        }
    }
    #[test]
    #[cfg(unix)]
    fn midbody_reset_is_observed_and_retry_gets_the_whole_file() {
        let scratch = Scratch::new("serve-dir-reset");
        fs::write(scratch.join("body"), vec![7; 32768]).unwrap();
        let server = Arc::new(server(scratch.path(), &["--first-reset", "1/1"]));
        let (bytes, error) = request(server.clone(), "/body");
        assert_eq!(error, Some(io::ErrorKind::ConnectionReset));
        let offset = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        assert_eq!(bytes.len() - offset, 16384);
        let (bytes, error) = request(server, "/body");
        assert_eq!(error, None);
        assert!(bytes.ends_with(&vec![7; 32768]));
    }
    #[test]
    fn slow_trickle_preserves_exact_body_and_options_are_checked() {
        let scratch = Scratch::new("serve-dir-slow");
        fs::write(scratch.join("body"), b"123456789").unwrap();
        let server = Arc::new(server(
            scratch.path(),
            &["--slow", "1/1", "--chunk-bytes", "2", "--delay-ms", "10"],
        ));
        let started = std::time::Instant::now();
        assert!(request(server, "/body").0.ends_with(b"123456789"));
        assert!(started.elapsed() >= Duration::from_millis(40));
        for bad in ["1/0", "2/1", "x/2", "1"] {
            assert!(Fraction::parse(bad).is_err());
        }
        assert!(
            Options::parse(
                [
                    scratch.path().display().to_string(),
                    "--chunk-bytes".into(),
                    "0".into()
                ]
                .into_iter()
            )
            .is_err()
        );
    }
}
