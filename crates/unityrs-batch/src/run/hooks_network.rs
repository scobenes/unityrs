//! Startup-only parsing of the official resolver and PEM-root hooks.
use super::{Log, Refused};
use crate::http::resolve::{Addresses, Resolver};
use rustls::pki_types::CertificateDer;
use std::{
    ffi::OsString,
    fs::File,
    io::{self, Read},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

const RESOLVE: &str = "UNITYRS_TEST_RESOLVE";
const ROOTS: &str = "UNITYRS_TEST_EXTRA_ROOTS";

/// Parsed `host=addr:port[+ms|+hold:path]` answers. An explicit list never
/// falls back to system DNS, even for a host absent from that list.
/// `Default` is the system resolver, for an extra-roots-only hook.
#[derive(Default)]
pub struct Resolve {
    entries: Option<Vec<Entry>>,
    log: Option<Arc<Log>>,
}
struct Entry {
    host: String,
    address: SocketAddr,
    wait: Wait,
}
enum Wait {
    None,
    Delay(Duration),
    Hold(PathBuf),
}
impl Resolve {
    /// Parse once, before any lookup starts. Numeric addresses alone are
    /// accepted on the right: parsing a hook cannot itself perform DNS.
    pub fn parse(list: &str) -> Result<Self, Refused> {
        let mut entries: Vec<Entry> = Vec::new();
        for entry in list.split(',') {
            let (host, answer) = entry.split_once('=').ok_or(Refused(RESOLVE))?;
            if host.is_empty()
                || host.len() > 253
                || !host
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'.' | b'_'))
                || entries.iter().any(|e| e.host.eq_ignore_ascii_case(host))
            {
                return Err(Refused(RESOLVE));
            }
            let (address, wait) = match answer.split_once('+') {
                None => (answer, Wait::None),
                Some((address, delay)) => {
                    let wait = match delay.strip_prefix("hold:") {
                        Some("") => return Err(Refused(RESOLVE)),
                        Some(path) => {
                            if path.as_bytes().contains(&0) {
                                return Err(Refused(RESOLVE));
                            }
                            let mut owned = OsString::new();
                            owned
                                .try_reserve_exact(path.len())
                                .map_err(|_| Refused(RESOLVE))?;
                            owned.push(path);
                            Wait::Hold(PathBuf::from(owned))
                        }
                        None => {
                            if delay.is_empty() || !delay.bytes().all(|c| c.is_ascii_digit()) {
                                return Err(Refused(RESOLVE));
                            }
                            Wait::Delay(Duration::from_millis(
                                delay.parse().map_err(|_| Refused(RESOLVE))?,
                            ))
                        }
                    };
                    (address, wait)
                }
            };
            let address = address.parse().map_err(|_| Refused(RESOLVE))?;
            let mut owned = String::new();
            owned
                .try_reserve_exact(host.len())
                .map_err(|_| Refused(RESOLVE))?;
            owned.push_str(host);
            entries.try_reserve(1).map_err(|_| Refused(RESOLVE))?;
            entries.push(Entry {
                host: owned,
                address,
                wait,
            });
        }
        Ok(Self {
            entries: Some(entries),
            log: None,
        })
    }
    /// Attach the process's already-open trace log before requests start.
    #[must_use]
    pub fn with_log(mut self, log: Option<Arc<Log>>) -> Self {
        self.log = log;
        self
    }
}
impl Resolver for Resolve {
    fn resolve(&self, host: &str, port: u16) -> io::Result<Addresses> {
        let Some(entries) = &self.entries else {
            let mut name = Vec::new();
            let length = host
                .len()
                .checked_add(1)
                .ok_or(io::ErrorKind::OutOfMemory)?;
            name.try_reserve_exact(length)
                .map_err(|_| io::ErrorKind::OutOfMemory)?;
            name.extend_from_slice(host.as_bytes());
            name.push(0);
            return crate::sys::resolve(&name, port);
        };
        let entry = entries
            .iter()
            .find(|e| e.host.eq_ignore_ascii_case(host))
            .ok_or(io::ErrorKind::NotFound)?;
        match &entry.wait {
            Wait::None => (),
            Wait::Delay(delay) => std::thread::sleep(*delay),
            Wait::Hold(path) => {
                if let Some(log) = &self.log {
                    log.line(format_args!("resolve {host} held"));
                }
                loop {
                    match path.try_exists() {
                        Ok(true) => break,
                        Ok(false) => std::thread::sleep(Duration::from_millis(1)),
                        Err(error) => return Err(error),
                    }
                }
            }
        }
        // The override's port is authoritative, as the documented syntax
        // maps test origins to a peer listening on its own ephemeral port.
        Ok(Addresses::first([entry.address]))
    }
}

/// Read and parse a PEM file at startup. All Rust buffers owned by this
/// parser grow fallibly; rustls validates every decoded certificate before
/// the hook becomes available to a service.
pub fn extra_roots(path: &Path) -> Result<Vec<CertificateDer<'static>>, Refused> {
    let mut file = File::open(path).map_err(|_| Refused(ROOTS))?;
    let mut bytes = Vec::new();
    let mut chunk = [0; 8_192];
    loop {
        let count = match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(count) => count,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Refused(ROOTS)),
        };
        bytes.try_reserve(count).map_err(|_| Refused(ROOTS))?;
        bytes.extend_from_slice(chunk.get(..count).ok_or(Refused(ROOTS))?);
    }
    certificates(&bytes)
}

fn certificates(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>, Refused> {
    const BEGIN: &[u8] = b"-----BEGIN CERTIFICATE-----";
    const END: &[u8] = b"-----END CERTIFICATE-----";
    let mut roots = Vec::new();
    let mut start = None;
    let mut offset = 0;
    for line in pem.split_inclusive(|&c| c == b'\n') {
        let trimmed = line.trim_ascii();
        if trimmed == BEGIN {
            if start.is_some() {
                return Err(Refused(ROOTS));
            }
            start = Some(offset + line.len());
        } else if trimmed == END {
            let begin = start.take().ok_or(Refused(ROOTS))?;
            let bytes = decode(pem.get(begin..offset).ok_or(Refused(ROOTS))?)?;
            let certificate = CertificateDer::from(bytes);
            // rustls's certificate-to-anchor conversion runs before the
            // running head, in the startup configuration allowance C_base.
            let mut check = rustls::RootCertStore::empty();
            check
                .roots
                .try_reserve_exact(1)
                .map_err(|_| Refused(ROOTS))?;
            check
                .add(CertificateDer::from(certificate.as_ref()))
                .map_err(|_| Refused(ROOTS))?;
            roots.try_reserve(1).map_err(|_| Refused(ROOTS))?;
            roots.push(certificate);
        }
        offset += line.len();
    }
    if start.is_some() || roots.is_empty() {
        return Err(Refused(ROOTS));
    }
    Ok(roots)
}

// Decode directly into a single fallibly reserved DER buffer; the PEM
// helper in pki-types has infallibly allocated intermediate buffers.
fn decode(encoded: &[u8]) -> Result<Vec<u8>, Refused> {
    let capacity = encoded
        .len()
        .checked_div(4)
        .and_then(|n| n.checked_mul(3))
        .ok_or(Refused(ROOTS))?;
    let mut decoded = Vec::new();
    decoded
        .try_reserve_exact(capacity)
        .map_err(|_| Refused(ROOTS))?;
    let mut quartet = [0; 4];
    let mut count = 0;
    let mut ended = false;
    for &byte in encoded {
        if byte.is_ascii_whitespace() {
            continue;
        }
        if ended {
            return Err(Refused(ROOTS));
        }
        *quartet.get_mut(count).ok_or(Refused(ROOTS))? = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 64,
            _ => return Err(Refused(ROOTS)),
        };
        count += 1;
        if count == 4 {
            let [a, b, c, d] = quartet;
            if a == 64
                || b == 64
                || (c == 64 && (d != 64 || b & 15 != 0))
                || (d == 64 && c != 64 && c & 3 != 0)
            {
                return Err(Refused(ROOTS));
            }
            decoded.push((a << 2) | (b >> 4));
            if c != 64 {
                decoded.push((b << 4) | (c >> 2));
                if d != 64 {
                    decoded.push((c << 6) | d);
                }
            }
            ended = c == 64 || d == 64;
            count = 0;
        }
    }
    if count != 0 || decoded.is_empty() {
        return Err(Refused(ROOTS));
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Instant};
    use unityrs_testkit::{
        guard::{self, DEADLOCK_GUARD},
        scratch::Scratch,
    };

    #[test]
    fn resolver_answers_use_numeric_override_port_and_never_fall_back() {
        let resolver =
            Resolve::parse("peer.invalid=127.0.0.1:4567,v6.invalid=[::1]:9876+0").unwrap();
        let answer = resolver.resolve("PEER.invalid", 443).unwrap();
        assert_eq!(
            answer.iter().collect::<Vec<_>>(),
            ["127.0.0.1:4567".parse().unwrap()]
        );
        let answer = resolver.resolve("v6.invalid", 80).unwrap();
        assert_eq!(
            answer.iter().collect::<Vec<_>>(),
            ["[::1]:9876".parse().unwrap()]
        );
        // localhost would succeed through system DNS; explicit hooks must
        // instead refuse it when no answer was listed.
        assert_eq!(
            resolver.resolve("localhost", 80).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn resolver_refuses_malformed_and_ambiguous_entries_at_startup() {
        for text in [
            "",
            "bad",
            "=127.0.0.1:80",
            "x=localhost:80",
            "x=127.0.0.1",
            "x=::1:80",
            "x=127.0.0.1:65536",
            "x=127.0.0.1:80,",
            "x=127.0.0.1:80+",
            "x=127.0.0.1:80+-1",
            "x=127.0.0.1:80++1",
            "x=127.0.0.1:80+1+2",
            "x=127.0.0.1:80+18446744073709551616",
            "x=127.0.0.1:80+hold:",
            "x=127.0.0.1:80+hold:a\0b",
            "bad\nhost=127.0.0.1:80",
            "x=127.0.0.1:80,X=127.0.0.1:81",
        ] {
            assert!(
                matches!(Resolve::parse(text), Err(Refused(RESOLVE))),
                "{text:?}"
            );
        }
    }

    #[test]
    fn delay_keeps_the_call_on_its_thread_until_elapsed() {
        let resolver = Resolve::parse("peer.invalid=127.0.0.1:1234+2").unwrap();
        let start = Instant::now();
        assert_eq!(resolver.resolve("peer.invalid", 80).unwrap().len(), 1);
        assert!(start.elapsed() >= Duration::from_millis(2));
    }

    #[test]
    fn held_lookup_traces_before_waiting_and_returns_only_after_release() {
        // Release on assertion failures too, before scratch is removed.
        struct Release(PathBuf);
        impl Drop for Release {
            fn drop(&mut self) {
                let _ = std::fs::write(&self.0, b"");
            }
        }
        let scratch = Scratch::new("resolver-hold");
        let release = scratch.join("release");
        let trace = scratch.join("trace");
        let resolver = Resolve::parse(&format!(
            "peer.invalid=127.0.0.1:1234+hold:{}",
            release.display(),
        ))
        .unwrap()
        .with_log(Some(Arc::new(Log::append(&trace).unwrap())));
        let (sent, received) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            sent.send(resolver.resolve("peer.invalid", 80)).unwrap();
        });
        let release = Release(release);
        let start = Instant::now();
        loop {
            if std::fs::read_to_string(&trace)
                .unwrap()
                .contains("resolve peer.invalid held\n")
            {
                break;
            }
            assert!(
                start.elapsed() < DEADLOCK_GUARD,
                "lookup failed to announce its hold"
            );
            std::thread::yield_now();
        }
        assert!(matches!(
            received.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        drop(release);
        assert_eq!(
            guard::recv(&received, "released resolver answer")
                .unwrap()
                .len(),
            1
        );
    }

    fn pem(bytes: &[u8]) -> Vec<u8> {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = b"-----BEGIN CERTIFICATE-----\r\n".to_vec();
        for chunk in bytes.chunks(3) {
            let a = chunk[0];
            let b = chunk.get(1).copied().unwrap_or(0);
            let c = chunk.get(2).copied().unwrap_or(0);
            out.push(ALPHABET[usize::from(a >> 2)]);
            out.push(ALPHABET[usize::from(((a & 3) << 4) | (b >> 4))]);
            out.push(if chunk.len() > 1 {
                ALPHABET[usize::from(((b & 15) << 2) | (c >> 6))]
            } else {
                b'='
            });
            out.push(if chunk.len() > 2 {
                ALPHABET[usize::from(c & 63)]
            } else {
                b'='
            });
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(b"-----END CERTIFICATE-----\r\n");
        out
    }

    #[test]
    fn pem_file_loads_every_valid_certificate_and_ignores_surrounding_text() {
        let root = include_bytes!("../../tests/http_support/root.der");
        let mut file = b"# synthetic test roots\n".to_vec();
        file.extend_from_slice(&pem(root));
        file.extend_from_slice(b"\n# another root\n");
        file.extend_from_slice(&pem(root));
        let scratch = Scratch::new("resolver-roots");
        let path = scratch.join("roots.pem");
        std::fs::write(&path, file).unwrap();
        let roots = extra_roots(&path).unwrap();
        assert_eq!(roots.len(), 2);
        assert!(roots.iter().all(|certificate| certificate.as_ref() == root));
        assert_eq!(extra_roots(&scratch.join("absent")), Err(Refused(ROOTS)));
    }

    #[test]
    fn pem_refuses_missing_unfinished_invalid_and_non_certificate_data() {
        for input in [
            b"".as_slice(),
            b"not a PEM file",
            b"-----BEGIN CERTIFICATE-----\nAA==\n",
            b"-----END CERTIFICATE-----\n",
            b"-----BEGIN CERTIFICATE-----\n-----BEGIN CERTIFICATE-----\n",
            b"-----BEGIN CERTIFICATE-----\nAA==\n-----END CERTIFICATE-----\n",
        ] {
            assert_eq!(certificates(input), Err(Refused(ROOTS)), "{input:?}");
        }
    }

    #[test]
    fn pem_decoder_checks_padding_bits_and_rejects_trailing_payload() {
        for input in [
            b"".as_slice(),
            b"A",
            b"AAA",
            b"=AAA",
            b"A=AA",
            b"AA=A",
            b"AB==",
            b"AAB=",
            b"AA==AAAA",
            b"AA!A",
        ] {
            assert_eq!(decode(input), Err(Refused(ROOTS)), "{input:?}");
        }
        assert_eq!(decode(b"AQ==").unwrap(), [1]);
        assert_eq!(decode(b"AQI=").unwrap(), [1, 2]);
        assert_eq!(decode(b"AQID \n").unwrap(), [1, 2, 3]);
    }
}
