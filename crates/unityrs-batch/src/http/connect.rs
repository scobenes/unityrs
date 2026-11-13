//! One deadline for resolution, sequential TCP attempts, and the TLS handshake.
use super::{
    Detail, Error,
    io::Stream,
    permit::RequestPermit,
    resolve::{self, Source},
    socket::{Registration, SocketTable},
};
use rustls::{ClientConfig, pki_types::ServerName};
use std::{future::poll_fn, os::fd::AsRawFd, sync::Arc, time::Duration};
use tokio::{
    net::TcpSocket,
    runtime::Handle,
    time::{Instant, timeout_at},
};
use tokio_rustls::TlsConnector;
use url::Url;

pub(crate) struct Connector {
    pub(crate) pool: Handle,
    pub(crate) table: Arc<dyn SocketTable>,
    pub(crate) tls: Arc<ClientConfig>,
    pub(crate) resolver: Source,
}
impl Connector {
    pub(crate) async fn connect(
        &self,
        permit: &mut RequestPermit,
        url: &Url,
        timeout: Duration,
    ) -> Result<Stream, Error> {
        // A previous timed-out lookup's wait is outside this connection timer.
        poll_fn(|cx| permit.poll_available(cx)).await;
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(Error::InvalidRequest)?;
        timeout_at(deadline, self.until(permit, url, deadline))
            .await
            .unwrap_or_else(|_| Err(Error::transport(Detail::Connect)))
    }
    async fn until(
        &self,
        permit: &mut RequestPermit,
        url: &Url,
        deadline: Instant,
    ) -> Result<Stream, Error> {
        let host = url.host_str().ok_or(Error::InvalidRequest)?;
        // url's IPv6 host_str includes brackets; the resolver and ServerName
        // consume the address itself, whereas Host keeps the authority form.
        let host = host
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(host);
        let port = url.port_or_known_default().ok_or(Error::InvalidRequest)?;
        let addresses = resolve::lookup(&self.pool, permit, &self.resolver, host, port).await?;
        let mut last = Error::transport(Detail::Connect);
        let count = addresses.len();
        for index in 0..count {
            let address = addresses.get(index).ok_or(Error::InvalidRequest)?;
            let remaining = u32::try_from(count - index).map_err(|_| Error::InvalidRequest)?;
            let now = Instant::now();
            let left = deadline.saturating_duration_since(now);
            if left.is_zero() {
                return Err(Error::transport(Detail::Connect));
            }
            let share = now + left / remaining;
            let socket = match if address.is_ipv4() {
                TcpSocket::new_v4()
            } else {
                TcpSocket::new_v6()
            } {
                Ok(socket) => socket,
                Err(error) => {
                    last = super::classify::connect(&error);
                    continue;
                }
            };
            if let Err(error) = socket.set_zero_linger() {
                last = super::classify::connect(&error);
                continue;
            }
            let registration = Registration::new(&self.table, socket.as_raw_fd());
            let registration = match registration {
                Ok(registration) => registration,
                Err(error) => {
                    drop(socket);
                    return Err(error);
                }
            };
            if let Err(error) = registration.check() {
                drop(socket);
                drop(registration);
                return Err(error);
            }
            // timeout_at owns the connect future: it drops that future (and
            // its socket) before this registration is removed on failure.
            match timeout_at(share, socket.connect(address)).await {
                Ok(Ok(stream)) => {
                    let stream = registration.wrap(stream);
                    return self.secure(stream, url, host).await;
                }
                Ok(Err(error)) => last = super::classify::connect(&error),
                Err(_) => last = Error::transport(Detail::Connect),
            }
            drop(registration);
        }
        Err(last)
    }
    async fn secure(
        &self,
        stream: super::socket::Registered<tokio::net::TcpStream>,
        url: &Url,
        host: &str,
    ) -> Result<Stream, Error> {
        if url.scheme() == "http" {
            return Ok(Stream::Plain(stream));
        }
        let mut owned = String::new();
        owned
            .try_reserve_exact(host.len())
            .map_err(|_| Error::Memory)?;
        owned.push_str(host);
        let name = ServerName::try_from(owned).map_err(|_| Error::InvalidRequest)?;
        let stream = TlsConnector::from(self.tls.clone())
            .connect(name, stream)
            .await
            .map_err(|e| super::classify::tls(&e))?;
        let stream = crate::alloc::try_box(stream).map_err(|_| Error::Memory)?;
        Ok(Stream::Tls(stream))
    }
}
