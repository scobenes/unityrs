//! Synthetic localhost TLS peer and fixed socket-table adapter; no DNS fallback.
use rustls::{
    ServerConfig, ServerConnection, StreamOwned,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener},
    os::fd::RawFd,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
};
use unityrs_batch::http::{
    Error, Service,
    resolve::{Addresses, Resolver},
    socket::SocketTable,
};
use unityrs_testkit::guard::{self, DEADLOCK_GUARD};
#[path = "reframe.rs"]
mod reframe;

pub const ROOT: &[u8] = include_bytes!("root.der");
const LEAF: &[u8] = include_bytes!("leaf.der");
const KEY: &[u8] = include_bytes!("key.der");
// Synthetic siblings signed by ROOT using the same test key: the wrong-host
// leaf has only DNS:wrong.invalid; the expired leaf was valid in 2000–2001.
pub const WRONG_HOST: &[u8] = include_bytes!("wrong-host.der");
pub const EXPIRED: &[u8] = include_bytes!("expired.der");
#[derive(Clone, Copy, Debug)]
pub enum Handshake {
    Eof,
    Reset,
    Stall,
}
static SERIAL: Mutex<()> = Mutex::new(());
pub fn lock() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub struct Peer {
    pub address: SocketAddr,
    done: mpsc::Receiver<io::Result<Observation>>,
}
#[derive(Debug)]
pub struct Observation {
    pub request: Vec<u8>,
    pub alpn: Option<Vec<u8>>,
    pub reset: bool,
    pub close_notify: bool,
    pub client_hello: Vec<u8>,
    pub server_name: Option<String>,
}
impl Peer {
    pub fn with_config(response: Vec<u8>, config: ServerConfig) -> Self {
        Self::configured(response, config, false)
    }
    pub fn with_aligned_config(response: Vec<u8>, config: ServerConfig) -> Self {
        Self::configured(response, config, true)
    }
    fn configured(response: Vec<u8>, config: ServerConfig, aligned: bool) -> Self {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (send, done) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = tls_peer(&listener, &response, true, Arc::new(config), aligned);
            let _ = send.send(result);
        });
        Self { address, done }
    }
    pub fn handshake(mode: Handshake) -> Self {
        use std::os::fd::AsRawFd;
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (send, done) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = (|| {
                let (mut socket, _) = listener.accept()?;
                socket.set_read_timeout(Some(DEADLOCK_GUARD))?;
                let mut buffer = [0; 4_096];
                let count = socket.read(&mut buffer)?;
                assert!(count > 0, "client entered TLS handshake");
                match mode {
                    Handshake::Reset => {
                        unityrs_batch::sys::test_set_zero_linger(socket.as_raw_fd())?;
                    }
                    Handshake::Eof => socket.shutdown(std::net::Shutdown::Write)?,
                    Handshake::Stall => (),
                }
                let reset = if matches!(mode, Handshake::Reset) {
                    drop(socket);
                    false
                } else {
                    loop {
                        match socket.read(&mut buffer) {
                            Ok(0) => break false,
                            Ok(_) => (),
                            Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {
                                break true;
                            }
                            Err(error) => return Err(error),
                        }
                    }
                };
                Ok(Observation {
                    request: Vec::new(),
                    alpn: None,
                    reset,
                    close_notify: false,
                    client_hello: Vec::new(),
                    server_name: None,
                })
            })();
            let _ = send.send(result);
        });
        Self { address, done }
    }
    pub fn tls(response: Vec<u8>, hold_close: bool) -> Self {
        Self::start(response, hold_close, false, false, LEAF)
    }
    pub fn certificate(response: Vec<u8>, leaf: &'static [u8]) -> Self {
        Self::start(response, false, false, false, leaf)
    }
    pub fn tls_stress(response: Vec<u8>) -> Self {
        Self::start(response, true, true, true, LEAF)
    }
    pub fn tls_large_chain(response: Vec<u8>) -> Self {
        Self::start(response, true, true, false, LEAF)
    }
    pub fn tls_many_authorities(response: Vec<u8>) -> Self {
        Self::start(response, true, false, true, LEAF)
    }
    pub fn twice(response: Vec<u8>) -> [Self; 2] {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (first_send, first) = mpsc::sync_channel(1);
        let (second_send, second) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let config = Arc::new(server_config(false, false, LEAF));
            assert!(config.send_tls13_tickets > 0);
            for send in [first_send, second_send] {
                let _ = send.send(tls_peer(&listener, &response, true, config.clone(), false));
            }
        });
        [
            Self {
                address,
                done: first,
            },
            Self {
                address,
                done: second,
            },
        ]
    }
    fn start(
        response: Vec<u8>,
        hold_close: bool,
        large_chain: bool,
        many_authorities: bool,
        leaf: &'static [u8],
    ) -> Self {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (send, done) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let config = Arc::new(server_config(large_chain, many_authorities, leaf));
            let _ = send.send(tls_peer(&listener, &response, hold_close, config, false));
        });
        Self { address, done }
    }
    pub fn finish(self) -> io::Result<Observation> {
        guard::recv(&self.done, "loopback TLS peer")
    }
}
fn tls_peer(
    listener: &TcpListener,
    response: &[u8],
    hold_close: bool,
    config: Arc<ServerConfig>,
    aligned: bool,
) -> io::Result<Observation> {
    let (socket, _) = listener.accept()?;
    socket.set_read_timeout(Some(DEADLOCK_GUARD))?;
    socket.set_write_timeout(Some(DEADLOCK_GUARD))?;
    let mut tls = StreamOwned::new(
        ServerConnection::new(config).unwrap(),
        HelloSocket {
            socket,
            hello: Vec::new(),
            reframer: aligned.then(reframe::Reframer::default),
        },
    );
    let mut request = Vec::new();
    let mut buffer = [0; 1_024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        let count = tls.read(&mut buffer)?;
        if count == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        request.extend_from_slice(&buffer[..count]);
    }
    tls.write_all(response)?;
    tls.flush()?;
    let alpn = tls.conn.alpn_protocol().map(<[u8]>::to_vec);
    let server_name = tls.conn.server_name().map(str::to_owned);
    let mut close_notify = false;
    let reset = if hold_close {
        loop {
            match tls.sock.read(&mut buffer) {
                Ok(0) => break false,
                Ok(count) => {
                    tls.conn.read_tls(&mut &buffer[..count])?;
                    let state = tls.conn.process_new_packets().map_err(io::Error::other)?;
                    close_notify |= state.peer_has_closed();
                }
                Err(error) => break error.kind() == io::ErrorKind::ConnectionReset,
            }
        }
    } else {
        false
    };
    Ok(Observation {
        request,
        alpn,
        reset,
        close_notify,
        client_hello: tls.sock.hello,
        server_name,
    })
}

struct HelloSocket {
    socket: std::net::TcpStream,
    hello: Vec<u8>,
    reframer: Option<reframe::Reframer>,
}
impl Read for HelloSocket {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = self.socket.read(bytes)?;
        // Capture the first record only; it contains rustls's ClientHello.
        // Keeping later application data would distort the allocation scenes.
        let target = if self.hello.len() >= 5 {
            5 + usize::from(u16::from_be_bytes([self.hello[3], self.hello[4]]))
        } else {
            5
        };
        if self.hello.len() < target {
            self.hello.extend_from_slice(&bytes[..count]);
        }
        Ok(count)
    }
}
impl Write for HelloSocket {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self.reframer.as_mut() {
            Some(reframer) => reframer.write(&mut self.socket, bytes),
            None => self.socket.write(bytes),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self.reframer.as_mut() {
            Some(reframer) => reframer.flush(&mut self.socket),
            None => self.socket.flush(),
        }
    }
}

pub struct FixedResolver(pub Vec<SocketAddr>);
impl Resolver for FixedResolver {
    fn resolve(&self, host: &str, _: u16) -> io::Result<Addresses> {
        assert!(matches!(host, "localhost" | "127.0.0.1" | "::1"));
        Ok(Addresses::first(self.0.iter().copied()))
    }
}
#[derive(Default)]
pub struct Table {
    pub closing: AtomicBool,
    pub close_on_register: AtomicBool,
    pub added: AtomicUsize,
    pub removed: AtomicUsize,
    descriptors: Mutex<[Option<RawFd>; 32]>,
}
impl Table {
    pub fn service(
        self: &Arc<Self>,
        handle: tokio::runtime::Handle,
        addresses: &[SocketAddr],
        roots: bool,
    ) -> Service {
        let roots = if roots {
            vec![CertificateDer::from(ROOT.to_vec())]
        } else {
            Vec::new()
        };
        Service::new(handle, self.clone())
            .unwrap()
            .with_test_hooks(Arc::new(FixedResolver(addresses.to_vec())), &roots)
            .unwrap()
    }
}
impl SocketTable for Table {
    fn register(&self, descriptor: RawFd) -> Result<usize, Error> {
        assert!(unityrs_batch::sys::test_zero_linger(descriptor).unwrap());
        let entry = self.added.fetch_add(1, Ordering::AcqRel);
        self.descriptors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[entry] = Some(descriptor);
        if self.close_on_register.load(Ordering::Acquire) {
            self.closing.store(true, Ordering::Release);
        }
        Ok(entry)
    }
    fn closing(&self) -> bool {
        self.closing.load(Ordering::Acquire)
    }
    fn remove(&self, entry: usize) {
        let descriptor = self
            .descriptors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)[entry]
            .take()
            .unwrap();
        assert!(
            !unityrs_batch::sys::test_socket_live(descriptor),
            "descriptor closed before clearing table"
        );
        self.removed.fetch_add(1, Ordering::AcqRel);
    }
}

// Large TLS 1.2 certificate and authority lists exercise both deframing and
// per-item allocations. Leave wire-framing headroom in the accepted authority
// flight; the combined flight deliberately hits rustls's deframer limit.
// Everything here is synthetic and owned by the peer.
fn server_config(large_chain: bool, many_authorities: bool, leaf: &[u8]) -> ServerConfig {
    let versions = if large_chain || many_authorities {
        vec![&rustls::version::TLS12]
    } else {
        vec![&rustls::version::TLS13, &rustls::version::TLS12]
    };
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&versions)
        .unwrap();
    let builder = if many_authorities {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(CertificateDer::from(ROOT.to_vec())).unwrap();
        let verifier =
            rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
                .clear_root_hint_subjects()
                .add_root_hint_subjects(
                    std::iter::repeat_with(|| {
                        // Keep the original malformed stress input as a
                        // rejection case. Accepted names carry a DER SEQUENCE.
                        rustls::DistinguishedName::from(if large_chain {
                            Vec::new()
                        } else {
                            vec![0x30, 0]
                        })
                    })
                    .take(if large_chain { 32_600 } else { 16_000 }),
                )
                .allow_unauthenticated()
                .build()
                .unwrap();
        builder.with_client_cert_verifier(verifier)
    } else {
        builder.with_no_client_auth()
    };
    let count = if large_chain {
        60_000 / (leaf.len() + 3)
    } else {
        1
    };
    let chain = std::iter::repeat_with(|| CertificateDer::from(leaf.to_vec()))
        .take(count)
        .collect();
    let mut config = builder
        .with_single_cert(
            chain,
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(KEY.to_vec())),
        )
        .unwrap();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    config
}
